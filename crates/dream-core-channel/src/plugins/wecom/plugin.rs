use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::error::ChannelError;
use crate::plugin::{ChannelPlugin, PluginCallbacks};
use crate::types::{
    BotInfo, MessageContentType, PluginConfig, PluginStatus, PluginType, UnifiedIncomingMessage, UnifiedMessageContent,
    UnifiedOutgoingMessage, UnifiedUser,
};

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
const WS_URL: &str = "wss://openws.work.weixin.qq.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DEDUP_TTL: Duration = Duration::from_secs(600);
const MAX_SEEN: usize = 4096;
// Keep each markdown message comfortably below the platform byte limit.
const TEXT_BYTES: usize = 4096;

struct Outbound {
    frame: Value,
    done: oneshot::Sender<Result<Value, String>>,
}

pub struct WecomPlugin {
    status: Arc<RwLock<PluginStatus>>,
    last_error: Option<String>,
    info: Option<BotInfo>,
    credentials: Option<(String, String)>,
    callbacks: Option<PluginCallbacks>,
    outbound: Option<mpsc::Sender<Outbound>>,
    shutdown: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
    send_lock: Mutex<()>,
}

impl Default for WecomPlugin {
    fn default() -> Self {
        Self {
            status: Arc::new(RwLock::new(PluginStatus::Created)),
            last_error: None,
            info: None,
            credentials: None,
            callbacks: None,
            outbound: None,
            shutdown: None,
            task: None,
            send_lock: Mutex::new(()),
        }
    }
}

impl Drop for WecomPlugin {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(true);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl WecomPlugin {
    async fn request(&self, cmd: &str, body: Value) -> Result<Value, ChannelError> {
        let sender = self
            .outbound
            .as_ref()
            .ok_or_else(|| ChannelError::ConnectionFailed("WeCom bot is stopped".into()))?;
        let (done, rx) = oneshot::channel();
        let frame = json!({"cmd":cmd,"headers":{"req_id":request_id()},"body":body});
        tokio::time::timeout(REQUEST_TIMEOUT, sender.send(Outbound { frame, done }))
            .await
            .map_err(|_| ChannelError::MessageSendFailed("WeCom send queue timed out".into()))?
            .map_err(|_| ChannelError::ConnectionFailed("WeCom connection closed".into()))?;
        tokio::time::timeout(REQUEST_TIMEOUT, rx)
            .await
            .map_err(|_| ChannelError::MessageSendFailed("WeCom acknowledgement timed out".into()))?
            .map_err(|_| ChannelError::ConnectionFailed("WeCom connection closed".into()))?
            .map_err(ChannelError::MessageSendFailed)
    }

