//! Which MCP servers a conversation gets, shared by the dream and ACP factories.
//!
//! Both factories used to inline the same rule, and both dropped every built-in
//! server on sight. That was right when built-ins were things a session should
//! only get when explicitly asked for (the PDF exporter, the team knowledge
//! base) — but it left the media generation server unreachable for any agent,
//! because a built-in can only ride in on the session snapshot the desktop
//! renderer writes, and only the new-conversation screen writes one. Every
//! conversation created anywhere else — a digital employee, a scheduled task, a
//! team member, or simply an older conversation — carries `mcp_server_ids: null`
//! and therefore had no way to generate an image or a video at all.
//!
//! So a narrow class of built-ins now rides on its own `enabled` flag instead,
//! which is exactly what the renderer already does for new conversations (see
//! `withBuiltinMediaMcp` in 1oneUI). Putting the rule here means one answer for
//! every conversation, however it was created.

use dream_core_db::IMcpServerRepository;
use dream_core_db::models::McpServerRow;
use tracing::warn;

/// Built-in servers a conversation gets on the strength of their own `enabled`
/// flag, without anyone having to tick them per assistant.
///
/// ⚠️ Deliberately a very short list. Auto-injecting every built-in would hand
/// each agent the PDF exporter and the team knowledge base as well, which is a
/// much larger decision than "the operator configured a media model".
///
/// These names are the MCP server names agents see, and they are a public
/// identifier — they must stay in step with the desktop shell, which registers
/// the rows under them:
///
/// - `BUILTIN_IMAGE_GEN_NAME` / `BUILTIN_IMAGE_GEN_LEGACY_NAMES` in
///   `packages/desktop/src/process/resources/builtinMcp/constants.ts`
/// - `BUILTIN_BROWSER_MCP_NAME` / `BUILTIN_BROWSER_MCP_LEGACY_NAMES` in
///   `packages/desktop/src/common/config/constants.ts`
/// - `BUILTIN_PAGE_READER_MCP_NAME` in
///   `packages/desktop/src/process/utils/runBackendMigrations.ts`
///
/// A name that drifts out of step does not fail — it silently stops being
/// injected, which is exactly how the browser regression documented below
/// stayed invisible.
const AUTO_INJECTED_BUILTIN_NAMES: &[&str] = &[
    "one-image-generation",
    // Legacy names, kept so installs that predate the rename keep working.
    "aionui-image-generation",
    "AionUi Image Generation",
    "builtin-image-gen",
    // Web search, on the same reasoning as media generation: it is inert until
    // the user pastes a provider API key, and pasting one is the explicit
    // statement that this capability is wanted. Without it here, an enabled
    // `one-web-search` still never reaches a conversation — the model is simply
    // never offered `web_search`, so it answers from training data or reaches
    // for an unrelated skill, with nothing anywhere saying why.
    "one-web-search",
    "builtin-web-search",
    // The in-app browser, on a different reasoning: it is not gated by a key,
    // it is gated by the user opening the browser preview panel — an act at
    // least as explicit as pasting one, and one that happens mid-conversation,
    // long after the new-conversation screen (the only place that writes a
    // snapshot) is gone. So the snapshot can never carry it.
    //
    // Observed before this was added: `one-browser` was enabled, its row said
    // `connected` with 26 tools, the settings page listed every one of them —
    // and a conversation with the browser panel open and a page loaded was
    // injected with `mcp_count=2 mcp_names=["one-image-generation",
    // "one-web-search"]`. Asked "what site did I just open", the model
    // correctly answered that it had no such tool. Nothing anywhere reported a
    // failure, and the `connected` status was read as proof the capability
    // worked, which sent a long debugging effort into the CDP bridge and the
    // chrome-devtools-mcp version — both downstream of a gate that was never
    // open.
    "one-browser",
    // Pre-rebrand legacy alias: sessions created before the rename still ask
    // for the browser MCP under this name (dream-ui's
    // BUILTIN_BROWSER_MCP_LEGACY_NAMES mirrors it). Deliberate, keep.
    "aionui-browser",
    // Reading the open page is the same capability minus the automation, and
    // ships as its own server so a model reaches for text extraction instead of
    // screenshot-and-OCR. Injected on the same terms, for the same reason.
    "one-page-reader",
];

