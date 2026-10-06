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

use std::path::Path;

/// App-data subdir holding backend-managed state — the frontend's
/// `USERDATA_DATA_SUBDIR`. Releases before the `conversations/` layout
/// provisioned temp workspaces as its direct children.
const DATA_SUBDIR: &str = "1one";

/// Marker separating the agent label from the id in an auto-provisioned leaf:
/// `{label}-temp-{id}`. The id is a conversation id on current releases and a
/// unix-millis timestamp on older ones, so it is not interpreted here.
const AUTO_LEAF_MARKER: &str = "-temp-";

/// Whether `workspace` is shaped like a workspace the backend provisioned.
///
/// Recognises a `{label}-temp-{id}` leaf whose parent is an auto root:
///   - `conversations/{leaf}` (legacy userless)
///   - `conversations/{Y}/{M}/{D}/{leaf}` (dated)
///   - `conversations/users/{dir}/{Y}/{M}/{D}/{leaf}` (per-user, current)
///   - `{userData}/1one/{leaf}` (pre-`conversations/` releases)
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
    let mut segments = workspace.split(['/', '\\']).filter(|s| !s.is_empty()).rev();
    let Some(leaf) = segments.next() else {
        return false;
    };
    let is_auto_leaf = leaf
        .split_once(AUTO_LEAF_MARKER)
        .is_some_and(|(label, id)| !label.is_empty() && !id.is_empty());
    if !is_auto_leaf {
        return false;
    }
    let Some(parent) = segments.next() else {
        return false;
    };
    parent == DATA_SUBDIR || parent == "conversations" || segments.any(|segment| segment == "conversations")
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
        // Pre-`conversations/` layout, under an app name two renames ago, with
        // a unix-millis id. Real rows in this shape exist on upgraded installs.
        assert!(auto(
            r"C:\Users\alice\AppData\Roaming\1ONE ClaudeCode\1one\aionrs-temp-1776132219793"
        ));
        assert!(auto("/home/alice/.config/one-work/1one/dream-temp-9f2c"));
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