    async fn send_file(&self, chat: &str, path: &str, name: &str) -> Result<String, ChannelError> {
        use base64::Engine;
        use md5::{Digest, Md5};
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| ChannelError::MessageSendFailed(e.to_string()))?;
        if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
            return Err(ChannelError::InvalidConfig(
                "Attachment must be between 1 byte and 20 MiB".into(),
            ));
        }
        let _guard = self.send_lock.lock().await;
        let chunk_size = 512 * 1024;
        let init = self.request("aibot_upload_media_init", json!({"type":"file","filename":name,"total_size":bytes.len(),"total_chunks":bytes.len().div_ceil(chunk_size),"md5":format!("{:x}", Md5::digest(&bytes))})).await?;
        let upload = init["body"]["upload_id"]
            .as_str()
            .ok_or_else(|| ChannelError::MessageSendFailed("WeCom returned no upload id".into()))?;
        for (index, chunk) in bytes.chunks(chunk_size).enumerate() {
            self.request("aibot_upload_media_chunk", json!({"upload_id":upload,"chunk_index":index,"base64_data":base64::engine::general_purpose::STANDARD.encode(chunk)})).await?;
        }
        let finished = self
            .request("aibot_upload_media_finish", json!({"upload_id":upload}))
            .await?;
        let media = finished["body"]["media_id"]
            .as_str()
            .ok_or_else(|| ChannelError::MessageSendFailed("WeCom returned no media id".into()))?;
        self.request(
            "aibot_send_msg",
            json!({"chatid":chat,"msgtype":"file","file":{"media_id":media}}),
        )
        .await?;
        Ok(media.to_owned())
    }
    pub fn new() -> Self {
        Self::default()
    }

    fn set_status(&self, status: PluginStatus) {
        *self.status.write().unwrap_or_else(|e| e.into_inner()) = status;
    }

    async fn send_text(&self, chat: &str, text: &str) -> Result<String, ChannelError> {
        if self.status() != PluginStatus::Running {
            return Err(ChannelError::ConnectionFailed("WeCom bot is not connected".into()));
        }
        let sender = self
            .outbound
            .as_ref()
            .ok_or_else(|| ChannelError::ConnectionFailed("WeCom bot is stopped".into()))?;
        // ACKs are matched by req_id. Serialize sends and never retry an ambiguous delivery.
        let _guard = self.send_lock.lock().await;
        let mut last = String::new();
        for content in split_utf8(text, TEXT_BYTES) {
            let req = request_id();
            let (done, rx) = oneshot::channel();
            let frame = json!({"cmd":"aibot_send_msg","headers":{"req_id":req},"body":{"chatid":chat,"msgtype":"markdown","markdown":{"content":content}}});
            tokio::time::timeout(REQUEST_TIMEOUT, sender.send(Outbound { frame, done }))
                .await
                .map_err(|_| ChannelError::MessageSendFailed("WeCom send queue timed out".into()))?
                .map_err(|_| ChannelError::ConnectionFailed("WeCom connection closed".into()))?;
            tokio::time::timeout(REQUEST_TIMEOUT, rx)
                .await
                .map_err(|_| {
                    ChannelError::MessageSendFailed(
                        "WeCom delivery acknowledgement timed out; delivery is uncertain".into(),
                    )
                })?
                .map_err(|_| ChannelError::ConnectionFailed("WeCom connection closed".into()))?
                .map_err(ChannelError::MessageSendFailed)?;
            last = req;
        }
        Ok(last)
    }
}

#[async_trait::async_trait]
impl ChannelPlugin for WecomPlugin {
    async fn initialize(&mut self, config: PluginConfig, callbacks: PluginCallbacks) -> Result<(), ChannelError> {
        self.set_status(PluginStatus::Initializing);
        let bot = config.credentials.app_id.filter(|v| !v.trim().is_empty());
        let secret = config.credentials.app_secret.filter(|v| !v.trim().is_empty());
        let (Some(bot), Some(secret)) = (bot, secret) else {
            self.set_status(PluginStatus::Error);
            self.last_error = Some("WeCom intelligent bot requires Bot ID and Secret".into());
            return Err(ChannelError::InvalidConfig(self.last_error.clone().unwrap()));
        };
        let mut probe = match connect_authenticated(&bot, &secret).await {
            Ok(socket) => socket,
            Err(e) => {
                self.set_status(PluginStatus::Error);
                self.last_error = Some(e.to_string());
                return Err(e);
            }
        };
        let _ = probe.close(None).await;
        self.info = Some(BotInfo {
            id: bot.clone(),
            username: None,
            display_name: "WeCom intelligent bot".into(),
        });
        self.credentials = Some((bot, secret));
        self.callbacks = Some(callbacks);
        self.set_status(PluginStatus::Ready);
        Ok(())
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.task.is_some() {
            return Err(ChannelError::PluginAlreadyRunning("wecom".into()));
        }
        let (bot, secret) = self
            .credentials
            .clone()
            .ok_or_else(|| ChannelError::InvalidConfig("WeCom is not initialized".into()))?;
        self.set_status(PluginStatus::Starting);
        let socket = match connect_authenticated(&bot, &secret).await {
            Ok(socket) => socket,
            Err(e) => {
                self.set_status(PluginStatus::Error);
                self.last_error = Some(e.to_string());
                return Err(e);
            }
        };
        let callbacks = self
            .callbacks
            .clone()
            .ok_or_else(|| ChannelError::InvalidConfig("WeCom callbacks missing".into()))?;
        let (tx, rx) = mpsc::channel(32);
        let (shutdown, stop) = watch::channel(false);
        self.outbound = Some(tx);
        self.shutdown = Some(shutdown);
        self.set_status(PluginStatus::Running);
        self.task = Some(tokio::spawn(connection_loop(
            socket,
            bot,
            secret,
            callbacks,
            rx,
            stop,
            self.status.clone(),
        )));
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        self.set_status(PluginStatus::Stopping);
        if let Some(stop) = self.shutdown.take() {
            let _ = stop.send(true);
        }
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(Duration::from_secs(5), &mut task).await.is_err()
        {
            task.abort();
            let _ = task.await;
        }
        self.outbound = None;
        self.set_status(PluginStatus::Stopped);
        Ok(())
    }

