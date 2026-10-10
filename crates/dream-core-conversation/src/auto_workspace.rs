//! Recognising workspaces this backend provisioned for a conversation.
//!
//! A conversation either runs in a directory the user picked (a "project") or
//! in a throwaway directory the backend provisioned for it. The distinction
//! drives two separate behaviours, which must agree:
//!   - the derived `is_temporary_workspace` flag the history sidebar groups by
//!     (see `convert`), and
//!   - whether a stored workspace that is gone at runtime is an error or is
//!     simply re-provisioned (see `session_context`).
//!
//! Keeping one recogniser for both is the point of this module: when they
//! disagreed, a chat filed under "Projects" in the sidebar also refused to
//! open, because the runtime treated the missing directory as a lost project.
//!
//! The naming rule lives here too ([`expected_auto_workspace_path`]), so what
//! we generate and what we recognise cannot drift apart: `service` and
//! `session_context` each used to carry their own verbatim copy of it.

use std::path::{Path, PathBuf};

use chrono::Datelike;
use dream_core_common::AgentType;

/// App-data subdir holding backend-managed state — the frontend's
/// `USERDATA_DATA_SUBDIR`. Releases before the `conversations/` layout
/// provisioned temp workspaces as its direct children.
const DATA_SUBDIR: &str = "1one";

/// Marker separating the agent label from the id in an auto-provisioned leaf:
/// `{label}-temp-{id}`. The id is a conversation id on current releases and a
/// unix-millis timestamp on older ones, so it is not interpreted here.
const AUTO_LEAF_MARKER: &str = "-temp-";

/// Subdir of an older generation that held named workspaces directly:
/// `{userData}/1one/workspaces/{label}-{unix_millis}`, with no `-temp-` in the
/// leaf. Nothing but the app writes there, so the leaf is not constrained —
/// a future naming scheme stays recognised.
const WORKSPACES_SUBDIR: &str = "workspaces";

/// Whether `workspace` is shaped like a workspace the backend provisioned.
///
/// Recognises a `{label}-temp-{id}` leaf whose parent is an auto root:
///   - `conversations/{leaf}` (legacy userless)
///   - `conversations/{Y}/{M}/{D}/{leaf}` (dated)
///   - `conversations/users/{dir}/{Y}/{M}/{D}/{leaf}` (per-user)
///   - `conversations/{Y-M-D}/{leaf}` and `conversations/users/{dir}/{Y-M-D}/{leaf}`
///     (current, see [`AutoWorkspaceLayout`])
///   - `{userData}/1one/{leaf}` (pre-`conversations/` releases)
///
/// plus any leaf directly inside `{userData}/1one/workspaces/`, the generation
/// between those two, whose leaf carries no `-temp-` marker at all.
///
/// This is deliberately independent of any root: the work dir is
/// user-configurable and the app-data root was renamed in 3.0.1, so a
/// workspace provisioned under a previous root — whose directory no longer
/// exists at all — must still be recognised as ours.
///
/// The parent must itself be an auto root, so an ordinary user directory that
/// merely holds a `-temp-` leaf (`~/work/dream-temp-1`) is not swept in:
/// misclassifying a real project as throwaway would silently replace it with
/// an empty directory instead of reporting that the project is gone.
///
/// Both separators are split on, so a Windows path stored in the DB is read
/// correctly on any host.
pub(crate) fn has_auto_workspace_structure(workspace: &str) -> bool {
    let workspace = workspace.trim();
    if workspace.is_empty() {
        return false;
    }
    let segments: Vec<&str> = workspace.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let Some((leaf, ancestors)) = segments.split_last() else {
        return false;
    };
    // A leaf with no parent segment proves nothing either way.
    let Some((parent, above)) = ancestors.split_last() else {
        return false;
    };

    // `{userData}/1one/workspaces/{leaf}` — the app owns that directory, so the
    // leaf's own shape is not required to match anything.
    if *parent == WORKSPACES_SUBDIR && above.last() == Some(&DATA_SUBDIR) {
        return true;
    }

    let is_auto_leaf = leaf
        .split_once(AUTO_LEAF_MARKER)
        .is_some_and(|(label, id)| !label.is_empty() && !id.is_empty());
    is_auto_leaf && (*parent == DATA_SUBDIR || *parent == "conversations" || above.contains(&"conversations"))
}

/// Whether `workspace` should be *reported* to the frontend as temporary.
///
/// Anything under the backend-managed `root` counts, on top of the structural
/// check — this is the long-standing meaning of the derived
/// `is_temporary_workspace` flag and of the frontend's `custom_workspace`.
///
/// Runtime decisions use [`has_auto_workspace_structure`] instead, without the
/// root prefix: a project the user picked *inside* the work dir is still their
/// project, and re-provisioning it over a missing directory would discard real
/// work. Being stricter at runtime errs toward "treat it as a project", which
/// is the safe direction.
pub(crate) fn is_reported_temporary_workspace(workspace: &str, root: &Path) -> bool {
    let trimmed = workspace.trim();
    if trimmed.is_empty() {
        return false;
    }
    Path::new(trimmed).starts_with(root) || has_auto_workspace_structure(trimmed)
}

