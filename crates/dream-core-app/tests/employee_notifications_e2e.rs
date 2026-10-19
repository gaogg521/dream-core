//! Result delivery exercises real persistence and report files with a recording transport.
mod common;

use dream_core_api_types::{CronPushTarget, EmployeeNotificationSettings};
use dream_core_db::db_params;
use dream_domain_employee::notifications::{EmployeeNotifications, EmployeeResultNotifier};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

type SentResult = (String, String, Vec<Vec<u8>>);
#[derive(Default)]
struct Transport {
    sent: Mutex<Vec<SentResult>>,
    fail: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl EmployeeResultNotifier for Transport {
    async fn validate(&self, owner: &str, target: &CronPushTarget) -> Result<(), String> {
        if target.chat_id == owner {
            Ok(())
        } else {
            Err("recipient belongs to another user".into())
        }
    }
    async fn send(&self, owner: &str, _: &CronPushTarget, text: &str, files: &[PathBuf]) -> Result<(), String> {
        if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("platform rejected delivery".into());
        }
        let mut bytes = Vec::new();
        for file in files {
            bytes.push(tokio::fs::read(file).await.unwrap());
        }
        self.sent.lock().await.push((owner.into(), text.into(), bytes));
        Ok(())
    }
}

#[tokio::test]
async fn notification_api_rejects_unconnected_sessions_and_another_users_employee() {
    use axum::http::StatusCode;
    use common::{body_json, get_with_token, json_with_token, setup_and_login};
    use serde_json::json;
    use tower::ServiceExt;
    let (mut app, services) = common::build_app().await;
    let (alice, csrf) = setup_and_login(&mut app, &services, "alice", "qa-password-123").await;
    let response = app.clone().oneshot(json_with_token("POST", "/api/one/employee/team-sync", json!({"agents":[{"id":"notify-api-agent","name":"QA reporter","agentType":"dream","automationConfig":{"persona":"QA reporter"}}],"authoritative":true}), &alice, &csrf)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let path = "/api/one/employee/agents/notify-api-agent/notification";
    let request = axum::http::Request::builder()
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let request = axum::http::Request::builder()
        .method("PUT")
        .uri(path)
        .header("cookie", format!("dream-session={alice}"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{\"target\":null}"))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(json_with_token(
            "PUT",
            path,
            json!({"target":{"plugin_id":"telegram"}}),
            &alice,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["code"], "BAD_REQUEST");
    let response = app
        .clone()
        .oneshot(json_with_token(
            "PUT",
            path,
            json!({"target":{"plugin_id":"telegram","chat_id":"unconnected-recipient"}}),
            &alice,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app.clone().oneshot(get_with_token(path, &alice)).await.unwrap();
    assert!(body_json(response).await["data"]["target"].is_null());
    let (bob, bob_csrf) = setup_and_login(&mut app, &services, "bob", "qa-password-123").await;
    let response = app
        .oneshot(json_with_token("PUT", path, json!({"target":null}), &bob, &bob_csrf))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn result_delivery_is_scoped_persistent_and_sends_success_report_or_failure_reason_once() {
    let root = tempfile::tempdir().unwrap();
    let (_, services) = common::build_app().await;
    let db = services.db.clone();
    let notifications = EmployeeNotifications::new(
        db.clone(),
        Arc::new(services.conversation_service.clone()),
        root.path().into(),
        services.conversation_repo.clone(),
    );
    let transport = Arc::new(Transport::default());
    notifications.bind(transport.clone());
    let settings = |owner: &str| EmployeeNotificationSettings {
        target: Some(CronPushTarget {
            plugin_id: "telegram".into(),
            chat_id: owner.into(),
        }),
    };
    notifications.set("alice", "agent", settings("alice")).await.unwrap();
    assert!(notifications.get("bob", "agent").await.unwrap().target.is_none());
    assert!(
        notifications
            .set("alice", "agent", settings("bob"))
            .await
            .unwrap_err()
            .to_string()
            .contains("recipient belongs to another user")
    );
    assert_eq!(
        notifications
            .get("alice", "agent")
            .await
            .unwrap()
            .target
            .unwrap()
            .chat_id,
        "alice"
    );
    db.execute("INSERT INTO one_personal_agents(id, owner_user_id, name, agent_type, created_at, updated_at) VALUES ('agent', 'alice', 'QA reporter', 'dream', 0, 0)", &[]).await.unwrap();
    for (id, status, summary, error, trigger) in [
        ("success", "success", "Report ready", "", "manual"),
        ("failed", "failed", "", "Provider unavailable for QA", "cron"),
        ("chat", "success", "Interactive chat", "", "chat"),
        ("disabled", "success", "Other user", "", "manual"),
    ] {
        let owner = if id == "disabled" { "bob" } else { "alice" };
        db.execute("INSERT INTO one_employee_runs(id, agent_id, owner_user_id, conversation_id, status, summary, error, trigger_source, started_at) VALUES (?, 'agent', ?, 'missing-conversation', ?, ?, ?, ?, 0)", &db_params![id, owner, status, summary, error, trigger]).await.unwrap();
        notifications.deliver(id).await;
        notifications.deliver(id).await;
    }
    let sent = transport.sent.lock().await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].0, "alice");
    assert!(sent[0].1.contains("Report ready"));
    assert_eq!(sent[0].2, vec![b"# QA reporter\n\nReport ready".to_vec()]);
    assert!(sent[1].1.contains("Provider unavailable for QA"));
    assert!(sent[1].2.is_empty());
    drop(sent);
    let receipts: (i64,) = db
        .fetch_one_as(
            "SELECT COUNT(*) FROM one_employee_deliveries WHERE status = 'sent'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(receipts.0, 2);
    transport.fail.store(true, std::sync::atomic::Ordering::Relaxed);
    db.execute("INSERT INTO one_employee_runs(id, agent_id, owner_user_id, conversation_id, status, summary, trigger_source, started_at) VALUES ('delivery-failed', 'agent', 'alice', 'missing-conversation', 'success', 'Work completed', 'manual', 0)", &[]).await.unwrap();
    notifications.deliver("delivery-failed").await;
    let outcome: (String, String) = db.fetch_one_as("SELECT r.status, d.status FROM one_employee_runs r JOIN one_employee_deliveries d ON d.run_id=r.id WHERE r.id='delivery-failed'", &[]).await.unwrap();
    assert_eq!(outcome, ("success".into(), "failed".into()));
    notifications
        .set("alice", "agent", EmployeeNotificationSettings::default())
        .await
        .unwrap();
    assert!(notifications.get("alice", "agent").await.unwrap().target.is_none());
}
