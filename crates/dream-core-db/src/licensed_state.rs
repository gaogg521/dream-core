//! Read-time authorization from the vendor-signed envelope, never SQL projections.
//! No cache: expiry and reactivation take effect on the next request.

use crate::{DbError, DbPool, db_params};
use dream_core_common::license::{Feature, Tier, tier_allows};
use dream_core_common::license_key::{LicensePayload, verify_license_signature};

#[derive(Debug, Clone)]
pub struct LicensedState {
    pub payload: Option<LicensePayload>,
    pub activated_at: i64,
    pub status: &'static str,
    pub checked_at: i64,
    tier_ceiling: Tier,
}

impl LicensedState {
    pub fn tier(&self) -> Tier {
        self.payload
            .as_ref()
            .filter(|p| p.exp.is_none_or(|e| self.checked_at < e))
            .map(|p| match (self.tier_ceiling, Tier::parse(&p.tier)) {
                (Tier::Free, _) | (_, Tier::Free) => Tier::Free,
                (Tier::Team, _) | (_, Tier::Team) => Tier::Team,
                _ => Tier::Enterprise,
            })
            .unwrap_or(Tier::Free)
    }

    pub fn allows(&self, feature: Feature) -> bool {
        tier_allows(self.tier(), feature)
    }
}

pub async fn read_license_state(pool: &DbPool, enterprise_id: &str) -> Result<LicensedState, DbError> {
    read_license_state_using(pool, enterprise_id, verify_license_signature).await
}

/// Verification dependency for isolated tests. Production callers use
/// `read_license_state`, whose trust key cannot be supplied by a request or DB.
#[doc(hidden)]
pub async fn read_license_state_using(
    pool: &DbPool,
    enterprise_id: &str,
    verify: fn(&str) -> Result<LicensePayload, dream_core_common::license_key::LicenseKeyError>,
) -> Result<LicensedState, DbError> {
    let now = dream_core_common::now_ms();
    let mut state = LicensedState {
        payload: None,
        activated_at: 0,
        status: "free",
        checked_at: now,
        tier_ceiling: Tier::Free,
    };
    let stored_tier: Option<String> = pool
        .fetch_optional_scalar(
            "SELECT tier FROM one_enterprise_license WHERE enterprise_id = ?",
            &db_params![enterprise_id],
        )
        .await?;
    state.tier_ceiling = stored_tier.as_deref().map(Tier::parse).unwrap_or(Tier::Free);
    if stored_tier.is_some_and(|t| Tier::parse(&t) != Tier::Free) {
        state.status = "unofficial";
    }
    // Latest activation replaces earlier grants, never unions them. A corrupt
    // latest row must not silently resurrect an older, more permissive license.
    let row: Option<(String, Option<String>, i64)> = pool.fetch_optional_as(
        "SELECT license_id, license_key, activated_at FROM one_license_activation WHERE enterprise_id = ? ORDER BY activated_at DESC, license_id DESC LIMIT 1",
        &db_params![enterprise_id],
    ).await?;
    let Some((license_id, key, activated_at)) = row else {
        return Ok(state);
    };
    state.status = "unofficial";
    let Some(key) = key else { return Ok(state) };
    let Ok(payload) = verify(&key) else {
        tracing::warn!(enterprise_id, license_id, "stored license signature rejected");
        return Ok(state);
    };
    if payload.lid != license_id || !payload.valid_for_instance(enterprise_id) {
        tracing::warn!(enterprise_id, license_id, "stored license identity rejected");
        return Ok(state);
    }
    if let Some(bound) = &payload.deployment_fingerprint {
        let fingerprint: Option<String> = pool
            .fetch_optional_scalar(
                "SELECT fingerprint FROM one_license_installation WHERE singleton_id = 1",
                &db_params![],
            )
            .await?;
        if !fingerprint.is_some_and(|f| f.eq_ignore_ascii_case(bound)) {
            tracing::warn!(enterprise_id, license_id, "stored license deployment binding rejected");
            return Ok(state);
        }
    }
    state.status = if payload.exp.is_some_and(|e| now >= e) {
        "free"
    } else {
        "official"
    };
    state.activated_at = activated_at;
    state.payload = Some(payload);
    Ok(state)
}