/// How the auto workspaces under `{workspace_root}/conversations` are grouped.
///
/// The work dir is a folder the user picks and browses, so every level here is
/// one more click to reach a conversation's files. Releases up to 0.1.81 always
/// wrote `users/{user_dir}/{Y}/{M}/{D}/{leaf}` — six levels below
/// `conversations/`, two of which said nothing on a desktop install with one
/// user. Paths already stored keep working: every recogniser accepts both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoWorkspaceLayout {
    /// `users/{user_dir}/{Y-M-D}/{leaf}` — one directory per account, for a
    /// deployment several accounts sign in to.
    #[default]
    PerUser,
    /// `{Y-M-D}/{leaf}` — a single-user install, where an account directory
    /// isolates nothing and only adds a level.
    SingleUser,
}

/// Where this conversation's auto workspace belongs today:
/// `{workspace_root}/conversations/[users/{user_dir}/]{Y-M-D}/{label}-temp-{id}`.
///
/// The dated parent is built from the CURRENT date, so re-provisioning an old
/// conversation lands under today — [`has_auto_workspace_structure`] wildcards
/// the date segments precisely so that stays recognisable.
pub(crate) fn expected_auto_workspace_path(
    workspace_root: &Path,
    layout: AutoWorkspaceLayout,
    user_id: &str,
    conversation_id: &str,
    agent_type: &AgentType,
    backend: Option<&serde_json::Value>,
) -> PathBuf {
    auto_workspace_parent(workspace_root, layout, user_id).join(format!(
        "{}-temp-{conversation_id}",
        conversation_label(agent_type, backend)
    ))
}

/// The dated directory auto workspaces are created under today.
pub(crate) fn auto_workspace_parent(workspace_root: &Path, layout: AutoWorkspaceLayout, user_id: &str) -> PathBuf {
    let mut parent = workspace_root.join("conversations");
    if layout == AutoWorkspaceLayout::PerUser {
        let dir = dream_core_common::user_dir_name(user_id).unwrap_or_else(|_| user_id.to_owned());
        parent = parent.join("users").join(dir);
    }
    let now = chrono::Local::now();
    parent.join(format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day()))
}

/// How many date directories sit between the account level and the leaf of
/// `parts` — a path relative to `{workspace_root}/conversations`, leaf
/// included — or `None` when it is not a dated auto-workspace path.
///
/// Accepts the dated parents of every release: `{Y}/{M}/{D}` (3) and the
/// current `{Y-M-D}` (1), each optionally under `users/{user_dir}`. Callers
/// add their own rule for the leaf; deleting a conversation uses the depth to
/// prune the date directories it emptied, and nothing above them.
pub(crate) fn dated_parent_depth(parts: &[&str]) -> Option<usize> {
    let (_leaf, dirs) = parts.split_last()?;
    let dated = match dirs {
        ["users", _user_dir, rest @ ..] => rest,
        rest => rest,
    };
    let digits = |s: &str, len: usize| s.len() == len && s.chars().all(|ch| ch.is_ascii_digit());
    match dated {
        [year, month, day] if digits(year, 4) && digits(month, 2) && digits(day, 2) => Some(3),
        [date] => {
            let fields: Vec<&str> = date.split('-').collect();
            let ok = matches!(fields.as_slice(), [y, m, d] if digits(y, 4) && digits(m, 2) && digits(d, 2));
            ok.then_some(1)
        }
        _ => None,
    }
}

