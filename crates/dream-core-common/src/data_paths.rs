//! Data-directory names, and the pre-rebrand names they replaced.
//!
//! The backend's on-disk layout still carried the upstream brand: the SQLite
//! catalog, the agent session store, the managed-process registry. Renaming them
//! is not a string edit — those paths ARE the user's data, and a backend that
//! looks for a name nothing on disk uses does not fail loudly. It creates the
//! file, finds it empty, and the user opens an app with no conversations.
//!
//! So nothing is moved. [`resolve_with_legacy`] prefers the current name and
//! falls back to the legacy one when that is what actually exists, which makes
//! the rename an aliasing change rather than a migration: an existing install
//! keeps reading and writing the exact files it always has, and a fresh install
//! gets the new names. There is no window in which a path is ambiguous, because
//! the legacy name is only ever chosen when the current one is absent.
//!
//! The deliberate exceptions live elsewhere and must stay: the packaged app id,
//! the `1ONE Code` userData folder and the `aionui://` deep-link scheme are
//! frozen historical values (see dream-ui's `PROD_USERDATA_APP_NAME` and
//! `electron-builder.yml`), because changing those strands the whole data
//! directory rather than one file inside it.

use std::path::{Path, PathBuf};

/// Backend SQLite catalog.
pub const BACKEND_DB_NAME: &str = "one-backend.db";
pub const LEGACY_BACKEND_DB_NAME: &str = "aionui-backend.db";

/// Per-agent session store, under the data directory.
pub const AGENT_SESSIONS_DIR: &str = "one-sessions";
pub const LEGACY_AGENT_SESSIONS_DIR: &str = "aionrs-sessions";

/// Managed-process registry, under the runtime directory.
pub const PROCESS_REGISTRY_DIR: &str = "one-process";
pub const LEGACY_PROCESS_REGISTRY_DIR: &str = "aionui-process";

/// Pick the name to use inside `parent`: the current one, unless only the legacy
/// one is present on disk.
///
/// A fresh install has neither and gets the current name. An install that has
/// both — a half-finished manual rename, or two versions run side by side —
/// gets the current one, so the newer layout wins rather than the older.
pub fn resolve_with_legacy(parent: &Path, current: &str, legacy: &str) -> PathBuf {
    let current_path = parent.join(current);
    if current_path.exists() {
        return current_path;
    }
    let legacy_path = parent.join(legacy);
    if legacy_path.exists() {
        return legacy_path;
    }
    current_path
}

/// Path to the backend SQLite catalog inside `data_dir`.
pub fn backend_db_path(data_dir: &Path) -> PathBuf {
    resolve_with_legacy(data_dir, BACKEND_DB_NAME, LEGACY_BACKEND_DB_NAME)
}

/// Path to the agent session store inside `data_dir`.
pub fn agent_sessions_dir(data_dir: &Path) -> PathBuf {
    resolve_with_legacy(data_dir, AGENT_SESSIONS_DIR, LEGACY_AGENT_SESSIONS_DIR)
}

/// Path to the managed-process registry inside `runtime_dir`.
pub fn process_registry_dir(runtime_dir: &Path) -> PathBuf {
    resolve_with_legacy(runtime_dir, PROCESS_REGISTRY_DIR, LEGACY_PROCESS_REGISTRY_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this module exists to prevent. An upgrade must not point the
    /// backend at a name nothing on disk uses — SQLite would create it, the
    /// catalog would come up empty, and the user's conversations, assistants and
    /// skills would all appear to be gone.
    #[test]
    fn an_existing_install_keeps_using_the_file_it_already_has() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LEGACY_BACKEND_DB_NAME), b"existing catalog").unwrap();

        assert_eq!(backend_db_path(dir.path()), dir.path().join(LEGACY_BACKEND_DB_NAME));
    }

    #[test]
    fn a_fresh_install_gets_the_current_name() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(backend_db_path(dir.path()), dir.path().join(BACKEND_DB_NAME));
    }

    /// With both present the newer layout wins — otherwise an install that had
    /// been migrated would silently fall back the moment a stale legacy file
    /// reappeared next to it.
    #[test]
    fn the_current_name_wins_when_both_exist() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(BACKEND_DB_NAME), b"new").unwrap();
        std::fs::write(dir.path().join(LEGACY_BACKEND_DB_NAME), b"old").unwrap();

        assert_eq!(backend_db_path(dir.path()), dir.path().join(BACKEND_DB_NAME));
    }

    /// Directories resolve the same way files do — the session store and the
    /// process registry are directories, and an install that keeps writing into
    /// the legacy one must keep finding it.
    #[test]
    fn a_legacy_directory_is_found_the_same_way_a_legacy_file_is() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(LEGACY_AGENT_SESSIONS_DIR)).unwrap();
        std::fs::create_dir(dir.path().join(LEGACY_PROCESS_REGISTRY_DIR)).unwrap();

        assert_eq!(
            agent_sessions_dir(dir.path()),
            dir.path().join(LEGACY_AGENT_SESSIONS_DIR)
        );
        assert_eq!(
            process_registry_dir(dir.path()),
            dir.path().join(LEGACY_PROCESS_REGISTRY_DIR)
        );
    }

    #[test]
    fn fresh_directories_use_the_current_names() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(agent_sessions_dir(dir.path()), dir.path().join(AGENT_SESSIONS_DIR));
        assert_eq!(process_registry_dir(dir.path()), dir.path().join(PROCESS_REGISTRY_DIR));
    }
}
