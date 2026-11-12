//! Composition adapter: tell the owner's chat bot when a long task finishes.
//!
//! A phone that reaches the desktop through the remote-access tunnel cannot be
//! relied on to get a browser notification (a locked phone freezes the page),
//! but a chat bot can always reach it. Started tasks are not tracked here —
//! the only signals used are the ones the realtime bus already carries:
//! `message.stream` (the first one of a turn marks its start) and
//! `turn.completed`. The message holds only the conversation name and how long
//! it took, never the result itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dream_core_api_types::CronPushTarget;
use dream_core_cron::pusher::CronResultPusher;
use dream_core_db::{IClientPreferenceRepository, IConversationRepository};
use dream_core_realtime::BroadcastEventBus;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

/// Client-preference key holding the user's choice (JSON object, see [`TurnPushConfig`]).
pub const PREF_KEY: &str = "notify.turnCompleted";

/// Turns shorter than this are not worth a phone buzz unless the user says otherwise.
const DEFAULT_MIN_SECONDS: u64 = 60;

/// Bound on turns whose start is remembered; a turn that never completes must not leak.
const MAX_TRACKED_TURNS: usize = 2000;
const TRACKED_TURN_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const MAX_NAME_CHARS: usize = 60;

fn default_min_seconds() -> u64 {
    DEFAULT_MIN_SECONDS
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TurnPushConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub plugin_id: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default = "default_min_seconds")]
    pub min_seconds: u64,
}

/// The stored preference, or `None` when the feature is off, unset or malformed.
/// A malformed value is treated as "off" rather than guessed at.
pub fn parse_config(raw: &str) -> Option<TurnPushConfig> {
    let config: TurnPushConfig = serde_json::from_str(raw).ok()?;
    (config.enabled && !config.plugin_id.trim().is_empty() && !config.chat_id.trim().is_empty()).then_some(config)
}

/// Whether a finished turn should be pushed.
///
/// Conversations that came in through a chat bot already deliver their reply
/// in that chat, so pushing again would only duplicate it.
pub fn should_push(config: &TurnPushConfig, conversation_source: Option<&str>, elapsed: Duration) -> bool {
    let from_desktop = matches!(conversation_source, None | Some("") | Some("dream"));
    from_desktop && elapsed >= Duration::from_secs(config.min_seconds)
}

fn format_elapsed(elapsed: Duration) -> String {
    let total = elapsed.as_secs();
    match total {
        0..=59 => format!("{total} 秒"),
        60..=3599 => format!("{} 分 {} 秒", total / 60, total % 60),
        _ => format!("{} 小时 {} 分", total / 3600, (total % 3600) / 60),
    }
}

pub fn compose_text(conversation_name: &str, elapsed: Duration) -> String {
    let name = conversation_name.trim();
    let name = if name.chars().count() > MAX_NAME_CHARS {
        format!("{}…", name.chars().take(MAX_NAME_CHARS).collect::<String>())
    } else {
        name.to_owned()
    };
    let headline = if name.is_empty() {
        "✅ 任务已完成".to_owned()
    } else {
        format!("✅ 任务已完成：{name}")
    };
    format!("{headline}\n用时 {}。回到 One Work 查看结果。", format_elapsed(elapsed))
}

/// What the notifier needs from the outside, so the logic above can be tested
/// without a database or a chat bot.
#[async_trait]
pub trait TurnPushLookup: Send + Sync {
    async fn config(&self, user_id: &str) -> Option<TurnPushConfig>;
    /// `(name, source)` of a conversation owned by `user_id`.
    async fn conversation(&self, user_id: &str, conversation_id: &str) -> Option<(String, Option<String>)>;
}

pub struct DbTurnPushLookup {
    pub prefs: Arc<dyn IClientPreferenceRepository>,
    pub conversations: Arc<dyn IConversationRepository>,
}

#[async_trait]
impl TurnPushLookup for DbTurnPushLookup {
    async fn config(&self, user_id: &str) -> Option<TurnPushConfig> {
        let rows = self.prefs.get_by_keys(user_id, &[PREF_KEY]).await.ok()?;
        rows.into_iter()
            .find(|row| row.key == PREF_KEY)
            .and_then(|row| parse_config(&row.value))
    }

    async fn conversation(&self, user_id: &str, conversation_id: &str) -> Option<(String, Option<String>)> {
        let row = self.conversations.get(user_id, conversation_id).await.ok()??;
        Some((row.name, row.source))
    }
}

/// Handle one finished turn. Failures are logged, never raised: a push that
/// cannot be delivered must not affect the turn that produced it.
pub async fn notify_turn_completed(
    lookup: &dyn TurnPushLookup,
    pusher: &dyn CronResultPusher,
    user_id: &str,
    conversation_id: &str,
    elapsed: Duration,
) {
    let Some(config) = lookup.config(user_id).await else {
        return;
    };
    // Cheap rejection first: most turns are short, so skip the conversation lookup for them.
    if elapsed < Duration::from_secs(config.min_seconds) {
        return;
    }
    let Some((name, source)) = lookup.conversation(user_id, conversation_id).await else {
        return;
    };
    if !should_push(&config, source.as_deref(), elapsed) {
        return;
    }
    let target = CronPushTarget {
        plugin_id: config.plugin_id.clone(),
        chat_id: config.chat_id.clone(),
    };
    match pusher.push(user_id, &target, &compose_text(&name, elapsed)).await {
        Ok(()) => tracing::info!(conversation_id, plugin_id = %target.plugin_id, "turn-completed push delivered"),
        Err(error) => {
            tracing::warn!(conversation_id, plugin_id = %target.plugin_id, error = %error, "turn-completed push failed")
        }
    }
}