    async fn send_message(&self, chat_id: &str, message: UnifiedOutgoingMessage) -> Result<String, ChannelError> {
        if message.message_type == crate::types::OutgoingMessageType::File {
            let path = message
                .file_url
                .as_deref()
                .ok_or_else(|| ChannelError::InvalidConfig("Missing attachment path".into()))?;
            return self
                .send_file(chat_id, path, message.file_name.as_deref().unwrap_or("result.md"))
                .await;
        }
        self.send_text(chat_id, message.text.as_deref().unwrap_or("")).await
    }

    async fn edit_message(
        &self,
        _chat_id: &str,
        _message_id: &str,
        _message: UnifiedOutgoingMessage,
    ) -> Result<(), ChannelError> {
        Err(ChannelError::PlatformApi(
            "WeCom markdown messages are sent once and cannot be edited".into(),
        ))
    }
    fn active_user_count(&self) -> usize {
        0
    }
    fn bot_info(&self) -> Option<&BotInfo> {
        self.info.as_ref()
    }
    fn plugin_type(&self) -> PluginType {
        PluginType::Wecom
    }
    fn status(&self) -> PluginStatus {
        *self.status.read().unwrap_or_else(|e| e.into_inner())
    }
    fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
}

async fn connect_authenticated(bot: &str, secret: &str) -> Result<Socket, ChannelError> {
    let certs = rustls_native_certs::load_native_certs();
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(certs.certs);
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| ChannelError::ConnectionFailed("WeCom TLS configuration failed".into()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    let fut = async {
        let (mut ws, _) = tokio_tungstenite::connect_async_tls_with_config(
            WS_URL,
            None,
            false,
            Some(tokio_tungstenite::Connector::Rustls(Arc::new(tls))),
        )
        .await
        .map_err(|_| ChannelError::ConnectionFailed("WeCom WebSocket connection failed".into()))?;
        let req = request_id();
        ws.send(Message::Text(
            json!({"cmd":"aibot_subscribe","headers":{"req_id":req},"body":{"bot_id":bot,"secret":secret}})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| ChannelError::ConnectionFailed("WeCom authentication send failed".into()))?;
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(Message::Text(text)) => {
                    let data: Value = serde_json::from_str(&text)?;
                    if data["headers"]["req_id"].as_str() == Some(&req) {
                        if data["errcode"].as_i64() != Some(0) {
                            return Err(ChannelError::ConnectionFailed(format!(
                                "WeCom authentication rejected (code {})",
                                data["errcode"].as_i64().unwrap_or(-1)
                            )));
                        }
                        tracing::info!("WeCom intelligent bot authenticated");
                        return Ok(ws);
                    }
                }
                Ok(Message::Ping(data)) => {
                    let _ = ws.send(Message::Pong(data)).await;
                }
                _ => break,
            }
        }
        Err(ChannelError::ConnectionFailed(
            "WeCom authentication connection closed".into(),
        ))
    };
    tokio::time::timeout(REQUEST_TIMEOUT, fut)
        .await
        .map_err(|_| ChannelError::ConnectionFailed("WeCom authentication timed out".into()))?
}