pub(crate) fn is_auto_injected_builtin(name: &str) -> bool {
    AUTO_INJECTED_BUILTIN_NAMES.contains(&name)
}

/// The MCP server rows this conversation should be given.
///
/// Encodes one rule for both agent backends:
///
/// - an explicit snapshot (`selected_ids`) defines the session's servers, and
///   they are injected regardless of the current global `enabled` flag — a
///   conversation keeps what it was created with;
/// - without a snapshot, every enabled row applies;
/// - built-ins are excluded either way, **except** the auto-injected ones,
///   which follow their own `enabled` flag under both branches. That exception
///   is the whole point of this function: it is what makes media generation
///   reachable from a scheduled task or a digital employee, neither of which
///   ever writes a snapshot.
///
/// Soft-deleted rows never come back: `list()` filters them out, so a server
/// removed from the UI stays removed even if a conversation still names it.
///
/// One `list()` serves both branches. It is a superset of what the snapshot
/// fetch could return — `list()` keeps disabled rows (they are only filtered by
/// the predicate below, which honours the snapshot) and drops deleted ones,
/// exactly like `list_by_ids_any`. Reading the whole table also means an
/// auto-injected built-in is present to be considered even when the snapshot
/// never mentioned it, which no per-id fetch could give us.
pub(crate) async fn load_session_mcp_rows(
    repo: &dyn IMcpServerRepository,
    selected_ids: Option<&[String]>,
    user_id: &str,
    conversation_id: &str,
) -> Vec<McpServerRow> {
    let rows = match repo.list(user_id).await {
        Ok(r) => r,
        Err(err) => {
            warn!(
                conversation_id,
                error = %err,
                "user_mcp: list() failed; skipping injection"
            );
            return Vec::new();
        }
    };

    rows.into_iter()
        .filter(|row| {
            if row.builtin {
                // An auto-injected built-in answers to its own enabled flag,
                // never to the snapshot — the snapshot is about user servers.
                return is_auto_injected_builtin(&row.name) && row.enabled;
            }
            selected_ids
                .map(|ids| ids.iter().any(|id| id == &row.id))
                .unwrap_or(row.enabled)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use dream_core_db::DbError;

    fn row(id: &str, name: &str, enabled: bool, builtin: bool) -> McpServerRow {
        McpServerRow {
            id: id.to_owned(),
            user_id: "u".to_owned(),
            name: name.to_owned(),
            description: None,
            enabled,
            transport_type: "stdio".to_owned(),
            transport_config: "{}".to_owned(),
            tools: None,
            last_test_status: "connected".to_owned(),
            last_connected: None,
            original_json: None,
            builtin,
            deleted_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    struct FakeRepo {
        rows: Vec<McpServerRow>,
    }

    #[async_trait]
    impl IMcpServerRepository for FakeRepo {
        async fn list(&self, _user_id: &str) -> Result<Vec<McpServerRow>, DbError> {
            Ok(self.rows.clone())
        }
        async fn find_by_id(&self, _user_id: &str, id: &str) -> Result<Option<McpServerRow>, DbError> {
            Ok(self.rows.iter().find(|r| r.id == id).cloned())
        }
        async fn find_by_name(&self, _user_id: &str, name: &str) -> Result<Option<McpServerRow>, DbError> {
            Ok(self.rows.iter().find(|r| r.name == name).cloned())
        }
        async fn create(&self, _params: dream_core_db::CreateMcpServerParams<'_>) -> Result<McpServerRow, DbError> {
            unimplemented!()
        }
        async fn update(
            &self,
            _user_id: &str,
            _id: &str,
            _params: dream_core_db::UpdateMcpServerParams<'_>,
        ) -> Result<McpServerRow, DbError> {
            unimplemented!()
        }
        async fn delete(&self, _user_id: &str, _id: &str) -> Result<(), DbError> {
            unimplemented!()
        }
        async fn batch_upsert(
            &self,
            _user_id: &str,
            _servers: &[dream_core_db::CreateMcpServerParams<'_>],
        ) -> Result<Vec<McpServerRow>, DbError> {
            unimplemented!()
        }
        async fn update_status(
            &self,
            _user_id: &str,
            _id: &str,
            _status: &str,
            _last_connected: Option<dream_core_common::TimestampMs>,
        ) -> Result<(), DbError> {
            unimplemented!()
        }
        async fn update_tools(&self, _user_id: &str, _id: &str, _tools: Option<&str>) -> Result<(), DbError> {
            unimplemented!()
        }
    }

    fn names(rows: &[McpServerRow]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    /// Web search has to reach a conversation the same way media does.
    ///
    /// Observed before this was added: `one-web-search` was enabled, its
    /// connection test passed, and the settings page listed its `web_search`
    /// tool — but a conversation was injected with exactly one server
    /// (`mcp_count=1 mcp_names=["one-image-generation"]`). The model was never
    /// offered the tool, so a question about a live stock price was answered by
    /// reaching for an unrelated skill instead, with nothing reporting a
    /// failure anywhere.
    #[tokio::test]
    async fn web_search_builtin_reaches_a_conversation_with_no_snapshot() {
        let repo = FakeRepo {
            rows: vec![
                row("b1", "one-web-search", true, true),
                row("b2", "one-export-pdf", true, true),
            ],
        };
        let rows = load_session_mcp_rows(&repo, None, "u", "c1").await;
        assert_eq!(names(&rows), vec!["one-web-search"]);
    }

    /// Disabled means disabled: the entry ships default-off and only turns on
    /// when the user has pasted a provider key, so an off row must not ride in.
    #[tokio::test]
    async fn disabled_web_search_is_not_injected() {
        let repo = FakeRepo {
            rows: vec![row("b1", "one-web-search", false, true)],
        };
        assert!(load_session_mcp_rows(&repo, None, "u", "c1").await.is_empty());
    }

    /// The in-app browser must reach a conversation the same way.
    ///
    /// The regression this pins: the browser panel is opened mid-conversation,
    /// so the new-conversation snapshot — the only other way in for a built-in
    /// — cannot possibly carry it. Without the allowlist entry the model is
    /// simply never offered a browser tool, and says so, while every surface a
    /// human checks (enabled flag, `connected` status, the settings tool list)
    /// keeps reporting success.
    #[tokio::test]
    async fn browser_builtins_reach_a_conversation_with_no_snapshot() {
        let repo = FakeRepo {
            rows: vec![
                row("b1", "one-browser", true, true),
                row("b2", "one-page-reader", true, true),
                row("b3", "one-export-pdf", true, true),
                row("b4", "one-team-knowledge", true, true),
            ],
        };
        let rows = load_session_mcp_rows(&repo, None, "u", "c1").await;
        assert_eq!(names(&rows), vec!["one-browser", "one-page-reader"]);
    }

    /// The pre-rebrand browser name still rides in, so an install that never
    /// got renamed forward does not silently lose the capability.
    #[tokio::test]
    async fn legacy_browser_name_is_injected() {
        let repo = FakeRepo {
            rows: vec![row("b1", "aionui-browser", true, true)],
        };
        let rows = load_session_mcp_rows(&repo, None, "u", "c1").await;
        assert_eq!(names(&rows), vec!["aionui-browser"]);
    }

    /// Disabled still means disabled — turning the browser MCP off in settings
    /// has to keep it out of every conversation.
    #[tokio::test]
    async fn disabled_browser_builtins_are_not_injected() {
        let repo = FakeRepo {
            rows: vec![
                row("b1", "one-browser", false, true),
                row("b2", "one-page-reader", false, true),
            ],
        };
        assert!(load_session_mcp_rows(&repo, None, "u", "c1").await.is_empty());
    }

    /// The list stays short on purpose: adding a built-in here hands it to
    /// every agent, including scheduled tasks and digital employees. These two
    /// are the ones a conversation must never be expected to tick for itself.
    #[tokio::test]
    async fn heavy_builtins_stay_out() {
        let repo = FakeRepo {
            rows: vec![
                row("b1", "one-export-pdf", true, true),
                row("b2", "one-team-knowledge", true, true),
            ],
        };
        assert!(load_session_mcp_rows(&repo, None, "u", "c1").await.is_empty());
    }

    /// A snapshot is about user servers; an auto-injected built-in follows its
    /// own enabled flag even when the snapshot never mentioned it.
    #[tokio::test]
    async fn web_search_rides_along_despite_a_snapshot_that_omits_it() {
        let repo = FakeRepo {
            rows: vec![
                row("u1", "user-server", true, false),
                row("b1", "one-web-search", true, true),
            ],
        };
        let rows = load_session_mcp_rows(&repo, Some(&["u1".to_owned()]), "u", "c1").await;
        assert_eq!(names(&rows), vec!["user-server", "one-web-search"]);
    }

    /// The regression this module exists for: a conversation nobody created
    /// through the new-conversation screen — a scheduled task, a digital
    /// employee — has no snapshot, and used to get no media tool at all.
    #[tokio::test]
    async fn media_builtin_reaches_a_conversation_with_no_snapshot() {
        let repo = FakeRepo {
            rows: vec![
                row("u1", "user-server", true, false),
                row("b1", "one-image-generation", true, true),
                row("b2", "one-export-pdf", true, true),
            ],
        };
        let rows = load_session_mcp_rows(&repo, None, "u", "c1").await;
        assert_eq!(names(&rows), vec!["user-server", "one-image-generation"]);
    }

    /// …and equally when the conversation *does* carry a snapshot that never
    /// mentioned it, which is what the new-conversation screen writes.
    #[tokio::test]
    async fn media_builtin_reaches_a_conversation_whose_snapshot_omits_it() {
        let repo = FakeRepo {
            rows: vec![
                row("u1", "user-server", false, false),
                row("b1", "one-image-generation", true, true),
            ],
        };
        let rows = load_session_mcp_rows(&repo, Some(&["u1".to_owned()]), "u", "c1").await;
        assert_eq!(names(&rows), vec!["user-server", "one-image-generation"]);
    }

    /// Only this one built-in. Handing every agent the PDF exporter and the
    /// team knowledge base is a different decision, and not this one.
    #[tokio::test]
    async fn other_builtins_stay_out() {
        let repo = FakeRepo {
            rows: vec![
                row("b2", "one-export-pdf", true, true),
                row("b3", "one-team-knowledge", true, true),
            ],
        };
        assert!(load_session_mcp_rows(&repo, None, "u", "c1").await.is_empty());
        assert!(
            load_session_mcp_rows(&repo, Some(&["b2".to_owned()]), "u", "c1")
                .await
                .is_empty()
        );
    }

    /// The flag is the operator's switch: disabled means disabled, and a
    /// snapshot cannot drag a disabled media server back in.
    #[tokio::test]
    async fn a_disabled_media_builtin_is_not_injected() {
        let repo = FakeRepo {
            rows: vec![row("b1", "aionui-image-generation", false, true)],
        };
        assert!(load_session_mcp_rows(&repo, None, "u", "c1").await.is_empty());
        assert!(
            load_session_mcp_rows(&repo, Some(&["b1".to_owned()]), "u", "c1")
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn legacy_names_from_older_installs_still_count() {
        let repo = FakeRepo {
            rows: vec![row("b1", "AionUi Image Generation", true, true)],
        };
        let rows = load_session_mcp_rows(&repo, None, "u", "c1").await;
        assert_eq!(names(&rows), vec!["AionUi Image Generation"]);
    }

    /// A snapshot still decides the *user* servers: one that was globally
    /// disabled after the conversation was created keeps working, and one the
    /// snapshot never listed stays out even while enabled.
    #[tokio::test]
    async fn the_snapshot_still_governs_user_servers() {
        let repo = FakeRepo {
            rows: vec![
                row("u1", "pinned-though-disabled", false, false),
                row("u2", "enabled-but-not-in-snapshot", true, false),
            ],
        };
        let rows = load_session_mcp_rows(&repo, Some(&["u1".to_owned()]), "u", "c1").await;
        assert_eq!(names(&rows), vec!["pinned-though-disabled"]);
    }

    /// De-dup: ticking it explicitly must not produce two entries.
    #[tokio::test]
    async fn an_explicitly_ticked_media_builtin_appears_once() {
        let repo = FakeRepo {
            rows: vec![row("b1", "one-image-generation", true, true)],
        };
        let rows = load_session_mcp_rows(&repo, Some(&["b1".to_owned()]), "u", "c1").await;
        assert_eq!(names(&rows), vec!["one-image-generation"]);
    }
}