/// Personal installations have no enterprise membership. Once membership is
/// present, any license read failure denies paid capabilities.
pub async fn user_feature_allowed(pool: &DbPool, user_id: &str, feature: Feature) -> Result<bool, DbError> {
    let enterprise_id = match pool
        .fetch_optional_scalar::<String>(
            "SELECT enterprise_id FROM one_enterprise_members WHERE user_id = ? ORDER BY enterprise_id LIMIT 1",
            &db_params![user_id],
        )
        .await
    {
        Ok(id) => id,
        Err(e) if crate::message_indicates_missing_table(&e.to_string()) => return Ok(true),
        Err(e) => return Err(e.into()),
    };
    let Some(id) = enterprise_id else { return Ok(true) };
    Ok(read_license_state(pool, &id)
        .await
        .map(|s| s.allows(feature))
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use dream_core_common::license_key::{sign_license_key, verify_license_signature_with_public_key};

    fn verify_test_key(key: &str) -> Result<LicensePayload, dream_core_common::license_key::LicenseKeyError> {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        verify_license_signature_with_public_key(key, &sk.verifying_key().to_bytes())
    }

    async fn setup() -> (DbPool, LicensePayload) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE one_enterprise_license (enterprise_id TEXT PRIMARY KEY, tier TEXT); CREATE TABLE one_license_activation (license_id TEXT PRIMARY KEY, enterprise_id TEXT, license_key TEXT, activated_at INTEGER); CREATE TABLE one_license_installation (singleton_id INTEGER PRIMARY KEY, fingerprint TEXT); INSERT INTO one_enterprise_license VALUES ('ent', 'enterprise'); INSERT INTO one_license_installation VALUES (1, 'sha256:test-install');").execute(&pool).await.unwrap();
        let payload = serde_json::from_value(serde_json::json!({
            "lid":"signed", "customer":"isolated test", "tier":"enterprise", "seats":7, "iat":0,
            "instance_id":"ent", "deployment_fingerprint":"sha256:test-install",
            "modules":[{"module":"/client/*"}], "tenant_cap":2,
        }))
        .unwrap();
        (DbPool::Sqlite(pool), payload)
    }

    async fn store(db: &DbPool, p: &LicensePayload) {
        let key = sign_license_key(p, &URL_SAFE_NO_PAD.encode([42u8; 32])).unwrap();
        db.execute("INSERT INTO one_license_activation VALUES (?, 'ent', ?, 1) ON CONFLICT(license_id) DO UPDATE SET license_key = excluded.license_key", &db_params![&p.lid, key]).await.unwrap();
    }

    async fn read(db: &DbPool) -> LicensedState {
        read_license_state_using(db, "ent", verify_test_key).await.unwrap()
    }

    #[tokio::test]
    async fn unsigned_or_fake_activation_never_grants_features() {
        let (db, _) = setup().await;
        assert_eq!(read(&db).await.status, "unofficial");
        db.execute(
            "INSERT INTO one_license_activation VALUES ('forged', 'ent', NULL, 2)",
            &[],
        )
        .await
        .unwrap();
        let state = read_license_state(&db, "ent").await.unwrap();
        assert_eq!(state.status, "unofficial");
        for f in dream_core_common::license::ALL_FEATURES {
            assert!(!state.allows(f));
        }
        db.execute(
            "UPDATE one_license_activation SET license_key = 'ONEWORK-invalid.signature'",
            &[],
        )
        .await
        .unwrap();
        assert!(read(&db).await.payload.is_none());
    }

    #[tokio::test]
    async fn signed_claims_survive_projection_tampering_but_not_binding_changes() {
        let (db, payload) = setup().await;
        store(&db, &payload).await;
        let state = read(&db).await;
        assert_eq!(state.status, "official");
        assert_eq!(state.tier(), Tier::Enterprise);
        assert_eq!(state.payload.as_ref().unwrap().seats, Some(7));
        assert_eq!(state.payload.as_ref().unwrap().modules[0].module, "/client/*");
        // Production identity must reject the throwaway issuer used by this fixture.
        assert_eq!(read_license_state(&db, "ent").await.unwrap().status, "unofficial");
        db.execute("UPDATE one_license_installation SET fingerprint = 'foreign'", &[])
            .await
            .unwrap();
        assert_eq!(read(&db).await.status, "unofficial");
    }

    #[tokio::test]
    async fn signed_expiry_and_instance_are_rechecked_on_every_read() {
        let (db, mut payload) = setup().await;
        payload.exp = Some(1);
        store(&db, &payload).await;
        let state = read(&db).await;
        assert!(state.payload.is_some());
        assert_eq!(state.tier(), Tier::Free);
        assert!(!state.allows(Feature::AuditLog));
        payload.exp = None;
        payload.instance_id = Some("foreign".into());
        store(&db, &payload).await;
        assert_eq!(read(&db).await.status, "unofficial");
    }

    #[tokio::test]
    async fn latest_invalid_activation_cannot_resurrect_older_grants() {
        let (db, payload) = setup().await;
        store(&db, &payload).await;
        db.execute(
            "INSERT INTO one_license_activation VALUES ('new-invalid', 'ent', NULL, 2)",
            &[],
        )
        .await
        .unwrap();
        assert_eq!(read(&db).await.status, "unofficial");
    }

    #[tokio::test]
    async fn local_downgrades_cannot_raise_signed_tier() {
        let (db, mut payload) = setup().await;
        payload.tier = "team".into();
        store(&db, &payload).await;
        assert_eq!(read(&db).await.tier(), Tier::Team);
        db.execute("UPDATE one_enterprise_license SET tier = 'free'", &[])
            .await
            .unwrap();
        assert_eq!(read(&db).await.tier(), Tier::Free);
    }
}
