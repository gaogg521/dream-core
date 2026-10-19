//! Account-state reconciliation for already-bound LDAP identities.
//!
//! This is deliberately not a partial snapshot of the company directory:
//! writing one into the Feishu mirror would mark unrelated people as departed.
//! Only a confirmed AD ACCOUNTDISABLE bit freezes identity admission. Errors,
//! missing entries and ambiguous matches preserve the last known state.

use crate::providers::ldap::{LdapProviderConfig, connect, escape_filter_value, resolve_bind_principal};
use crate::{SsoError, SsoService, models::SsoProviderKind};
use async_trait::async_trait;
use dream_core_db::{DbBackend, DbPool, db_params};
use ldap3::{Scope, SearchEntry};
use serde::Serialize;
use std::{sync::Arc, time::Duration};

/// Wired by the app layer. No membership, seat or resource removal is allowed.
#[async_trait]
pub trait LdapSessionRevoker: Send + Sync {
    async fn revoke(&self, user_id: &str) -> Result<(), String>;
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LdapStatusReport {
    pub checked: usize,
    pub disabled: usize,
    pub restored: usize,
    pub unknown: usize,
    pub pending_revocations: usize,
    pub skipped: bool,
}

pub async fn identity_allowed(db: &DbPool, user_id: &str) -> Result<bool, SsoError> {
    let row = db
        .fetch_optional_as::<(i64,)>(
            "SELECT COUNT(*) FROM one_sso_directory_identity_state s \
         JOIN one_sso_identities i ON i.id = s.identity_id AND i.user_id = s.user_id \
         WHERE s.user_id = ? AND i.provider = 'ldap' AND s.source_disabled = 1",
            &db_params![user_id],
        )
        .await?;
    Ok(row.is_some_and(|row| row.0 == 0))
}

#[async_trait]
impl dream_core_auth::middleware::DirectoryIdentityGate for SsoService {
    async fn is_allowed(&self, user_id: &str) -> Result<bool, String> {
        identity_allowed(&self.db, user_id).await.map_err(|e| e.to_string())
    }
}

/// A missing UAC value is unknown, never evidence that an account is enabled.
fn disabled_from_entry(entry: &SearchEntry) -> Option<bool> {
    entry
        .attrs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("userAccountControl"))?
        .1
        .first()?
        .parse::<u64>()
        .ok()
        .map(|flags| flags & 2 != 0)
}

fn lookup_target(config: &LdapProviderConfig, external_id: &str) -> Option<(String, Scope, String)> {
    let base = config.base_dn.trim();
    if external_id.to_lowercase().ends_with(&base.to_lowercase()) && external_id.contains('=') {
        return Some((external_id.to_owned(), Scope::Base, "(objectClass=*)".into()));
    }
    let attr = config.external_id_attribute.trim();
    if attr.is_empty()
        || !attr
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.')
    {
        return None;
    }
    Some((
        base.to_owned(),
        Scope::Subtree,
        format!("({attr}={})", escape_filter_value(external_id)),
    ))
}

async fn record_unknown(db: &DbPool, identity_id: &str, now: i64) -> Result<(), SsoError> {
    db.execute("UPDATE one_sso_directory_identity_state SET last_checked_at = ?, last_error = 'directory_lookup_unknown' WHERE identity_id = ?",
        &db_params![now, identity_id]).await?;
    Ok(())
}

async fn reconcile(
    db: &DbPool,
    revoker: &dyn LdapSessionRevoker,
    identity_id: &str,
    user_id: &str,
    disabled: bool,
    now: i64,
    report: &mut LdapStatusReport,
) -> Result<(), SsoError> {
    let sql = match db.backend() {
        DbBackend::Sqlite => {
            "INSERT INTO one_sso_directory_identity_state (identity_id,user_id,last_checked_at) VALUES (?,?,?) ON CONFLICT(identity_id) DO NOTHING"
        }
        DbBackend::MySql => {
            "INSERT IGNORE INTO one_sso_directory_identity_state (identity_id,user_id,last_checked_at) VALUES (?,?,?)"
        }
    };
    db.execute(sql, &db_params![identity_id, user_id, now]).await?;
    if disabled {
        let changed = db.execute("UPDATE one_sso_directory_identity_state SET source_disabled = 1, revocation_pending = 1 WHERE identity_id = ? AND source_disabled = 0",
            &db_params![identity_id]).await?;
        report.disabled += changed as usize;
        if changed > 0 {
            tracing::warn!(user_id, "LDAP source account disabled; identity admission frozen");
        }
    }
    let pending = db
        .fetch_optional_as::<(i64,)>(
            "SELECT revocation_pending FROM one_sso_directory_identity_state WHERE identity_id = ?",
            &db_params![identity_id],
        )
        .await?
        .is_some_and(|row| row.0 != 0);
    // Finish a pending revocation before unfreezing a restored source account:
    // otherwise its old JWT could become usable again after a failed revoke.
    if pending {
        if let Err(error) = revoker.revoke(user_id).await {
            report.pending_revocations += 1;
            db.execute("UPDATE one_sso_directory_identity_state SET last_checked_at = ?, last_error = 'revocation_pending' WHERE identity_id = ?",
                &db_params![now, identity_id]).await?;
            tracing::error!(user_id, %error, "LDAP credential revocation pending; identity remains frozen");
            return Ok(());
        }
        db.execute(
            "UPDATE one_sso_directory_identity_state SET revocation_pending = 0 WHERE identity_id = ?",
            &db_params![identity_id],
        )
        .await?;
    }
    if !disabled {
        let changed = db.execute("UPDATE one_sso_directory_identity_state SET source_disabled = 0 WHERE identity_id = ? AND source_disabled = 1",
            &db_params![identity_id]).await?;
        report.restored += changed as usize;
        if changed > 0 {
            tracing::info!(user_id, "LDAP source account restored; fresh authentication allowed");
        }
    }
    db.execute(
        "UPDATE one_sso_directory_identity_state SET last_checked_at = ?, last_error = NULL WHERE identity_id = ?",
        &db_params![now, identity_id],
    )
    .await?;
    Ok(())
}

