//! Data-directory names, and the pre-rebrand names they replaced.
//!
//! The backend's on-disk layout still carried the upstream brand: the SQLite
//! catalog, the agent session store, the managed-process registry. Renaming them
//! is not a string edit — those paths ARE the user's data, and a backend that
//! looks for a name nothing on disk uses does not fail loudly. It creates the
//! file, finds it empty, and the user opens an app with no conversations.
//!
//! Two mechanisms, and they answer different questions.
//!
//! [`resolve_with_legacy`] decides which name to USE right now: the current
//! one, falling back to the legacy one when that is what exists. It is what
//! kept every pre-rebrand install working through the rename without touching
//! a byte.
//!
//! [`adopt_current_name`] decides what the name SHOULD be, once, at startup:
//! it renames a legacy path onto the current one. Aliasing alone was supposed
//! to be enough, on the reasoning that "there is no window in which a path is
//! ambiguous, because the legacy name is only ever chosen when the current one
//! is absent". That is true of the lookup and false of the outcome — an
//! install left on `aionui-backend.db` indefinitely is one stray empty
//! `one-backend.db` away from opening with no history at all, because the
//! current name always wins. Renaming ends the exposure; the fallback stays,
//! because a rename that cannot happen must still leave a working app.
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

/// What [`adopt_current_name`] did, for the caller to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptOutcome {
    /// Nothing to do: already on the current name, or neither name exists.
    AlreadyCurrent,
    /// The legacy name was renamed onto the current one.
    Adopted,
    /// Both names exist. Left alone — see the doc comment.
    Conflict,
    /// The rename failed. Left alone; `resolve_with_legacy` still finds the
    /// legacy name in place, so nothing is worse than before.
    Failed,
}

/// Bring a legacy-named path onto the current name, once, at startup.
///
/// Aliasing alone left every pre-rebrand install reading and writing
/// `aionui-backend.db` forever, and that is a file one stray empty
/// `one-backend.db` permanently hides: [`resolve_with_legacy`] prefers the
/// current name whenever it exists, so the moment anything creates one beside
/// a legacy catalog the user opens an app with none of their history in it.
/// Renaming closes that window instead of living with it.
///
/// Deliberately does nothing when BOTH names exist. That state means two
/// catalogs hold data and only a human knows which one matters; picking by
/// size or age would be a guess, and a wrong guess here is the user's entire
/// history. The caller logs it loudly and today's behaviour continues.
///
/// Must run before anything opens these paths — a directory or an open SQLite
/// file cannot be renamed on Windows.
///
/// Failure is not fatal by construction: if the rename does not happen, the
/// legacy name is still on disk and `resolve_with_legacy` still resolves to
/// it, which is exactly the behaviour that shipped before this existed.
pub fn adopt_current_name(parent: &Path, current: &str, legacy: &str) -> AdoptOutcome {
    let current_path = parent.join(current);
    let legacy_path = parent.join(legacy);
    if !legacy_path.exists() {
        return AdoptOutcome::AlreadyCurrent;
    }
    if current_path.exists() {
        return AdoptOutcome::Conflict;
    }
    if std::fs::rename(&legacy_path, &current_path).is_err() {
        return AdoptOutcome::Failed;
    }

    // SQLite keeps committed transactions in `-wal` until a checkpoint, and it
    // finds that file by the database's own name. Renaming the catalog without
    // it silently discards whatever had not been checkpointed — an unclean
    // shutdown's worth of the user's most recent work. Move it too, and put
    // the catalog back if that fails rather than leave the pair split.
    let wal_from = sidecar(&legacy_path, "-wal");
    if wal_from.exists() && std::fs::rename(&wal_from, sidecar(&current_path, "-wal")).is_err() {
        let _ = std::fs::rename(&current_path, &legacy_path);
        return AdoptOutcome::Failed;
    }
    // `-shm` is a rebuildable index into the WAL, not data. Moving it is nice;
    // failing to is harmless, and a stale one under the old name is ignored.
    let shm_from = sidecar(&legacy_path, "-shm");
    if shm_from.exists() {
        let _ = std::fs::rename(&shm_from, sidecar(&current_path, "-shm"));
    }
    AdoptOutcome::Adopted
}

