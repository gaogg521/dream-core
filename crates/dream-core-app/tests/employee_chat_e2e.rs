//! End-to-end HTTP test for opening a chat with a digital employee.
//!
//! A company-distributed employee reaches a member's machine through the
//! team-sync channel; the member then opens a conversation with it. The chat
//! entry must bind that conversation to the employee server-side (the run row
//! the runtime trusts) without sending anything on the member's behalf.

mod common;

use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

use common::{body_json, build_app, get_with_token, json_with_token, setup_and_login};

const AGENT_ID: &str = "pa_team_chat";

async fn sync_team_employee(app: &axum::Router, token: &str, csrf: &str) {
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/one/employee/team-sync",
            json!({
                "agents": [{
                    "id": AGENT_ID,
                    "name": "Weekly reporter",
                    "description": "Drafts the weekly report",
                    "agentType": "dream",
                    "automationConfig": {
                        "persona": "You draft weekly reports.",
                        "starterPrompts": ["Draft this week's report"]
                    }
                }],
                "authoritative": true
            }),
            token,
            csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "team-sync materializes the employee");
}

#[tokio::test]
async fn chat_with_a_team_employee_binds_a_fresh_conversation() {
    let (mut app, services) = build_app().await;
    let (token, csrf) = setup_and_login(&mut app, &services, "member", "pw12345678").await;
    sync_team_employee(&app, &token, &csrf).await;

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            &format!("/api/one/employee/agents/{AGENT_ID}/chat"),
            json!({}),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    let status = resp.status();
    let body = body_json(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let conversation_id = body["data"]["conversationId"].as_str().unwrap().to_owned();
    let run_id = body["data"]["runId"].as_str().unwrap().to_owned();

    // The conversation exists, carries the employee identity, and nothing was
    // sent into it: the member's first message is the task.
    let resp = app
        .clone()
        .oneshot(get_with_token(&format!("/api/conversations/{conversation_id}"), &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let conversation = body_json(resp).await;
    assert_eq!(conversation["data"]["extra"]["one_employee_id"], AGENT_ID);
    let resp = app
        .clone()
        .oneshot(get_with_token(
            &format!("/api/conversations/{conversation_id}/messages"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let messages = body_json(resp).await;
    let count = messages["data"]["items"]
        .as_array()
        .or_else(|| messages["data"].as_array())
        .map_or(0, Vec::len);
    assert_eq!(count, 0, "opening a chat sends nothing: {messages}");

    // The binding row is recorded as a chat, settled — never left "running".
    let resp = app
        .clone()
        .oneshot(get_with_token(
            &format!("/api/one/employee/agents/{AGENT_ID}/runs"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let runs = body_json(resp).await;
    let run = &runs["data"][0];
    assert_eq!(run["id"], run_id);
    assert_eq!(run["conversationId"], conversation_id);
    assert_eq!(run["status"], "success");
    assert_eq!(run["triggerSource"], "chat");
    assert!(
        run["finishedAt"].is_i64(),
        "a chat binding is settled at creation: {run}"
    );
}

#[tokio::test]
async fn chat_is_refused_for_an_employee_the_caller_cannot_use() {
    let (mut app, services) = build_app().await;
    let (owner_token, owner_csrf) = setup_and_login(&mut app, &services, "owner", "pw12345678").await;
    sync_team_employee(&app, &owner_token, &owner_csrf).await;
    let (other_token, other_csrf) = setup_and_login(&mut app, &services, "other", "pw12345678").await;

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            &format!("/api/one/employee/agents/{AGENT_ID}/chat"),
            json!({}),
            &other_token,
            &other_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "another member's employee is not visible"
    );

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "POST",
            "/api/one/employee/agents/pa_missing/chat",
            json!({}),
            &owner_token,
            &owner_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn chat_requires_authentication() {
    let (app, _services) = build_app().await;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/one/employee/agents/{AGENT_ID}/chat"))
        .header("content-type", "application/json")
        .header("x-csrf-token", "csrf-test")
        .header("cookie", "dream-csrf-token=csrf-test")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(resp).await["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn a_company_employee_can_be_scheduled_with_a_task_that_survives_resync() {
    let (mut app, services) = build_app().await;
    let (token, csrf) = setup_and_login(&mut app, &services, "member", "pw12345678").await;
    sync_team_employee(&app, &token, &csrf).await;

    let resp = app
        .clone()
        .oneshot(json_with_token(
            "PUT",
            &format!("/api/one/employee/agents/{AGENT_ID}/schedule"),
            json!({
                "schedule": { "kind": "every", "every_ms": 3_600_000 },
                "enabled": true,
                "task": "  汇总本周发布风险  "
            }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    let status = resp.status();
    let body = body_json(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["scheduleEnabled"], true);
    assert_eq!(
        body["data"]["scheduleTask"], "汇总本周发布风险",
        "the task is stored trimmed"
    );

    // The company changes the employee; the member's schedule and task stay.
    sync_team_employee(&app, &token, &csrf).await;
    let resp = app
        .clone()
        .oneshot(get_with_token(&format!("/api/one/employee/agents/{AGENT_ID}"), &token))
        .await
        .unwrap();
    let agent = body_json(resp).await;
    assert_eq!(agent["data"]["scheduleEnabled"], true);
    assert_eq!(agent["data"]["scheduleTask"], "汇总本周发布风险");

    // Writing the schedule again without a task clears it.
    let resp = app
        .clone()
        .oneshot(json_with_token(
            "PUT",
            &format!("/api/one/employee/agents/{AGENT_ID}/schedule"),
            json!({ "schedule": { "kind": "every", "every_ms": 3_600_000 }, "enabled": true }),
            &token,
            &csrf,
        ))
        .await
        .unwrap();
    assert!(body_json(resp).await["data"]["scheduleTask"].is_null());
}