/// Watch the realtime bus for the lifetime of the process.
pub fn spawn(bus: Arc<BroadcastEventBus>, lookup: Arc<dyn TurnPushLookup>, pusher: Arc<dyn CronResultPusher>) {
    tokio::spawn(async move {
        let mut rx = bus.subscribe();
        let mut started: HashMap<String, Instant> = HashMap::new();
        loop {
            let message = match rx.recv().await {
                Ok(message) => message,
                // Falling behind only loses some stream chunks; the next turn is tracked normally.
                Err(RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "turn-completed push watcher lagged");
                    continue;
                }
                Err(RecvError::Closed) => break,
            };
            match message.name.as_str() {
                "message.stream" => {
                    let Some(turn_id) = message.data["turn_id"].as_str() else {
                        continue;
                    };
                    if started.len() >= MAX_TRACKED_TURNS {
                        started.retain(|_, at| at.elapsed() < TRACKED_TURN_TTL);
                    }
                    if started.len() < MAX_TRACKED_TURNS {
                        started.entry(turn_id.to_owned()).or_insert_with(Instant::now);
                    }
                }
                "turn.completed" => {
                    let (Some(turn_id), Some(user_id), Some(conversation_id)) = (
                        message.data["turn_id"].as_str(),
                        message.data["user_id"].as_str(),
                        message.data["conversation_id"].as_str(),
                    ) else {
                        continue;
                    };
                    // A turn whose start was never seen (the watcher began mid-turn) has no
                    // known length, so it is not pushed rather than guessed at.
                    let Some(began) = started.remove(turn_id) else {
                        continue;
                    };
                    let elapsed = began.elapsed();
                    let (user_id, conversation_id) = (user_id.to_owned(), conversation_id.to_owned());
                    let (lookup, pusher) = (lookup.clone(), pusher.clone());
                    // Detached: a slow chat platform must not stall the bus reader.
                    tokio::spawn(async move {
                        notify_turn_completed(lookup.as_ref(), pusher.as_ref(), &user_id, &conversation_id, elapsed)
                            .await;
                    });
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn config(min_seconds: u64) -> TurnPushConfig {
        TurnPushConfig {
            enabled: true,
            plugin_id: "lark_1".into(),
            chat_id: "oc_abc".into(),
            min_seconds,
        }
    }

    #[test]
    fn parses_an_enabled_target_with_the_default_threshold() {
        let parsed = parse_config(r#"{"enabled":true,"plugin_id":"lark_1","chat_id":"oc_abc"}"#).unwrap();
        assert_eq!(parsed, config(60));
    }

    #[test]
    fn off_unset_or_malformed_means_no_push() {
        assert!(parse_config(r#"{"enabled":false,"plugin_id":"p","chat_id":"c"}"#).is_none());
        assert!(parse_config(r#"{"enabled":true,"plugin_id":"","chat_id":"c"}"#).is_none());
        assert!(parse_config(r#"{"enabled":true,"plugin_id":"p","chat_id":"  "}"#).is_none());
        assert!(parse_config("true").is_none());
        assert!(parse_config("not json").is_none());
    }

    #[test]
    fn short_turns_and_chat_originated_turns_are_not_pushed() {
        let c = config(60);
        assert!(should_push(&c, Some("dream"), Duration::from_secs(60)));
        assert!(should_push(&c, None, Duration::from_secs(600)));
        assert!(!should_push(&c, Some("dream"), Duration::from_secs(59)));
        for source in ["lark", "dingtalk", "telegram", "weixin"] {
            assert!(!should_push(&c, Some(source), Duration::from_secs(600)), "{source}");
        }
    }

    #[test]
    fn zero_threshold_pushes_every_desktop_turn() {
        assert!(should_push(&config(0), Some("dream"), Duration::from_secs(0)));
    }

    #[test]
    fn message_carries_the_name_and_duration_but_not_the_result() {
        assert_eq!(
            compose_text("整理合同", Duration::from_secs(192)),
            "✅ 任务已完成：整理合同\n用时 3 分 12 秒。回到 One Work 查看结果。"
        );
        assert_eq!(
            compose_text("  ", Duration::from_secs(5)),
            "✅ 任务已完成\n用时 5 秒。回到 One Work 查看结果。"
        );
        assert!(compose_text("x", Duration::from_secs(7500)).contains("2 小时 5 分"));
    }

    #[test]
    fn a_very_long_name_is_clipped() {
        let text = compose_text(&"字".repeat(200), Duration::from_secs(61));
        assert!(text.contains(&format!("{}…", "字".repeat(MAX_NAME_CHARS))));
        assert!(!text.contains(&"字".repeat(MAX_NAME_CHARS + 1)));
    }

    struct FakeLookup {
        config: Option<TurnPushConfig>,
        conversation: Option<(String, Option<String>)>,
        conversation_lookups: Mutex<u32>,
    }

    #[async_trait]
    impl TurnPushLookup for FakeLookup {
        async fn config(&self, _user_id: &str) -> Option<TurnPushConfig> {
            self.config.clone()
        }
        async fn conversation(&self, _user_id: &str, _conversation_id: &str) -> Option<(String, Option<String>)> {
            *self.conversation_lookups.lock().unwrap() += 1;
            self.conversation.clone()
        }
    }

    #[derive(Default)]
    struct FakePusher {
        pushed: Mutex<Vec<(String, CronPushTarget, String)>>,
        fail: bool,
    }

    #[async_trait]
    impl CronResultPusher for FakePusher {
        async fn push(&self, owner: &str, target: &CronPushTarget, text: &str) -> Result<(), String> {
            self.pushed
                .lock()
                .unwrap()
                .push((owner.to_owned(), target.clone(), text.to_owned()));
            if self.fail { Err("offline".into()) } else { Ok(()) }
        }
    }

    fn lookup(config: Option<TurnPushConfig>, source: Option<&str>) -> FakeLookup {
        FakeLookup {
            config,
            conversation: Some(("整理合同".into(), source.map(str::to_owned))),
            conversation_lookups: Mutex::new(0),
        }
    }

    #[tokio::test]
    async fn a_long_desktop_turn_is_pushed_to_the_chosen_chat() {
        let (lookup, pusher) = (lookup(Some(config(60)), Some("dream")), FakePusher::default());
        notify_turn_completed(&lookup, &pusher, "u1", "c1", Duration::from_secs(120)).await;
        let pushed = pusher.pushed.lock().unwrap();
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].0, "u1");
        assert_eq!(
            pushed[0].1,
            CronPushTarget {
                plugin_id: "lark_1".into(),
                chat_id: "oc_abc".into()
            }
        );
        assert!(pushed[0].2.starts_with("✅ 任务已完成：整理合同"));
    }

    #[tokio::test]
    async fn nothing_is_pushed_when_the_feature_is_off() {
        let (lookup, pusher) = (lookup(None, Some("dream")), FakePusher::default());
        notify_turn_completed(&lookup, &pusher, "u1", "c1", Duration::from_secs(900)).await;
        assert!(pusher.pushed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn short_turns_skip_even_the_conversation_lookup() {
        let (lookup, pusher) = (lookup(Some(config(60)), Some("dream")), FakePusher::default());
        notify_turn_completed(&lookup, &pusher, "u1", "c1", Duration::from_secs(5)).await;
        assert!(pusher.pushed.lock().unwrap().is_empty());
        assert_eq!(*lookup.conversation_lookups.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn a_conversation_that_came_from_a_chat_is_not_echoed_back() {
        let (lookup, pusher) = (lookup(Some(config(0)), Some("lark")), FakePusher::default());
        notify_turn_completed(&lookup, &pusher, "u1", "c1", Duration::from_secs(900)).await;
        assert!(pusher.pushed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_watcher_pushes_when_a_stream_start_and_a_completion_are_seen() {
        use dream_core_api_types::WebSocketMessage;
        use dream_core_realtime::EventBroadcaster;
        use serde_json::json;

        let bus = Arc::new(BroadcastEventBus::new(64));
        let pusher = Arc::new(FakePusher::default());
        spawn(
            bus.clone(),
            Arc::new(lookup(Some(config(0)), Some("dream"))),
            pusher.clone(),
        );
        // Let the spawned watcher subscribe before anything is sent.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Completion of a turn whose start was never seen: unknown length, not pushed.
        bus.broadcast(WebSocketMessage::new(
            "turn.completed",
            json!({"user_id": "u1", "conversation_id": "c1", "turn_id": "unseen", "status": "finished"}),
        ));
        bus.broadcast(WebSocketMessage::new(
            "message.stream",
            json!({"turn_id": "t1", "type": "content"}),
        ));
        bus.broadcast(WebSocketMessage::new(
            "turn.completed",
            json!({"user_id": "u1", "conversation_id": "c1", "turn_id": "t1", "status": "finished"}),
        ));

        for _ in 0..50 {
            if !pusher.pushed.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let pushed = pusher.pushed.lock().unwrap();
        assert_eq!(pushed.len(), 1, "only the turn whose start was seen is pushed");
        assert!(pushed[0].2.starts_with("✅ 任务已完成：整理合同"));
    }

    #[tokio::test]
    async fn a_failing_push_is_swallowed() {
        let (lookup, pusher) = (
            lookup(Some(config(0)), Some("dream")),
            FakePusher {
                fail: true,
                ..Default::default()
            },
        );
        notify_turn_completed(&lookup, &pusher, "u1", "c1", Duration::from_secs(900)).await;
        assert_eq!(pusher.pushed.lock().unwrap().len(), 1);
    }
}