async fn connection_loop(
    mut socket: Socket,
    bot: String,
    secret: String,
    callbacks: PluginCallbacks,
    mut outgoing: mpsc::Receiver<Outbound>,
    mut stop: watch::Receiver<bool>,
    status: Arc<RwLock<PluginStatus>>,
) {
    let mut seen: HashMap<String, Instant> = HashMap::new();
    let mut attempts = 0u32;
    loop {
        let reason = listen(&mut socket, &callbacks, &mut outgoing, &mut stop, &mut seen).await;
        if *stop.borrow() || reason == EndReason::Stopped {
            break;
        }
        *status.write().unwrap_or_else(|e| e.into_inner()) = PluginStatus::Error;
        if reason == EndReason::Replaced {
            tracing::warn!("WeCom connection replaced by another client; stopped reconnecting");
            break;
        }
        loop {
            attempts += 1;
            if attempts > 10 {
                tracing::warn!("WeCom reconnect attempts exhausted");
                return;
            }
            tokio::select! {
                _ = stop.changed() => return,
                _ = tokio::time::sleep(Duration::from_secs((1u64 << attempts.min(5)).min(30))) => {}
            }
            match connect_authenticated(&bot, &secret).await {
                Ok(ws) => {
                    socket = ws;
                    *status.write().unwrap_or_else(|e| e.into_inner()) = PluginStatus::Running;
                    break;
                }
                Err(_) => tracing::warn!(attempts, "WeCom reconnect failed"),
            }
        }
    }
    let _ = socket.close(None).await;
}

#[derive(PartialEq)]
enum EndReason {
    Stopped,
    Disconnected,
    Replaced,
}

