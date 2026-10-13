//! `GET /api/channel/push-targets`: the bots (and chats) a scheduled run's result
//! can be delivered to.
//!
//! A platform only reveals a chat id once someone has messaged the bot, so the
//! list is built from the sessions the bot has already opened. A bot that has
//! never been messaged shows up with no chats, which the client explains instead
//! of hiding the bot.

use std::collections::HashMap;

use axum::extract::{Extension, Json, State};
use dream_core_api_types::{ApiResponse, PluginStatusResponse, PushTargetChat, PushTargetResponse};
use dream_core_auth::CurrentUser;
use dream_core_common::ApiError;
use dream_core_db::models::{AssistantSessionRow, AssistantUserRow};

use crate::routes::ChannelRouterState;

/// Platforms whose direct-message chat id is the user's own id (WeChat uses the
/// sender id as the chat; a Telegram private chat id equals the user id). Others
/// (Lark, Slack, Discord, ...) use chat ids that only a session reveals.
const DIRECT_CHAT_PLATFORMS: [&str; 2] = ["weixin", "telegram"];

pub(crate) async fn get_push_targets(
    State(state): State<ChannelRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<PushTargetResponse>>>, ApiError> {
    let plugins = state.manager.get_plugin_status(&user.id).await?;
    let users = state
        .repo
        .get_all_users(&user.id)
        .await
        .map_err(crate::error::ChannelError::from)?;
    let sessions = state
        .repo
        .get_all_sessions(&user.id)
        .await
        .map_err(crate::error::ChannelError::from)?;
    Ok(Json(ApiResponse::ok(build_push_targets(plugins, &users, &sessions))))
}

/// Enabled bots, each with the distinct chats of the users authorized on that
/// platform. Pure so the matching rules are testable without a database.
pub(crate) fn build_push_targets(
    plugins: Vec<PluginStatusResponse>,
    users: &[AssistantUserRow],
    sessions: &[AssistantSessionRow],
) -> Vec<PushTargetResponse> {
    let users_by_id: HashMap<&str, &AssistantUserRow> = users.iter().map(|u| (u.id.as_str(), u)).collect();

    plugins
        .into_iter()
        .filter(|plugin| plugin.enabled)
        .map(|plugin| {
            let mut chats: Vec<PushTargetChat> = Vec::new();
            for session in sessions {
                let Some(chat_id) = session.chat_id.as_deref().filter(|id| !id.is_empty()) else {
                    continue;
                };
                let Some(owner) = users_by_id.get(session.user_id.as_str()) else {
                    continue;
                };
                if owner.platform_type != plugin.plugin_type || chats.iter().any(|chat| chat.chat_id == chat_id) {
                    continue;
                }
                chats.push(PushTargetChat {
                    chat_id: chat_id.to_owned(),
                    display_name: owner.display_name.clone(),
                });
            }
            // A person who has been authorized but has not chatted since has no
            // session yet. On these platforms a direct chat is addressed by the
            // person's own id, so they can still be offered.
            if DIRECT_CHAT_PLATFORMS.contains(&plugin.plugin_type.as_str()) {
                for owner in users.iter().filter(|u| u.platform_type == plugin.plugin_type) {
                    if !chats.iter().any(|chat| chat.chat_id == owner.platform_user_id) {
                        chats.push(PushTargetChat {
                            chat_id: owner.platform_user_id.clone(),
                            display_name: owner.display_name.clone(),
                        });
                    }
                }
            }
            PushTargetResponse {
                plugin_id: plugin.plugin_id,
                plugin_type: plugin.plugin_type,
                name: plugin.name,
                connected: plugin.connected,
                chats,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(id: &str, enabled: bool) -> PluginStatusResponse {
        PluginStatusResponse {
            plugin_id: id.into(),
            plugin_type: id.into(),
            name: format!("{id} bot"),
            enabled,
            status: None,
            last_connected: None,
            created_at: 0,
            updated_at: 0,
            connected: enabled,
            has_token: true,
            bot_username: None,
            active_users: 0,
        }
    }

    fn user(id: &str, platform: &str, name: Option<&str>) -> AssistantUserRow {
        AssistantUserRow {
            id: id.into(),
            owner_user_id: "owner".into(),
            platform_user_id: format!("pu-{id}"),
            platform_type: platform.into(),
            display_name: name.map(Into::into),
            authorized_at: 0,
            last_active: None,
            session_id: None,
        }
    }

    fn session(user_id: &str, chat_id: Option<&str>) -> AssistantSessionRow {
        AssistantSessionRow {
            id: format!("s-{user_id}-{}", chat_id.unwrap_or("none")),
            user_id: user_id.into(),
            agent_type: "dream".into(),
            conversation_id: None,
            workspace: None,
            chat_id: chat_id.map(Into::into),
            created_at: 0,
            last_activity: 0,
        }
    }

    #[test]
    fn disabled_bots_are_not_offered() {
        let targets = build_push_targets(vec![plugin("telegram", false), plugin("lark", true)], &[], &[]);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].plugin_id, "lark");
    }

    #[test]
    fn a_bot_nobody_messaged_is_listed_without_chats() {
        let targets = build_push_targets(vec![plugin("lark", true)], &[], &[]);
        assert!(targets[0].chats.is_empty());
    }

    #[test]
    fn chats_come_only_from_users_of_the_same_platform() {
        let users = [user("u1", "lark", Some("小王")), user("u2", "telegram", None)];
        let sessions = [session("u1", Some("chat-lark")), session("u2", Some("chat-tg"))];
        let targets = build_push_targets(vec![plugin("lark", true), plugin("telegram", true)], &users, &sessions);
        assert_eq!(
            targets[0].chats,
            vec![PushTargetChat {
                chat_id: "chat-lark".into(),
                display_name: Some("小王".into())
            }]
        );
        assert_eq!(targets[1].chats[0].chat_id, "chat-tg");
    }

    #[test]
    fn an_authorized_user_who_never_chatted_is_offered_on_direct_chat_platforms_only() {
        let users = [user("w1", "weixin", Some("wechat")), user("l1", "lark", Some("小王"))];
        let targets = build_push_targets(vec![plugin("weixin", true), plugin("lark", true)], &users, &[]);
        assert_eq!(
            targets[0].chats,
            vec![PushTargetChat {
                chat_id: "pu-w1".into(),
                display_name: Some("wechat".into())
            }]
        );
        assert!(
            targets[1].chats.is_empty(),
            "a Lark chat id cannot be derived from the user id"
        );
    }

    #[test]
    fn a_session_chat_and_the_user_id_do_not_duplicate_on_direct_platforms() {
        let users = [user("w1", "weixin", None)];
        let sessions = [session("w1", Some("pu-w1"))];
        let targets = build_push_targets(vec![plugin("weixin", true)], &users, &sessions);
        assert_eq!(targets[0].chats.len(), 1);
    }

    #[test]
    fn duplicate_and_blank_chats_collapse() {
        let users = [user("u1", "lark", None)];
        let sessions = [
            session("u1", Some("c1")),
            session("u1", Some("c1")),
            session("u1", Some("")),
            session("u1", None),
            session("ghost", Some("c9")),
        ];
        let targets = build_push_targets(vec![plugin("lark", true)], &users, &sessions);
        assert_eq!(targets[0].chats.len(), 1);
        assert_eq!(targets[0].chats[0].chat_id, "c1");
    }
}
