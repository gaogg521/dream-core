//! Whether a workspace path is one the backend provisioned for a conversation.
//!
//! Lives in the foundation crate because both the conversation layer (which provisions and
//! re-provisions these directories) and the agent layer (which decides, for example, that a
//! conversation without a project of its own shares one user-level memory) have to agree on it.

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
///     (current, see `AutoWorkspaceLayout` in the conversation crate)
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
pub fn has_auto_workspace_structure(workspace: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::has_auto_workspace_structure;

    #[test]
    fn a_provisioned_conversation_directory_is_recognised_on_either_separator() {
        let windows = [
            "C:",
            "Users",
            "me",
            "work",
            "conversations",
            "2026-10-29",
            "dream-temp-4c69a9e9",
        ]
        .join(&char::from(92).to_string());
        assert!(has_auto_workspace_structure(&windows));
        assert!(has_auto_workspace_structure(
            "/home/me/work/conversations/users/u1/2026-10-29/dream-temp-4c69a9e9"
        ));
    }

    #[test]
    fn a_project_the_person_chose_is_not_mistaken_for_one() {
        assert!(!has_auto_workspace_structure("/home/me/work/website"));
        // A `-temp-` leaf alone proves nothing: its parent has to be one of ours.
        assert!(!has_auto_workspace_structure("/home/me/work/dream-temp-1"));
        assert!(!has_auto_workspace_structure(""));
    }
}
