//! HTTP-level tests for the secure-export routes: real router, real in-memory
//! SQLite, a `CurrentUser` per request. They cover the authorization matrix
//! the design relies on (recipient scopes, cross-company isolation, revoke,
//! expiry, rate limit, plan gate) and that the data key never shows up
//! anywhere but the open response.

use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::secure_export::{content_hash_prefix, watermark_line};
use crate::service::PlatformService;
use crate::state::OnePlatformRouterState;

struct Fixture {
    db: dream_core_db::Database,
    service: Arc<PlatformService>,
}

/// ent-1: alice, bob, admin, auditor in t1; carol in t2. ent-2: dave in t9.
/// `loner` exists as a user but belongs to no company.
async fn fixture() -> Fixture {
    let db = dream_core_db::init_database_memory().await.unwrap();
    crate::migrate::run_one_platform_migrations(&dream_core_db::DbPool::Sqlite(db.pool().clone()))
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, \
             role TEXT NOT NULL DEFAULT 'member', created_at INTEGER NOT NULL DEFAULT 0, \
             updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));\
         CREATE TABLE IF NOT EXISTS one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, \
             updated_at INTEGER NOT NULL DEFAULT 0);\
         CREATE TABLE IF NOT EXISTS one_enterprise_members (user_id TEXT PRIMARY KEY, enterprise_id TEXT NOT NULL, \
             display_name TEXT, role TEXT NOT NULL DEFAULT 'member', joined_at INTEGER, updated_at INTEGER);\
         CREATE TABLE IF NOT EXISTS one_audit_logs (id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, user_id TEXT, \
             username TEXT, action TEXT NOT NULL, resource TEXT, ip_address TEXT, user_agent TEXT, \
             created_at INTEGER NOT NULL, latency_ms INTEGER, result TEXT NOT NULL DEFAULT 'success');\
         CREATE TABLE IF NOT EXISTS one_runtime_nodes (id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, \
             user_id TEXT NOT NULL, machine_id TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'approved');",
    )
    .execute(db.pool())
    .await
    .unwrap();
    for (user, tenant, role, enterprise, name) in [
        ("alice", "t1", "member", "ent-1", "Alice"),
        ("bob", "t1", "member", "ent-1", "Bob"),
        ("admin", "t1", "org_admin", "ent-1", "Admin"),
        ("auditor", "t1", "auditor", "ent-1", "Auditor"),
        ("carol", "t2", "member", "ent-1", "Carol"),
        ("dave", "t9", "member", "ent-2", "Dave"),
    ] {
        sqlx::query("INSERT INTO one_user_org (user_id, tenant_id, role) VALUES (?, ?, ?)")
            .bind(user)
            .bind(tenant)
            .bind(role)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO one_enterprise_members (user_id, enterprise_id, display_name) VALUES (?, ?, ?)")
            .bind(user)
            .bind(enterprise)
            .bind(name)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let service = PlatformService::new(dream_core_db::DbPool::Sqlite(db.pool().clone()), [7u8; 32]);
    *service.secure_export_plan_override.lock().unwrap() = Some(true);
    Fixture {
        db,
        service: Arc::new(service),
    }
}

impl Fixture {
    async fn call(&self, user: &str, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call_from(user, method, uri, body, Some("machine-a")).await
    }

    async fn call_from(
        &self,
        user: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
        machine: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut current = dream_core_auth::CurrentUser::local_default();
        current.id = user.to_owned();
        current.username = user.to_owned();
        let app = crate::routes::one_platform_routes(OnePlatformRouterState::new(self.service.clone()))
            .layer(Extension(current));
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(m) = machine {
            req = req
                .header("x-dream-machine-id", m)
                .header("authorization", "Bearer test");
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string()))
                .unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    async fn audit_count(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM one_audit_logs WHERE action = ?")
            .bind(action)
            .fetch_one(self.db.pool())
            .await
            .unwrap()
    }
}

fn code(v: &Value) -> &str {
    v["code"].as_str().unwrap_or_default()
}