/// `path` with `suffix` appended to its file name — SQLite's own convention
/// for `-wal` / `-shm`, which are `<db>-wal`, not `<db>.wal`.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
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

    // -- adopt_current_name --------------------------------------------------

    fn write(path: &std::path::Path, body: &str) {
        std::fs::write(path, body).unwrap();
    }

    /// The case every pre-rebrand install is in. The catalog keeps its
    /// contents and gains the current name, so nothing can hide it later.
    #[test]
    fn a_legacy_catalog_alone_is_renamed_onto_the_current_name() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(LEGACY_BACKEND_DB_NAME), "the user's history");

        assert_eq!(
            adopt_current_name(dir.path(), BACKEND_DB_NAME, LEGACY_BACKEND_DB_NAME),
            AdoptOutcome::Adopted
        );
        assert!(!dir.path().join(LEGACY_BACKEND_DB_NAME).exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(BACKEND_DB_NAME)).unwrap(),
            "the user's history"
        );
        // And the resolver now finds it under the current name.
        assert_eq!(backend_db_path(dir.path()), dir.path().join(BACKEND_DB_NAME));
    }

    /// SQLite keeps committed transactions in `-wal` until a checkpoint and
    /// finds that file by the database's own name. Leaving it behind would
    /// discard an unclean shutdown's worth of the user's most recent work.
    #[test]
    fn the_wal_travels_with_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(LEGACY_BACKEND_DB_NAME), "catalog");
        write(
            &dir.path().join(format!("{LEGACY_BACKEND_DB_NAME}-wal")),
            "uncheckpointed",
        );
        write(&dir.path().join(format!("{LEGACY_BACKEND_DB_NAME}-shm")), "index");

        assert_eq!(
            adopt_current_name(dir.path(), BACKEND_DB_NAME, LEGACY_BACKEND_DB_NAME),
            AdoptOutcome::Adopted
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(format!("{BACKEND_DB_NAME}-wal"))).unwrap(),
            "uncheckpointed"
        );
        assert!(!dir.path().join(format!("{LEGACY_BACKEND_DB_NAME}-wal")).exists());
        assert!(dir.path().join(format!("{BACKEND_DB_NAME}-shm")).exists());
    }

    /// Both names holding data is the one case where a guess costs the user
    /// everything. Nothing is moved, nothing is deleted, and resolution stays
    /// exactly what it was.
    #[test]
    fn two_catalogs_are_left_alone_rather_than_guessed_between() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(BACKEND_DB_NAME), "new");
        write(&dir.path().join(LEGACY_BACKEND_DB_NAME), "old");

        assert_eq!(
            adopt_current_name(dir.path(), BACKEND_DB_NAME, LEGACY_BACKEND_DB_NAME),
            AdoptOutcome::Conflict
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(BACKEND_DB_NAME)).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(LEGACY_BACKEND_DB_NAME)).unwrap(),
            "old"
        );
    }

    /// A fresh install, and an install that has already been through this.
    /// Both must be a no-op — in particular, no empty file is created.
    #[test]
    fn nothing_happens_when_there_is_no_legacy_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            adopt_current_name(dir.path(), BACKEND_DB_NAME, LEGACY_BACKEND_DB_NAME),
            AdoptOutcome::AlreadyCurrent
        );
        assert!(!dir.path().join(BACKEND_DB_NAME).exists());

        write(&dir.path().join(BACKEND_DB_NAME), "already current");
        assert_eq!(
            adopt_current_name(dir.path(), BACKEND_DB_NAME, LEGACY_BACKEND_DB_NAME),
            AdoptOutcome::AlreadyCurrent
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(BACKEND_DB_NAME)).unwrap(),
            "already current"
        );
    }

    /// Directories go the same way — the session store and the process
    /// registry are directories, not files, and carry no sidecars.
    #[test]
    fn a_legacy_directory_is_renamed_with_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(LEGACY_AGENT_SESSIONS_DIR);
        std::fs::create_dir_all(legacy.join("nested")).unwrap();
        write(&legacy.join("nested").join("session.json"), "a session");

        assert_eq!(
            adopt_current_name(dir.path(), AGENT_SESSIONS_DIR, LEGACY_AGENT_SESSIONS_DIR),
            AdoptOutcome::Adopted
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(AGENT_SESSIONS_DIR).join("nested").join("session.json")).unwrap(),
            "a session"
        );
    }
}
