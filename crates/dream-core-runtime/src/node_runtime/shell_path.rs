//! PATH fallback that lets agent shell commands reach the managed Node runtime.
//!
//! Bundled skills ship Node scripts (e.g. the deck renderer) and tell the model to
//! run `node …`. On a machine without a system Node, that command would fail even
//! though the app already carries a Node runtime for MCP servers. Appending the
//! managed runtime's directory — after every existing entry, so a user-installed
//! Node keeps winning — closes that gap without touching the backend's own env.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// PATH value for agent shells with the managed Node directory appended, or `None`
/// when no change is needed (`node` already resolves, or no managed runtime is
/// present on disk). Never downloads anything.
pub fn path_with_managed_node_fallback() -> Option<OsString> {
    if which::which("node").is_ok() {
        return None;
    }
    let managed_dir = super::managed::probe_preferred_local_runtime()
        .and_then(|runtime| runtime.node_path.parent().map(PathBuf::from));
    append_path_entry(std::env::var_os("PATH").as_deref(), managed_dir)
}

fn append_path_entry(current: Option<&OsStr>, dir: Option<PathBuf>) -> Option<OsString> {
    let dir = dir?;
    let mut entries: Vec<PathBuf> = current
        .map(|value| std::env::split_paths(value).collect())
        .unwrap_or_default();
    if entries.iter().any(|entry| entry == &dir) {
        return None;
    }
    entries.push(dir);
    std::env::join_paths(entries).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_managed_dir_after_existing_entries() {
        let current = std::env::join_paths([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap();
        let out = append_path_entry(Some(&current), Some(PathBuf::from("/opt/node/bin"))).unwrap();
        let entries: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(
            entries,
            vec![
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/opt/node/bin")
            ]
        );
    }

    #[test]
    fn no_change_without_managed_runtime() {
        assert_eq!(append_path_entry(Some(OsStr::new("/usr/bin")), None), None);
    }

    #[test]
    fn no_change_when_dir_already_present() {
        let current = std::env::join_paths([PathBuf::from("/opt/node/bin")]).unwrap();
        assert_eq!(
            append_path_entry(Some(&current), Some(PathBuf::from("/opt/node/bin"))),
            None
        );
    }

    #[test]
    fn works_with_empty_path() {
        let out = append_path_entry(None, Some(PathBuf::from("/opt/node/bin"))).unwrap();
        assert_eq!(
            std::env::split_paths(&out).collect::<Vec<_>>(),
            vec![PathBuf::from("/opt/node/bin")]
        );
    }
}