pub async fn sync_bound_identities(
    sso: &SsoService,
    revoker: &dyn LdapSessionRevoker,
) -> Result<LdapStatusReport, SsoError> {
    let mut report = LdapStatusReport::default();
    let Some(row) = sso
        .get_provider_row(SsoProviderKind::Ldap)
        .await?
        .filter(|row| row.enabled)
    else {
        report.skipped = true;
        return Ok(report);
    };
    let config: LdapProviderConfig =
        serde_json::from_str(&row.config).map_err(|_| SsoError::ProviderNotConfigured("ldap".into()))?;
    let principal = resolve_bind_principal(&config);
    if principal.is_empty() || config.bind_password.is_empty() || config.base_dn.trim().is_empty() {
        return Err(SsoError::ProviderUnavailable);
    }
    let identities = sso
        .db
        .fetch_all_as::<(String, String, String)>(
            "SELECT id,user_id,external_id FROM one_sso_identities WHERE provider = 'ldap'",
            &[],
        )
        .await?;
    if identities.is_empty() {
        return Ok(report);
    }
    let timeout = Duration::from_millis(config.timeout_ms.unwrap_or(10_000).clamp(1000, 30_000));
    let mut ldap = connect(&config).await?;
    let bound = tokio::time::timeout(timeout, ldap.simple_bind(&principal, &config.bind_password))
        .await
        .map_err(|_| SsoError::ProviderUnavailable)?
        .map_err(|_| SsoError::ProviderUnavailable)?;
    if bound.rc != 0 {
        let _ = ldap.unbind().await;
        return Err(SsoError::ProviderUnavailable);
    }
    for (identity_id, user_id, external_id) in identities {
        let now = dream_core_common::now_ms() as i64;
        let Some((base, scope, filter)) = lookup_target(&config, &external_id) else {
            report.unknown += 1;
            record_unknown(&sso.db, &identity_id, now).await?;
            continue;
        };
        let found = tokio::time::timeout(timeout, ldap.search(&base, scope, &filter, vec!["userAccountControl"])).await;
        let disabled = match found {
            Ok(Ok(result)) => match result.success() {
                Ok((mut entries, _)) if entries.len() == 1 => {
                    disabled_from_entry(&SearchEntry::construct(entries.remove(0)))
                }
                _ => None,
            },
            _ => None,
        };
        let Some(disabled) = disabled else {
            report.unknown += 1;
            record_unknown(&sso.db, &identity_id, now).await?;
            continue;
        };
        report.checked += 1;
        reconcile(&sso.db, revoker, &identity_id, &user_id, disabled, now, &mut report).await?;
    }
    let _ = ldap.unbind().await;
    Ok(report)
}