/// The leaf's label: an ACP conversation is named after its backend (`claude`,
/// `codex`, …), everything else after its agent type.
pub(crate) fn conversation_label(agent_type: &AgentType, backend: Option<&serde_json::Value>) -> String {
    if *agent_type == AgentType::Acp
        && let Some(serde_json::Value::String(s)) = backend
        && !s.is_empty()
    {
        return s.clone();
    }
    agent_type.serde_name().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/srv/one-data";

    fn auto(workspace: &str) -> bool {
        has_auto_workspace_structure(workspace)
    }

    fn reported(workspace: &str) -> bool {
        is_reported_temporary_workspace(workspace, Path::new(ROOT))
    }

    #[test]
    fn recognises_workspaces_under_the_current_root() {
        assert!(auto("/srv/one-data/conversations/users/u1/2026/10/06/dream-temp-abc"));
        assert!(reported(
            "/srv/one-data/conversations/users/u1/2026/10/06/dream-temp-abc"
        ));
        assert!(auto("/srv/one-data/conversations/2026-10-10/dream-temp-abc"));
        assert!(auto(
            r"D:\onework测试\conversations\users\u1\2026-10-10\claude-temp-abc"
        ));
    }

    #[test]
    fn the_single_user_layout_drops_the_account_level_and_folds_the_date() {
        let root = Path::new("/w");
        let agent = AgentType::DreamEngine;
        let single = expected_auto_workspace_path(root, AutoWorkspaceLayout::SingleUser, "u1", "c1", &agent, None);
        let per_user = expected_auto_workspace_path(root, AutoWorkspaceLayout::PerUser, "u1", "c1", &agent, None);

        let rel = |p: &Path| -> Vec<String> {
            p.strip_prefix("/w/conversations")
                .unwrap()
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect()
        };
        let single = rel(&single);
        let per_user = rel(&per_user);
        assert_eq!(single.len(), 2, "{single:?}");
        assert_eq!(single[1], "dream-temp-c1");
        assert_eq!(per_user.len(), 4, "{per_user:?}");
        assert_eq!(&per_user[..2], ["users", "u1"]);
        assert_eq!(per_user[2], single[0], "both use the same one-level date");

        // What we write is what we recognise.
        fn parts(v: &[String]) -> Vec<&str> {
            v.iter().map(String::as_str).collect()
        }
        assert_eq!(dated_parent_depth(&parts(&single)), Some(1));
        assert_eq!(dated_parent_depth(&parts(&per_user)), Some(1));
    }

    #[test]
    fn dated_parent_depth_covers_every_release_and_nothing_else() {
        assert_eq!(dated_parent_depth(&["2026", "10", "06", "x-temp-1"]), Some(3));
        assert_eq!(
            dated_parent_depth(&["users", "u", "2026", "10", "06", "x-temp-1"]),
            Some(3)
        );
        assert_eq!(dated_parent_depth(&["2026-10-06", "x-temp-1"]), Some(1));
        assert_eq!(dated_parent_depth(&["users", "u", "2026-10-06", "x-temp-1"]), Some(1));

        assert_eq!(dated_parent_depth(&["x-temp-1"]), None);
        assert_eq!(dated_parent_depth(&["2026-10", "x-temp-1"]), None);
        assert_eq!(dated_parent_depth(&["2026-10-06-01", "x-temp-1"]), None);
        assert_eq!(dated_parent_depth(&["notes", "x-temp-1"]), None);
        assert_eq!(dated_parent_depth(&["users", "u", "x-temp-1"]), None);
        assert_eq!(dated_parent_depth(&["YYYY", "MM", "DD", "x-temp-1"]), None);
    }

    #[test]
    fn a_project_inside_the_work_dir_is_reported_temporary_but_is_not_structurally_auto() {
        // The runtime must keep treating it as the user's project: it holds
        // real work, so a missing directory is an error, not something to
        // silently re-provision.
        assert!(!auto("/srv/one-data/custom-workspace"));
        assert!(reported("/srv/one-data/custom-workspace"));
    }

    #[test]
    fn recognises_workspaces_provisioned_under_a_previous_root() {
        // Dated and per-user layouts, no longer under the current root.
        assert!(auto("/old-root/conversations/2026/09/30/claude-temp-abc"));
        assert!(auto("/old-root/conversations/team-temp-t1"));
        assert!(auto("/old-root/conversations/users/u1/2026/09/30/dream-temp-fd4d9bd2"));
        // Pre-`conversations/` layout, with a unix-millis id. The pre-rebrand
        // app-data root and the `aionrs` label are quoted deliberately: these
        // are legacy values read back from rows that upgraded installs still
        // carry, so renaming them here would stop testing the real data.
        assert!(auto(
            r"C:\Users\alice\AppData\Roaming\1ONE ClaudeCode\1one\aionrs-temp-1776132219793"
        ));
        assert!(auto("/home/alice/.config/one-work/1one/dream-temp-9f2c"));
        // The `workspaces/` generation: `{label}-{unix_millis}`, no `-temp-`;
        // same legacy root, same reason.
        assert!(auto(
            r"C:\Users\alice\AppData\Roaming\1ONE ClaudeCode\1one\workspaces\aionrs-1782784235220"
        ));
        assert!(auto(r"C:\x\1one\workspaces\claude-1783146802117"));
    }

    #[test]
    fn a_workspaces_dir_outside_the_app_data_subdir_is_not_auto() {
        // Only `{userData}/1one/workspaces/` is ours. A user directory that
        // happens to be called `workspaces` is not.
        assert!(!auto("/Users/alice/workspaces/my-project"));
        assert!(!auto("/Users/alice/code/workspaces/client-site"));
        // `1one` must be the workspaces dir's own parent, not just an ancestor.
        assert!(!auto("/Users/alice/1one/projects/workspaces/thing"));
    }

    #[test]
    fn keeps_user_directories_non_auto() {
        assert!(!auto(""));
        assert!(!auto("   "));
        assert!(!auto("/Users/alice/my-project"));
        assert!(!auto("/Users/alice/projects/conversations"));
        // A `-temp-` leaf alone proves nothing: the parent must be an auto root.
        assert!(!auto("/Users/alice/my-temp-notes"));
        assert!(!auto("/Users/alice/work/dream-temp-1"));
        assert!(!auto("/Users/alice/1one-backup/dream-temp-1"));
        // Leaf with an empty label or id, and a leaf with no parent at all.
        assert!(!auto("/Users/alice/conversations/-temp-"));
        assert!(!auto("/Users/alice/1one/-temp-x"));
        assert!(!auto("dream-temp-1"));
    }
}
