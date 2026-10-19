use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use axum::{Json, Router, body::to_bytes, extract::Request, routing::any};
use dream_core_db::{IChannelRepository, SqliteChannelRepository, models::ChannelPluginRow};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};

use super::{plugin::WeixinPlugin, state::WeixinState, types::WeixinRawMessage};
use crate::{
    error::ChannelError,
    plugin::{ChannelPlugin, PluginCallbacks},
    plugin_state::{EncryptedPluginStateStore, PluginStateStore},
    types::{PluginConfig, UnifiedOutgoingMessage},
};

const OWNER: &str = "system_default_user";
const KEY: [u8; 32] = [42; 32];

async fn register(repo: &dyn IChannelRepository) {
    repo.upsert_plugin(
        OWNER,
        &ChannelPluginRow {
            id: "weixin".into(),
            owner_user_id: OWNER.into(),
            r#type: "weixin".into(),
            name: "Test".into(),
            enabled: true,
            config: "fixture".into(),
            status: None,
            last_connected: None,
            created_at: 0,
            updated_at: 0,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn receives_then_reopens_database_and_sends_without_new_incoming_message() {
    let requests = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let received_once = Arc::new(AtomicBool::new(false));
    let captured = requests.clone();
    let app = Router::new().fallback(any(move |request:Request| {
        let captured = captured.clone();
        let once = received_once.clone();
        async move {
            let path = request.uri().path().to_owned();
            let body:Value = serde_json::from_slice(&to_bytes(request.into_body(), 65536).await.unwrap()).unwrap();
            captured.lock().await.push((path.clone(),body));
            if path.ends_with("getupdates") {
                if !once.swap(true, Ordering::SeqCst) {
                    Json(json!({"ret":0,"get_updates_buf":"saved-cursor","msgs":[{"from_user_id":"chat-a","msg_id":"first","context_token":"private-context","item_list":[{"type":1,"text_item":{"text":"first incoming"}}]}]}))
                } else {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Json(json!({"ret":0,"get_updates_buf":"saved-cursor","msgs":[]}))
                }
            } else { Json(json!({"ret":0})) }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let path = std::env::temp_dir().join(format!("weixin-state-{}.db", dream_core_common::generate_id()));
    let db = dream_core_db::init_database(&path).await.unwrap();
    let repo: Arc<dyn IChannelRepository> = Arc::new(SqliteChannelRepository::new(db.pool().clone()));
    register(repo.as_ref()).await;
    let store = Arc::new(EncryptedPluginStateStore::new(repo.clone(), OWNER, "weixin", KEY));
    let config: PluginConfig =
        serde_json::from_value(json!({"credentials":{"bot_token":"fixture","account_id":"account-a","baseUrl":base}}))
            .unwrap();
    let (message_tx, mut messages) = mpsc::channel(8);
    let (confirm_tx, _) = mpsc::channel(8);
    let mut first = WeixinPlugin::new();
    first.set_state_store(store.clone());
    first
        .initialize(config.clone(), PluginCallbacks { message_tx, confirm_tx })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), messages.recv())
        .await
        .unwrap()
        .unwrap();
    first.stop().await.unwrap();
    drop(first);
    let encrypted = repo
        .get_plugin_state(OWNER, "weixin", "account-a")
        .await
        .unwrap()
        .unwrap();
    assert!(!encrypted.contains("private-context"));
    assert!(!encrypted.contains("saved-cursor"));
    assert!(
        repo.get_plugin_state("another-owner", "weixin", "account-a")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repo.get_plugin_state(OWNER, "telegram", "account-a")
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.load("another-account").await.unwrap().is_none());
    let wrong_key = EncryptedPluginStateStore::new(repo.clone(), OWNER, "weixin", [43; 32]);
    assert!(matches!(
        wrong_key.load("account-a").await,
        Err(ChannelError::DecryptionFailed(_))
    ));
    drop(wrong_key);
    drop(store);
    drop(repo);
    db.close().await;
    drop(db);

    let reopened = dream_core_db::init_database(&path).await.unwrap();
    let repo: Arc<dyn IChannelRepository> = Arc::new(SqliteChannelRepository::new(reopened.pool().clone()));
    let store = Arc::new(EncryptedPluginStateStore::new(repo.clone(), OWNER, "weixin", KEY));
    let (message_tx, mut messages) = mpsc::channel(8);
    let (confirm_tx, _) = mpsc::channel(8);
    let mut second = WeixinPlugin::new();
    second.set_state_store(store.clone());
    let restart_boundary = requests.lock().await.len();
    second
        .initialize(config, PluginCallbacks { message_tx, confirm_tx })
        .await
        .unwrap();
    let message: UnifiedOutgoingMessage =
        serde_json::from_value(json!({"type":"text","text":"after restart"})).unwrap();
    second.send_message("chat-a", message).await.unwrap();
    assert!(
        matches!(messages.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "the new plugin must not receive a fresh context before sending"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if requests
                .lock()
                .await
                .iter()
                .skip(restart_boundary)
                .any(|(path, _)| path.ends_with("getupdates"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let requests = requests.lock().await;
    let sent = requests.iter().find(|(path, _)| path.ends_with("sendmessage")).unwrap();
    assert_eq!(sent.1["msg"]["context_token"], "private-context");
    assert_eq!(sent.1["msg"]["to_user_id"], "chat-a");
    assert!(
        requests
            .iter()
            .skip(restart_boundary)
            .filter(|(path, _)| path.ends_with("getupdates"))
            .all(|(_, body)| body["get_updates_buf"] == "saved-cursor")
    );
    drop(requests);
    tokio::time::timeout(Duration::from_secs(1), second.stop())
        .await
        .unwrap()
        .unwrap();
    drop(second);
    repo.delete_plugin(OWNER, "weixin").await.unwrap();
    assert!(
        store.load("account-a").await.unwrap().is_none(),
        "unlink removes private state"
    );
    drop(store);
    drop(repo);
    reopened.close().await;
    drop(reopened);
    server.abort();
    // SQLx's SQLite worker may release its final Windows file handle just after close().
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match std::fs::remove_file(&path) {
                Ok(()) => break,
                Err(error) if error.raw_os_error() == Some(32) => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(error) => panic!("Could not remove closed QA database: {error}"),
            }
        }
    })
    .await
    .unwrap();
}

struct FailingStore;
#[tokio::test]
async fn an_empty_poll_cursor_does_not_reset_the_saved_position() {
    let mut state = WeixinState {
        cursor: "saved-position".into(),
        ..Default::default()
    };
    state.advance(None, "account", Some(""), &[]).await.unwrap();
    assert_eq!(state.cursor, "saved-position");
}

#[async_trait::async_trait]
impl PluginStateStore for FailingStore {
    async fn load(&self, _: &str) -> Result<Option<String>, ChannelError> {
        Ok(Some("{invalid".into()))
    }
    async fn save(&self, _: &str, _: &str) -> Result<(), ChannelError> {
        Err(ChannelError::PlatformApi("storage unavailable".into()))
    }
}

#[tokio::test]
async fn storage_failure_does_not_advance_cursor_or_replace_context() {
    let mut state = WeixinState {
        cursor: "previous".into(),
        ..Default::default()
    };
    state.context_tokens.insert("chat".into(), "old".into());
    let message = WeixinRawMessage {
        from_user_id: Some("chat".into()),
        context_token: Some("new".into()),
        ..Default::default()
    };
    let error = state
        .advance(Some(&FailingStore), "account", Some("new-cursor"), &[message])
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Platform API error: storage unavailable");
    assert_eq!(state.cursor, "previous");
    assert_eq!(state.context_tokens["chat"], "old");
    let error = WeixinState::restore(Some(&FailingStore), "account")
        .await
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "Invalid plugin configuration: Invalid encrypted WeChat runtime state"
    );
}