pub fn start_scheduler(
    sso: Arc<SsoService>,
    revoker: Arc<dyn LdapSessionRevoker>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(30)) => {},
                _ = shutdown.changed() => return,
            }
            let result = tokio::select! {
                result = sync_bound_identities(&sso, revoker.as_ref()) => result,
                _ = shutdown.changed() => return,
            };
            match result {
                Ok(report) if report.disabled + report.restored + report.unknown + report.pending_revocations > 0 => {
                    tracing::info!(
                        checked = report.checked,
                        disabled = report.disabled,
                        restored = report.restored,
                        unknown = report.unknown,
                        pending = report.pending_revocations,
                        "LDAP identity-state scan completed"
                    )
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "LDAP identity-state scan failed; previous account restrictions retained")
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Revoker {
        fail: AtomicBool,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl LdapSessionRevoker for Revoker {
        async fn revoke(&self, _: &str) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                Err("test failure".into())
            } else {
                Ok(())
            }
        }
    }
    async fn fixture() -> (DbPool, Revoker) {
        let sqlite = dream_core_db::init_database_memory().await.unwrap();
        let db = DbPool::Sqlite(sqlite.pool().clone());
        crate::migrate::run_one_sso_migrations(&db).await.unwrap();
        db.execute("INSERT INTO one_sso_identities (id,provider,external_id,user_id,created_at) VALUES ('ldap-id','ldap','test','member',0)", &[]).await.unwrap();
        (
            db,
            Revoker {
                fail: AtomicBool::new(false),
                calls: AtomicUsize::new(0),
            },
        )
    }
    #[test]
    fn only_account_disable_bit_blocks() {
        for (flags, expected) in [
            ("66048", Some(false)),
            ("66050", Some(true)),
            ("16", Some(false)),
            ("invalid", None),
        ] {
            let entry = SearchEntry {
                dn: "test".into(),
                attrs: [("USERACCOUNTCONTROL".into(), vec![flags.into()])].into(),
                bin_attrs: Default::default(),
            };
            assert_eq!(disabled_from_entry(&entry), expected);
        }
        let entry = SearchEntry {
            dn: "test".into(),
            attrs: Default::default(),
            bin_attrs: Default::default(),
        };
        assert_eq!(disabled_from_entry(&entry), None);
    }
    #[test]
    fn lookup_uses_bound_dn_or_escaped_configured_attribute() {
        let mut config = LdapProviderConfig {
            base_dn: "DC=example,DC=com".into(),
            ..Default::default()
        };
        let (base, scope, filter) = lookup_target(&config, "CN=test,DC=example,DC=com").unwrap();
        assert_eq!(base, "CN=test,DC=example,DC=com");
        assert!(matches!(scope, Scope::Base));
        assert_eq!(filter, "(objectClass=*)");
        assert!(lookup_target(&config, "unknown-user").is_none());
        config.external_id_attribute = "sAMAccountName".into();
        let (_, scope, filter) = lookup_target(&config, "test*)(objectClass=*)").unwrap();
        assert!(matches!(scope, Scope::Subtree));
        assert!(!filter.contains("*)(objectClass=*"));
        config.external_id_attribute = "bad)(attribute".into();
        assert!(lookup_target(&config, "test").is_none());
    }
    #[tokio::test]
    async fn disabled_restored_and_unlinked_identity_admission() {
        let (db, revoker) = fixture().await;
        let mut report = LdapStatusReport::default();
        assert!(identity_allowed(&db, "member").await.unwrap());
        reconcile(&db, &revoker, "ldap-id", "member", true, 1, &mut report)
            .await
            .unwrap();
        assert!(!identity_allowed(&db, "member").await.unwrap());
        reconcile(&db, &revoker, "ldap-id", "member", true, 2, &mut report)
            .await
            .unwrap();
        assert_eq!(revoker.calls.load(Ordering::SeqCst), 1);
        record_unknown(&db, "ldap-id", 3).await.unwrap();
        assert!(!identity_allowed(&db, "member").await.unwrap());
        reconcile(&db, &revoker, "ldap-id", "member", false, 4, &mut report)
            .await
            .unwrap();
        assert!(identity_allowed(&db, "member").await.unwrap());
        assert_eq!((report.disabled, report.restored), (1, 1));
        reconcile(&db, &revoker, "ldap-id", "member", true, 5, &mut report)
            .await
            .unwrap();
        db.execute("DELETE FROM one_sso_identities WHERE id = 'ldap-id'", &[])
            .await
            .unwrap();
        assert!(identity_allowed(&db, "member").await.unwrap());
    }
    #[tokio::test]
    async fn restoration_stays_blocked_until_failed_revocation_is_retried() {
        let (db, revoker) = fixture().await;
        revoker.fail.store(true, Ordering::SeqCst);
        let mut report = LdapStatusReport::default();
        reconcile(&db, &revoker, "ldap-id", "member", true, 1, &mut report)
            .await
            .unwrap();
        reconcile(&db, &revoker, "ldap-id", "member", false, 2, &mut report)
            .await
            .unwrap();
        assert!(!identity_allowed(&db, "member").await.unwrap());
        assert_eq!(report.pending_revocations, 2);
        revoker.fail.store(false, Ordering::SeqCst);
        reconcile(&db, &revoker, "ldap-id", "member", false, 3, &mut report)
            .await
            .unwrap();
        assert!(identity_allowed(&db, "member").await.unwrap());
        assert_eq!(report.restored, 1);
        let identities = db
            .fetch_optional_as::<(i64,)>("SELECT COUNT(*) FROM one_sso_identities", &[])
            .await
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(identities, 1, "lifecycle must retain the bound identity");
    }
}