async fn listen(
    socket: &mut Socket,
    callbacks: &PluginCallbacks,
    outgoing: &mut mpsc::Receiver<Outbound>,
    stop: &mut watch::Receiver<bool>,
    seen: &mut HashMap<String, Instant>,
) -> EndReason {
    let mut pending: HashMap<String, (Instant, oneshot::Sender<Result<Value, String>>)> = HashMap::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    heartbeat.tick().await;
    let mut ping_id = String::new();
    let mut missed = 0;
    loop {
        tokio::select! {
            _ = stop.changed() => return EndReason::Stopped,
            _ = heartbeat.tick() => {
                if missed >= 2 { return EndReason::Disconnected; }
                ping_id = request_id(); missed += 1;
                if socket.send(Message::Text(json!({"cmd":"ping","headers":{"req_id":ping_id}}).to_string().into())).await.is_err() { return EndReason::Disconnected; }
                pending.retain(|_, (at, _)| at.elapsed() < REQUEST_TIMEOUT);
                seen.retain(|_, at| at.elapsed() < DEDUP_TTL);
            }
            command = outgoing.recv() => {
                let Some(command) = command else { return EndReason::Stopped; };
                if command.done.is_closed() { continue; }
                let req = command.frame["headers"]["req_id"].as_str().unwrap_or("").to_owned();
                if socket.send(Message::Text(command.frame.to_string().into())).await.is_err() { return EndReason::Disconnected; }
                pending.insert(req, (Instant::now(), command.done));
            }
            frame = socket.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(data) = serde_json::from_str::<Value>(&text) else { continue; };
                        let req = data["headers"]["req_id"].as_str().unwrap_or("");
                        if req == ping_id && data["errcode"].as_i64() == Some(0) { missed = 0; continue; }
                        if let Some((_, done)) = pending.remove(req) {
                            let result = if data["errcode"].as_i64() == Some(0) { Ok(data.clone()) } else { Err(format!("WeCom delivery rejected (code {})", data["errcode"].as_i64().unwrap_or(-1))) };
                            let _ = done.send(result); continue;
                        }
                        if data["cmd"] == "aibot_event_callback" && data["body"]["event"]["eventtype"] == "disconnected_event" { return EndReason::Replaced; }
                        if data["cmd"] != "aibot_msg_callback" { continue; }
                        if let Some(message) = incoming(&data["body"]) {
                            if seen.contains_key(&message.id) { continue; }
                            // Bounded memory and queue; never block ACK processing behind a full queue.
                            if seen.len() >= MAX_SEEN { seen.retain(|_, at| at.elapsed() < DEDUP_TTL); }
                            if seen.len() >= MAX_SEEN { tracing::warn!("WeCom inbound dedup capacity reached"); continue; }
                            let id = message.id.clone();
                            match callbacks.message_tx.try_send(message) {
                                Ok(()) => { seen.insert(id, Instant::now()); }
                                Err(_) => tracing::warn!("WeCom inbound queue unavailable"),
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        match socket.send(Message::Pong(data)).await {
                            Ok(()) => {}
                            Err(_) => return EndReason::Disconnected,
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return EndReason::Disconnected,
                    _ => {}
                }
            }
        }
    }
}

fn incoming(body: &Value) -> Option<UnifiedIncomingMessage> {
    let id = body["msgid"].as_str()?.trim();
    let user = body["from"]["userid"].as_str()?.trim();
    if id.is_empty() || user.is_empty() {
        return None;
    }
    let chat = match body["chattype"].as_str()? {
        "single" => user,
        "group" => body["chatid"].as_str()?.trim(),
        _ => return None,
    };
    if chat.is_empty() {
        return None;
    }
    let text = match body["msgtype"].as_str()? {
        "text" => body["text"]["content"].as_str()?.to_owned(),
        "voice" => body["voice"]["content"].as_str()?.to_owned(),
        _ => return None,
    };
    if text.trim().is_empty() {
        return None;
    }
    Some(UnifiedIncomingMessage {
        owner_user_id: None,
        id: id.into(),
        platform: PluginType::Wecom,
        chat_id: chat.into(),
        user: UnifiedUser {
            id: user.into(),
            username: None,
            display_name: user.into(),
            avatar_url: None,
        },
        content: UnifiedMessageContent {
            content_type: MessageContentType::Text,
            text,
            attachments: None,
        },
        timestamp: body["create_time"].as_i64().unwrap_or_default() * 1000,
        reply_to_message_id: None,
        action: None,
        raw: None,
    })
}

fn request_id() -> String {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "onework_{}_{}",
        dream_core_common::now_ms(),
        COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn split_utf8(text: &str, max: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(max);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&rest[..end]);
        rest = &rest[end..];
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn attachment_upload_waits_for_each_receipt_and_preserves_bytes() {
        use base64::Engine;
        let path = crate::delivery_test::file().await;
        let (tx, mut rx) = mpsc::channel::<Outbound>(8);
        let mut plugin = WecomPlugin::new();
        plugin.outbound = Some(tx);
        let worker = tokio::spawn(async move {
            let init = rx.recv().await.unwrap();
            assert_eq!(init.frame["cmd"], "aibot_upload_media_init");
            init.done.send(Ok(json!({"body":{"upload_id":"upload-test"}}))).unwrap();
            let chunk = rx.recv().await.unwrap();
            assert_eq!(chunk.frame["cmd"], "aibot_upload_media_chunk");
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(chunk.frame["body"]["base64_data"].as_str().unwrap())
                .unwrap();
            assert_eq!(bytes, b"name,value\nnotification-proof,42\n");
            chunk.done.send(Ok(json!({}))).unwrap();
            let finish = rx.recv().await.unwrap();
            assert_eq!(finish.frame["cmd"], "aibot_upload_media_finish");
            finish.done.send(Ok(json!({"body":{"media_id":"media-test"}}))).unwrap();
            let send = rx.recv().await.unwrap();
            assert_eq!(send.frame["body"]["chatid"], "chosen-chat");
            assert_eq!(send.frame["body"]["file"]["media_id"], "media-test");
            send.done.send(Err("denied".into())).unwrap();
        });
        assert!(
            plugin
                .send_file("chosen-chat", path.to_str().unwrap(), "report.csv")
                .await
                .unwrap_err()
                .to_string()
                .contains("denied")
        );
        worker.await.unwrap();
        tokio::fs::remove_file(path).await.unwrap();
    }
    #[test]
    fn normalize_private_and_group_messages_without_retaining_raw_secrets() {
        let mut b = json!({"msgid":"m1","chattype":"single","from":{"userid":"u1"},"msgtype":"text","text":{"content":"hello"},"response_url":"sensitive"});
        let m = incoming(&b).unwrap();
        assert_eq!(m.chat_id, "u1");
        assert!(m.raw.is_none());
        b["chattype"] = json!("group");
        assert!(incoming(&b).is_none());
        b["chatid"] = json!("group-1");
        assert_eq!(incoming(&b).unwrap().chat_id, "group-1");
        b["from"]["userid"] = json!("");
        assert!(incoming(&b).is_none());
    }
    #[test]
    fn utf8_chunks_reconstruct_the_entire_long_response() {
        let text = "中文🙂".repeat(2000);
        let parts = split_utf8(&text, TEXT_BYTES);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.len() <= TEXT_BYTES));
        assert_eq!(parts.concat(), text);
    }
}