fn dek() -> String {
    BASE64.encode([0x42u8; 32])
}

fn package_body(id: &str, scope: &str, users: &[&str]) -> Value {
    json!({
        "packageId": id,
        "dek": dek(),
        "recipientScope": scope,
        "recipientUserIds": users,
        "fileNames": ["report.md"],
        "totalBytes": 123,
        "contentSha256": "ab12",
    })
}

#[tokio::test]
async fn policy_fields_round_trip_and_absent_fields_are_kept() {
    let f = fixture().await;
    let (s, v) = f
        .call("alice", "GET", "/api/one/platform/my-security-policy", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["data"]["exportWatermarkEnabled"], json!(false));
    assert_eq!(v["data"]["exportEncryptionMode"], json!("optional"));

    let (s, v) = f
        .call(
            "admin",
            "PUT",
            "/api/one/admin/platform/security-policy",
            Some(json!({"exportWatermarkEnabled": true, "exportEncryptionMode": "required"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["data"]["exportEncryptionMode"], json!("required"));

    // An older console that knows nothing about export control must not
    // reset it when it saves the other fields.
    let (s, v) = f
        .call(
            "admin",
            "PUT",
            "/api/one/admin/platform/security-policy",
            Some(json!({"terminalToolsRequireApproval": true})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["data"]["exportWatermarkEnabled"], json!(true));
    assert_eq!(v["data"]["exportEncryptionMode"], json!("required"));

    let (s, v) = f
        .call(
            "admin",
            "PUT",
            "/api/one/admin/platform/security-policy",
            Some(json!({"exportEncryptionMode": "sometimes"})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(code(&v), "BAD_REQUEST");

    // Members cannot write policy.
    let (s, _) = f
        .call(
            "alice",
            "PUT",
            "/api/one/admin/platform/security-policy",
            Some(json!({"exportEncryptionMode": "optional"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Strict preset turns both on; relaxed turns them back off.
    let (_, v) = f
        .call(
            "admin",
            "POST",
            "/api/one/admin/platform/security-policy/tier",
            Some(json!({"tier": "strict"})),
        )
        .await;
    assert_eq!(v["data"]["exportWatermarkEnabled"], json!(true));
    assert_eq!(v["data"]["exportEncryptionMode"], json!("required"));
}

#[tokio::test]
async fn template_apply_keeps_export_control() {
    let f = fixture().await;
    f.call(
        "admin",
        "PUT",
        "/api/one/admin/platform/security-policy",
        Some(json!({"exportWatermarkEnabled": true, "exportEncryptionMode": "default"})),
    )
    .await;
    let (s, v) = f
        .call(
            "admin",
            "POST",
            "/api/one/admin/platform/security-policy/templates",
            Some(json!({"name": "tpl", "tier": "relaxed"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["data"]["id"].as_str().unwrap().to_owned();
    let (s, v) = f
        .call(
            "admin",
            "POST",
            &format!("/api/one/admin/platform/security-policy/templates/{id}/apply"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["data"]["exportWatermarkEnabled"], json!(true));
    assert_eq!(v["data"]["exportEncryptionMode"], json!("default"));
}

#[tokio::test]
async fn seeds_are_per_machine_and_gated() {
    let f = fixture().await;
    let uri = "/api/one/platform/secure-export/watermark-seed";
    let (s, a1) = f.call_from("alice", "POST", uri, None, Some("m1")).await;
    assert_eq!(s, StatusCode::OK, "{a1}");
    let (_, a2) = f.call_from("alice", "POST", uri, None, Some("m1")).await;
    let (_, b1) = f.call_from("alice", "POST", uri, None, Some("m2")).await;
    assert_eq!(
        a1["data"]["seedId"], a2["data"]["seedId"],
        "same machine reuses its seed"
    );
    assert_ne!(
        a1["data"]["seedId"], b1["data"]["seedId"],
        "another machine gets its own"
    );
    let key = BASE64.decode(a1["data"]["seedKey"].as_str().unwrap()).unwrap();
    assert_eq!(key.len(), 32);
    // The stored key is encrypted, never the plaintext.
    let stored: String = sqlx::query_scalar("SELECT key_enc FROM one_watermark_seeds WHERE id = ?")
        .bind(a1["data"]["seedId"].as_str().unwrap())
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert!(stored.starts_with("encv1:"));
    assert!(!stored.contains(a1["data"]["seedKey"].as_str().unwrap()));

    let (s, v) = f.call("loner", "POST", uri, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(code(&v), "NOT_IN_ENTERPRISE");

    *f.service.secure_export_plan_override.lock().unwrap() = Some(false);
    let (s, v) = f.call("alice", "POST", uri, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(code(&v), "PLAN_FEATURE_REQUIRED");
}

#[tokio::test]
async fn events_are_validated_deduplicated_and_audited() {
    let f = fixture().await;
    let uri = "/api/one/platform/secure-export/events";
    let event = json!({"eventId": "ev-1", "kind": "conversation_markdown", "fileNames": ["a.md"],
        "totalBytes": 10, "contentSha256": "abcd", "clientTs": 1000});
    let (s, v) = f
        .call("alice", "POST", uri, Some(json!({"events": [event.clone()]})))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["data"]["accepted"], json!(1));
    // A retried upload stores nothing twice.
    let (_, v) = f.call("alice", "POST", uri, Some(json!({"events": [event]}))).await;
    assert_eq!(v["data"]["accepted"], json!(0));
    assert_eq!(f.audit_count("secure_export.export").await, 1);

    let clip = json!({"eventId": "ev-2", "kind": "clipboard", "clientTs": 1001});
    f.call("alice", "POST", uri, Some(json!({"events": [clip]}))).await;
    assert_eq!(f.audit_count("secure_export.clipboard.copy").await, 1);

    let bad = json!({"eventId": "ev-3", "kind": "teleport", "clientTs": 1});
    let (s, v) = f.call("alice", "POST", uri, Some(json!({"events": [bad]}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap_or_default().contains("teleport") || v.to_string().contains("teleport"));

    let (s, v) = f
        .call("auditor", "GET", "/api/one/admin/platform/secure-export/events", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["data"].as_array().unwrap().len(), 2);
    assert_eq!(v["data"][1]["userName"], json!("Alice"));
    assert_eq!(v["data"][1]["machineId"], json!("machine-a"));

    let (s, _) = f
        .call("alice", "GET", "/api/one/admin/platform/secure-export/events", None)
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn package_recipients_are_enforced() {
    let f = fixture().await;
    let create = "/api/one/platform/secure-export/packages";
    let (s, v) = f
        .call("alice", "POST", create, Some(package_body("p-tenant", "tenant", &[])))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["data"]["ownerName"], json!("Alice"));
    assert!(v["data"].get("dek").is_none(), "create must not echo the key");
    let stored: String = sqlx::query_scalar("SELECT dek_enc FROM one_secure_export_packages WHERE id = 'p-tenant'")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert!(stored.starts_with("encv1:") && !stored.contains(&dek()));

    let open = |id: &str| format!("/api/one/platform/secure-export/packages/{id}/open");
    // Owner and a tenant-mate get the key back.
    for user in ["alice", "bob"] {
        let (s, v) = f.call(user, "POST", &open("p-tenant"), None).await;
        assert_eq!(s, StatusCode::OK, "{user}: {v}");
        assert_eq!(v["data"]["dek"], json!(dek()));
    }
    // Same company, other tenant: refused and recorded.
    let (s, v) = f.call("carol", "POST", &open("p-tenant"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(f.audit_count("secure_export.package.open_denied").await, 1);
    // Another company: the package does not exist for them.
    let (s, v) = f.call("dave", "POST", &open("p-tenant"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(code(&v), "NOT_FOUND");

    // Enterprise scope reaches carol; named users reach only those named.
    f.call("alice", "POST", create, Some(package_body("p-ent", "enterprise", &[])))
        .await;
    let (s, _) = f.call("carol", "POST", &open("p-ent"), None).await;
    assert_eq!(s, StatusCode::OK);
    f.call(
        "alice",
        "POST",
        create,
        Some(package_body("p-users", "users", &["carol"])),
    )
    .await;
    let (s, _) = f.call("carol", "POST", &open("p-users"), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = f.call("bob", "POST", &open("p-users"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Naming someone outside the company is refused up front.
    let (s, v) = f
        .call("alice", "POST", create, Some(package_body("p-bad", "users", &["dave"])))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    // A reused id cannot swap the key behind an existing package.
    let (s, _) = f
        .call("bob", "POST", create, Some(package_body("p-tenant", "tenant", &[])))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let mut short = package_body("p-short", "tenant", &[]);
    short["dek"] = json!(BASE64.encode([1u8; 16]));
    let (s, _) = f.call("alice", "POST", create, Some(short)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Open counts are visible to the owner and the admin, keys to neither.
    let (s, v) = f.call("alice", "GET", create, None).await;
    assert_eq!(s, StatusCode::OK);
    let mine = v["data"].as_array().unwrap();
    assert_eq!(mine.len(), 3);
    assert!(!v.to_string().contains(&dek()));
    let tenant_pkg = mine.iter().find(|p| p["id"] == json!("p-tenant")).unwrap();
    assert_eq!(tenant_pkg["openCount"], json!(2));
    let (s, v) = f
        .call("admin", "GET", "/api/one/admin/platform/secure-export/packages", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(!v.to_string().contains(&dek()));
    let (s, v) = f
        .call(
            "admin",
            "GET",
            "/api/one/admin/platform/secure-export/packages/p-tenant/opens",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["data"].as_array().unwrap().len(), 3, "two grants and one denial");
}

#[tokio::test]
async fn revoke_and_expiry_stop_opening() {
    let f = fixture().await;
    let create = "/api/one/platform/secure-export/packages";
    f.call("alice", "POST", create, Some(package_body("p1", "tenant", &[])))
        .await;

    // Only the owner (or an admin) may revoke; others do not even see it.
    let (s, _) = f
        .call(
            "bob",
            "POST",
            "/api/one/platform/secure-export/packages/p1/revoke",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, v) = f
        .call(
            "alice",
            "POST",
            "/api/one/platform/secure-export/packages/p1/revoke",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["data"]["status"], json!("revoked"));
    let (s, v) = f
        .call("bob", "POST", "/api/one/platform/secure-export/packages/p1/open", None)
        .await;
    assert_eq!(s, StatusCode::GONE);
    assert_eq!(code(&v), "PACKAGE_REVOKED");

    f.call("alice", "POST", create, Some(package_body("p2", "tenant", &[])))
        .await;
    sqlx::query("UPDATE one_secure_export_packages SET expires_at = 1 WHERE id = 'p2'")
        .execute(f.db.pool())
        .await
        .unwrap();
    let (s, v) = f
        .call(
            "alice",
            "POST",
            "/api/one/platform/secure-export/packages/p2/open",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::GONE);
    assert_eq!(code(&v), "PACKAGE_EXPIRED");

    // An admin can revoke any package in the tenant.
    f.call("alice", "POST", create, Some(package_body("p3", "tenant", &[])))
        .await;
    let (s, v) = f
        .call(
            "admin",
            "POST",
            "/api/one/admin/platform/secure-export/packages/p3/revoke",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["data"]["revokedBy"], json!("admin"));
    // The auditor reads but never revokes.
    let (s, _) = f
        .call(
            "auditor",
            "POST",
            "/api/one/admin/platform/secure-export/packages/p3/revoke",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn open_attempts_are_rate_limited() {
    let f = fixture().await;
    f.call(
        "alice",
        "POST",
        "/api/one/platform/secure-export/packages",
        Some(package_body("p1", "tenant", &[])),
    )
    .await;
    for _ in 0..30 {
        let (s, _) = f
            .call("bob", "POST", "/api/one/platform/secure-export/packages/p1/open", None)
            .await;
        assert_eq!(s, StatusCode::OK);
    }
    let (s, v) = f
        .call("bob", "POST", "/api/one/platform/secure-export/packages/p1/open", None)
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(code(&v), "RATE_LIMITED");
}

#[tokio::test]
async fn watermark_lookup_identifies_and_detects_tampering() {
    let f = fixture().await;
    let (_, seed) = f
        .call_from(
            "alice",
            "POST",
            "/api/one/platform/secure-export/watermark-seed",
            None,
            Some("laptop-7"),
        )
        .await;
    let seed_id = seed["data"]["seedId"].as_str().unwrap().to_owned();
    let key = BASE64.decode(seed["data"]["seedKey"].as_str().unwrap()).unwrap();
    let body = "# Q3 plan\n\nRevenue target: 12M.\n";
    let ts: u64 = 1_760_000_000;
    let doc = format!("{body}\n{}\n", watermark_line(&key, &seed_id, ts, body));
    // The matching export event, reported by the client.
    f.call_from(
        "alice",
        "POST",
        "/api/one/platform/secure-export/events",
        Some(
            json!({"events": [{"eventId": "ev-wm", "kind": "conversation_markdown", "seedId": seed_id,
            "fileNames": ["q3.md"], "contentSha256": "00", "clientTs": ts as i64 * 1000 + 300}]}),
        ),
        Some("laptop-7"),
    )
    .await;

    let verify = "/api/one/admin/platform/secure-export/watermark/verify";
    let (s, v) = f.call("auditor", "POST", verify, Some(json!({"text": doc}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let m = &v["data"]["marks"][0];
    assert_eq!(m["found"], json!(true));
    assert_eq!(m["macValid"], json!(true));
    assert_eq!(m["contentMatches"], json!(true));
    assert_eq!(m["userName"], json!("Alice"));
    assert_eq!(m["machineId"], json!("laptop-7"));
    assert_eq!(m["exportedAt"], json!(ts as i64 * 1000));
    assert_eq!(m["exportEvent"]["fileNames"], json!(["q3.md"]));
    assert_eq!(v["data"]["contentHash"], json!(content_hash_prefix(body)));
    assert_eq!(f.audit_count("secure_export.watermark.verify").await, 1);

    // Edited after export: the signature holds, the content does not.
    let edited = doc.replace("12M", "20M");
    let (_, v) = f.call("admin", "POST", verify, Some(json!({"text": edited}))).await;
    assert_eq!(v["data"]["marks"][0]["macValid"], json!(true));
    assert_eq!(v["data"]["marks"][0]["contentMatches"], json!(false));

    // A forged signature is reported as such.
    let line = watermark_line(&key, &seed_id, ts, body);
    let forged_line = format!("{}0000000000000000 -->", &line[..line.len() - 20]);
    let forged = format!("{body}\n{forged_line}\n");
    let (_, v) = f.call("admin", "POST", verify, Some(json!({"text": forged}))).await;
    assert_eq!(v["data"]["marks"][0]["macValid"], json!(false));

    // Another tenant's admin cannot resolve alice's seed.
    let (_, carol_seed) = f
        .call("carol", "POST", "/api/one/platform/secure-export/watermark-seed", None)
        .await;
    let carol_key = BASE64.decode(carol_seed["data"]["seedKey"].as_str().unwrap()).unwrap();
    let foreign = format!(
        "{body}\n{}\n",
        watermark_line(&carol_key, carol_seed["data"]["seedId"].as_str().unwrap(), ts, body)
    );
    let (_, v) = f.call("admin", "POST", verify, Some(json!({"text": foreign}))).await;
    assert_eq!(v["data"]["marks"][0]["found"], json!(false));
    assert!(v["data"]["marks"][0]["userName"].is_null());

    // Members cannot look marks up.
    let (s, _) = f.call("alice", "POST", verify, Some(json!({"text": doc}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
