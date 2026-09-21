//! Personal-edition backup and restore of the local install.
//!
//! The enterprise bundle in `dream-domain-org::backup` answers a different
//! question and deliberately excludes conversations and messages: it captures
//! an organization's configuration, and replaying someone's chat history into
//! another deployment would be a privacy problem rather than a feature.
//!
//! Personal backup is the inverse. A user moving to a new machine wants
//! precisely that history — conversations, messages, providers, skills — and
//! without this has no way to carry any of it across.
//!
//! # Selective by category, in both directions
//!
//! Conversations, provider wiring and skills all live in the same SQLite
//! catalog, so "export only my conversations" cannot be served by copying the
//! file. Two rules follow, and they are load-bearing:
//!
//! * **Export really deletes what was not selected.** Recording the choice in
//!   the manifest and shipping the whole catalog would hand someone who asked
//!   for conversations a file with every API key in it.
//! * **Restore merges, it does not replace.** A catalog that was exported with
//!   only conversations has an empty `providers` table; restoring it wholesale
//!   would wipe the model configuration on the target machine. Each selected
//!   category is merged into what is already there and the rest is untouched.
//!
//! Merging also means a restore needs no restart — nothing swaps the live
//! catalog out from under the running pool.
//!
//! # Why the catalog is copied with `VACUUM INTO`
//!
//! SQLite keeps committed transactions in `-wal` until a checkpoint. Copying
//! the `.db` file alone silently drops everything not yet checkpointed — the
//! same trap [`dream_core_common::adopt_current_name`] documents for renames.
//! `VACUUM INTO` writes a fully checkpointed, internally consistent copy while
//! the app keeps running.
//!
//! # The archive is a credential whenever it carries providers
//!
//! `users.jwt_secret` travels with every backup (a restore needs the identity
//! the rows belong to), and the provider API keys are encrypted with a key
//! derived from it by a pure function (`derive_encryption_key`). An archive
//! that includes the provider category therefore contains usable keys. The API
//! reports this per archive so the UI can say so rather than bury it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use dream_core_db::DbPool;

use crate::backup_crypto::{ArchiveEncryption, ArchiveKey};
use crate::error::SystemError;

/// Archive format version. A restore refuses anything it does not recognize
/// rather than half-applying it.
///
/// 3 adds passphrase encryption, which every new archive now carries. The bump
/// is what makes an older build refuse such a file cleanly instead of unpacking
/// a `catalog.db` full of ciphertext and reporting a corrupt database. Version
/// 2 archives — written before this existed — still restore.
pub const BACKUP_FORMAT_VERSION: u32 = 3;

const MANIFEST_NAME: &str = "manifest.json";

/// Where the catalog copy lives inside the archive. A fixed name rather than
/// the on-disk one: an install still on the pre-rebrand `aionui-backend.db`
/// must restore onto a machine using the current name, and the reverse.
const ARCHIVE_DB_NAME: &str = "catalog.db";

/// Prefix for copied data-directory subtrees inside the archive.
const ARCHIVE_FILES_PREFIX: &str = "files/";

/// Identity rows. Always carried, never deleted by a category filter: every
/// other table is scoped by `user_id`, and restoring rows whose owner is absent
/// would leave them unreachable.
const IDENTITY_TABLES: &[&str] = &["users"];

/// Conversations, their messages, and the project/folder structure they hang
/// off. `acp_session` is the per-conversation agent session row — without it a
/// restored bridged conversation cannot resume.
const CONVERSATION_TABLES: &[&str] = &[
    "conversations",
    "messages",
    "conversation_artifacts",
    "conversation_assistant_snapshots",
    "acp_session",
    "projects",
    "folders",
    "project_explorer",
];

/// Model wiring: which providers exist, how they authenticate, and the bridge
/// and MCP endpoints that hang off them.
const PROVIDER_TABLES: &[&str] = &[
    "providers",
    "claude_bridge_config",
    "codex_bridge_config",
    "mcp_servers",
    "oauth_tokens",
    "remote_agents",
    "agent_metadata",
];

const SKILL_TABLES: &[&str] = &["skills", "skill_import_records"];

/// Assistants, teams, scheduled jobs and preferences — everything the user
/// configured that is not a provider or a skill.
const APP_SETTINGS_TABLES: &[&str] = &[
    "assistants",
    "assistant_definitions",
    "assistant_overrides",
    "assistant_overlays",
    "assistant_preferences",
    "assistant_plugins",
    "assistant_sessions",
    "assistant_users",
    "assistant_marketplace_personas",
    "teams",
    "team_tasks",
    "cron_jobs",
    "cron_job_runs",
    "mailbox",
    "system_settings",
    "client_preferences",
];

/// Skill files on disk, alongside [`SKILL_TABLES`].
const SKILL_DIRS: &[&str] = &["skills"];

/// Assistant resources on disk, alongside [`APP_SETTINGS_TABLES`].
const APP_SETTINGS_DIRS: &[&str] = &["assistant-avatars", "assistant-rules"];

const APP_SETTINGS_FILES: &[&str] = &["extension-states.json", "extension-user-states.json"];

/// Conversation workspaces: every file an agent read or produced. Its own
/// category because this subtree alone can reach gigabytes.
const ATTACHMENT_DIRS: &[&str] = &["conversations"];

/// What a backup carries.
///
/// All false is rejected rather than producing an archive with nothing in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupScope {
    /// Conversations and their messages.
    #[serde(default)]
    pub conversations: bool,
    /// Files agents read or produced, under the conversation workspaces.
    #[serde(default)]
    pub attachments: bool,
    /// Providers, bridges and MCP servers. An archive with this set contains
    /// usable API keys — see the module docs.
    #[serde(default)]
    pub providers: bool,
    /// Installed and authored skills, with their files.
    #[serde(default)]
    pub skills: bool,
    /// Assistants, teams, scheduled jobs and preferences.
    #[serde(default)]
    pub app_settings: bool,
}

impl BackupScope {
    /// Everything — what "move me to a new machine" means.
    pub fn all() -> Self {
        Self {
            conversations: true,
            attachments: true,
            providers: true,
            skills: true,
            app_settings: true,
        }
    }

    pub fn is_empty(&self) -> bool {
        !(self.conversations || self.attachments || self.providers || self.skills || self.app_settings)
    }

    /// Catalog tables this scope covers, identity rows included.
    fn tables(&self) -> Vec<&'static str> {
        let mut tables = IDENTITY_TABLES.to_vec();
        if self.conversations {
            tables.extend_from_slice(CONVERSATION_TABLES);
        }
        if self.providers {
            tables.extend_from_slice(PROVIDER_TABLES);
        }
        if self.skills {
            tables.extend_from_slice(SKILL_TABLES);
        }
        if self.app_settings {
            tables.extend_from_slice(APP_SETTINGS_TABLES);
        }
        tables
    }

    /// Data-directory subtrees this scope covers.
    fn dirs(&self) -> Vec<&'static str> {
        let mut dirs = Vec::new();
        if self.attachments {
            dirs.extend_from_slice(ATTACHMENT_DIRS);
        }
        if self.skills {
            dirs.extend_from_slice(SKILL_DIRS);
        }
        if self.app_settings {
            dirs.extend_from_slice(APP_SETTINGS_DIRS);
        }
        dirs
    }

    fn files(&self) -> Vec<&'static str> {
        if self.app_settings {
            APP_SETTINGS_FILES.to_vec()
        } else {
            Vec::new()
        }
    }
}

/// Every table a backup could ever carry, for the export-side filter.
fn all_known_tables() -> Vec<&'static str> {
    let mut tables = Vec::new();
    tables.extend_from_slice(CONVERSATION_TABLES);
    tables.extend_from_slice(PROVIDER_TABLES);
    tables.extend_from_slice(SKILL_TABLES);
    tables.extend_from_slice(APP_SETTINGS_TABLES);
    tables
}

/// Describes an archive, for the restore path and for the user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format_version: u32,
    /// Epoch millis the export ran.
    pub exported_at: i64,
    /// App version that produced it, so a restore can refuse a downgrade.
    pub app_version: String,
    /// What the archive actually contains.
    pub scope: BackupScope,
    /// Uncompressed size of everything in the archive.
    pub total_bytes: u64,
    /// True when the archive carries decryptable provider credentials, so the
    /// UI can warn about where the file is stored.
    pub contains_credentials: bool,
    /// How the payload is encrypted, or `None` for a version 2 archive written
    /// before encryption existed.
    ///
    /// Lives in the manifest, which is the one part left in the clear: a person
    /// can see what an archive holds and when it was made without producing the
    /// passphrase, and only the data itself needs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<ArchiveEncryption>,
}

/// Outcome of a restore, per category.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    /// Rows merged into the live catalog, by table.
    pub rows_by_table: std::collections::BTreeMap<String, u64>,
    /// Files copied out of the archive.
    pub files_restored: u64,
    /// Rows dropped because the parent they referenced was not restored —
    /// what a partial scope costs, reported rather than left to be discovered.
    pub orphans_removed: std::collections::BTreeMap<String, u64>,
}

/// Backup and restore over a data directory.
#[derive(Clone)]
pub struct BackupService {
    data_dir: PathBuf,
    app_version: String,
}

impl BackupService {
    pub fn new(data_dir: PathBuf, app_version: String) -> Self {
        Self { data_dir, app_version }
    }

    /// Write a backup archive to `destination`.
    ///
    /// `pool` must be the pool serving this data directory. A MySQL pool is
    /// rejected: that is the enterprise server deployment, which has its own
    /// backup path and whose catalog is not a file in this directory.
    /// `passphrase` is required, not optional. Every archive carries
    /// `users.jwt_secret` whatever the scope, so there is no category that is
    /// safe to write in the clear — and leaving the choice to the person
    /// exporting means the one time they skip it is the time it matters.
    pub async fn export(
        &self,
        pool: &DbPool,
        destination: &Path,
        scope: BackupScope,
        passphrase: &str,
    ) -> Result<BackupManifest, SystemError> {
        if scope.is_empty() {
            return Err(SystemError::BadRequest(
                "Choose at least one kind of data to back up.".to_owned(),
            ));
        }
        let DbPool::Sqlite(_) = pool else {
            return Err(SystemError::BadRequest(
                "Backup is available for the local catalog only; this deployment uses a server database.".to_owned(),
            ));
        };

        let staging = tempfile::tempdir()
            .map_err(|error| SystemError::Internal(format!("Could not create a staging directory: {error}")))?;
        let catalog_copy = staging.path().join(ARCHIVE_DB_NAME);
        copy_catalog(pool, &catalog_copy).await?;
        prune_catalog_to_scope(&catalog_copy, scope).await?;

        let mut entries = vec![(ARCHIVE_DB_NAME.to_owned(), catalog_copy.clone())];
        for name in scope.files() {
            let path = self.data_dir.join(name);
            if path.is_file() {
                entries.push((format!("{ARCHIVE_FILES_PREFIX}{name}"), path));
            }
        }
        for dir in scope.dirs() {
            let root = self.data_dir.join(dir);
            if !root.is_dir() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&root).follow_links(false) {
                let entry = entry
                    .map_err(|error| SystemError::Internal(format!("Could not read {dir} for backup: {error}")))?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(&self.data_dir)
                    .map_err(|_| SystemError::Internal("Backup walked outside the data directory.".to_owned()))?;
                entries.push((
                    format!("{ARCHIVE_FILES_PREFIX}{}", to_archive_path(relative)),
                    entry.path().to_path_buf(),
                ));
            }
        }

        let total_bytes = entries
            .iter()
            .filter_map(|(_, path)| std::fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum();
        // Derived before anything is written, so a passphrase that is too
        // short fails here rather than after the archive exists.
        let key = ArchiveKey::new(passphrase)?;
        let manifest = BackupManifest {
            format_version: BACKUP_FORMAT_VERSION,
            exported_at: now_ms(),
            app_version: self.app_version.clone(),
            scope,
            total_bytes,
            contains_credentials: scope.providers,
            encryption: Some(key.encryption().clone()),
        };

        // Compressing runs on the blocking pool, not an async worker.
        //
        // This is not a formality: a real export was 13,304 files / 228 MB and
        // took about half an hour, all of it inside one synchronous call. Left
        // on an async worker that is one of tokio's few threads pinned for the
        // whole run, and every other request contends for what is left.
        let manifest_for_write = manifest.clone();
        let destination = destination.to_path_buf();
        tokio::task::spawn_blocking(move || write_archive(&destination, &manifest_for_write, &entries, Some(&key)))
            .await
            .map_err(|error| SystemError::Internal(format!("The backup task could not be run: {error}")))??;
        Ok(manifest)
    }

    /// Read an archive's manifest without applying anything.
    pub fn preview(&self, archive: &Path) -> Result<BackupManifest, SystemError> {
        let manifest = read_manifest(archive)?;
        if manifest.format_version > BACKUP_FORMAT_VERSION {
            return Err(SystemError::BadRequest(format!(
                "This backup uses format version {}, which this version of the app cannot read. Update the app first.",
                manifest.format_version
            )));
        }
        Ok(manifest)
    }

    /// Merge an archive into the live install.
    ///
    /// `scope` narrows what is applied; categories the archive does not carry
    /// are ignored. Nothing outside the selected categories is touched, so a
    /// conversations-only archive cannot wipe the provider configuration.
    /// `passphrase` opens an encrypted archive. A version 2 archive, written
    /// before encryption existed, ignores it and restores as before.
    pub async fn restore(
        &self,
        pool: &DbPool,
        archive: &Path,
        requested: BackupScope,
        passphrase: &str,
    ) -> Result<RestoreOutcome, SystemError> {
        let DbPool::Sqlite(live) = pool else {
            return Err(SystemError::BadRequest(
                "Restore is available for the local catalog only; this deployment uses a server database.".to_owned(),
            ));
        };
        let manifest = self.preview(archive)?;
        if is_newer_version(&manifest.app_version, &self.app_version) {
            return Err(SystemError::BadRequest(format!(
                "This backup was made by version {} and cannot be restored into version {}. Update the app first.",
                manifest.app_version, self.app_version
            )));
        }

        // Only what both the archive holds and the caller asked for.
        let effective = intersect(manifest.scope, requested);
        if effective.is_empty() {
            return Err(SystemError::BadRequest(
                "This backup does not contain any of the selected kinds of data.".to_owned(),
            ));
        }

        // The passphrase is checked against the manifest's verifier before a
        // single byte is unpacked, so a mistyped one says so instead of
        // failing later as a damaged catalog.
        let key = match manifest.encryption.as_ref() {
            Some(encryption) => Some(ArchiveKey::reopen(passphrase, encryption)?),
            None => None,
        };

        let staging = tempfile::tempdir()
            .map_err(|error| SystemError::Internal(format!("Could not create a staging directory: {error}")))?;
        unpack_archive(archive, staging.path(), key.as_ref())?;

        let mut outcome = RestoreOutcome::default();
        let staged_catalog = staging.path().join(ARCHIVE_DB_NAME);
        if staged_catalog.is_file() {
            let merged = merge_catalog(live, &staged_catalog, effective).await?;
            outcome.rows_by_table = merged.rows_by_table;
            outcome.orphans_removed = merged.orphans_removed;
        }

        let staged_files = staging.path().join("files");
        if staged_files.is_dir() {
            outcome.files_restored = copy_tree(&staged_files, &self.data_dir, effective)?;
        }
        Ok(outcome)
    }
}

/// Copy the catalog into `destination` as a consistent, checkpointed file.
async fn copy_catalog(pool: &DbPool, destination: &Path) -> Result<(), SystemError> {
    // `VACUUM INTO` refuses to overwrite, and tempdir hands us a fresh
    // directory, so there is nothing to remove first.
    let target = destination.to_string_lossy().into_owned();
    pool.execute("VACUUM INTO ?", &[dream_core_db::DbValue::from(target)])
        .await
        .map_err(|error| SystemError::Internal(format!("Could not copy the catalog for backup: {error}")))?;
    Ok(())
}

/// Empty every table outside `scope`, then reclaim the space.
///
/// Deleting rather than filtering on the way out keeps the schema intact, so a
/// restore never meets a missing table. Foreign keys stay off for the duration:
/// `conversations` cascades from `users`, and a cascade firing mid-prune would
/// delete rows the scope asked to keep.
async fn prune_catalog_to_scope(catalog: &Path, scope: BackupScope) -> Result<(), SystemError> {
    let pool = open_sqlite(catalog).await?;
    // One connection throughout: `PRAGMA foreign_keys` is per-connection, so
    // issuing it against the pool would leave the DELETEs running with foreign
    // keys still on — and the cascade from `users` would then delete rows the
    // scope asked to keep.
    let mut conn = pool
        .acquire()
        .await
        .map_err(|error| SystemError::Internal(format!("Could not open the backup catalog: {error}")))?;
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not prepare the backup catalog: {error}")))?;

    let keep = scope.tables();
    for table in all_known_tables() {
        if keep.contains(&table) || !table_exists(&mut conn, table).await? {
            continue;
        }
        sqlx::query(&format!("DELETE FROM \"{table}\""))
            .execute(&mut *conn)
            .await
            .map_err(|error| SystemError::Internal(format!("Could not trim {table} from the backup: {error}")))?;
    }

    sqlx::query("VACUUM")
        .execute(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not compact the backup catalog: {error}")))?;
    drop(conn);
    pool.close().await;
    Ok(())
}

/// Merge the archive's catalog into the live one, table by table.
///
/// Everything runs on ONE connection taken out of the pool, and that is not a
/// detail: `ATTACH DATABASE` is per-connection state. Issued against the pool,
/// the attach lands on whichever connection is free and the next query — on a
/// different one of the five — cannot see `backup` at all. The failure is a
/// plain "no such table", and a pool of one connection (as in a test) hides it
/// completely.
async fn merge_catalog(live: &SqlitePool, staged: &Path, scope: BackupScope) -> Result<MergeOutcome, SystemError> {
    let mut conn = live
        .acquire()
        .await
        .map_err(|error| SystemError::Internal(format!("Could not open the catalog for restore: {error}")))?;

    // Foreign keys OFF for the merge, exactly as the export path does, and for
    // a sharper reason.
    //
    // Rows go in grouped by category, and that order is not a topological one:
    // `conversation_assistant_snapshots` is restored with the conversations
    // while the `assistant_definitions` it points at belong to app settings and
    // arrive last. On the machine the archive came from those parent rows are
    // already present and nothing complains. On a NEW machine they are not —
    // which is why "export here, restore there" failed with a bare 500 while
    // restoring onto the source machine looked fine.
    //
    // Reimposing referential checks row by row would only be asking the merge
    // to arrive in an order the category grouping cannot express. The archive
    // is internally consistent; what it can be missing is a parent the user
    // chose NOT to restore, and `sweep_orphans` deals with that once the whole
    // merge has landed.
    //
    // The placement matters twice: BEFORE the transaction, because
    // `PRAGMA foreign_keys` is a documented no-op inside one and issued after
    // BEGIN would silently do nothing; and restored before the connection goes
    // back to the pool, or every later query on it would run unchecked.
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not prepare the catalog for restore: {error}")))?;

    let staged_path = staged.to_string_lossy().replace('\'', "''");
    let attached = sqlx::query(&format!("ATTACH DATABASE '{staged_path}' AS backup"))
        .execute(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not open the backup catalog: {error}")));

    let result = match attached {
        Err(error) => Err(error),
        Ok(_) => merge_within_transaction(&mut conn, scope).await,
    };

    // Detach even when the merge failed: the connection goes back to the pool
    // either way, and an attached database left on it would make the next
    // restore's ATTACH fail with "database backup is already in use".
    let _ = sqlx::query("DETACH DATABASE backup").execute(&mut *conn).await;
    // Never hand a connection back with enforcement switched off.
    let _ = sqlx::query("PRAGMA foreign_keys=ON").execute(&mut *conn).await;
    result
}

/// What a merge changed.
#[derive(Debug, Default)]
pub struct MergeOutcome {
    pub rows_by_table: std::collections::BTreeMap<String, u64>,
    /// Rows dropped because the parent they referenced was not restored.
    pub orphans_removed: std::collections::BTreeMap<String, u64>,
}

/// The merge and its cleanup, as one transaction.
///
/// Without this a failure left the catalog half-merged — measured on real data,
/// the old code committed `users`, `conversations` and `messages` and then
/// aborted, so the target ended up holding conversations and nothing else, with
/// nothing to say the rest never arrived.
async fn merge_within_transaction(
    conn: &mut sqlx::SqliteConnection,
    scope: BackupScope,
) -> Result<MergeOutcome, SystemError> {
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not begin the restore: {error}")))?;

    // Read before the merge overwrites it: this is the key the target machine
    // is actually running with, and the running process will not re-read it.
    let local_secret = read_data_secret(&mut *conn, "main").await.unwrap_or_default();
    let archive_secret = read_data_secret(&mut *conn, "backup").await.unwrap_or_default();

    // Read while the local ids still exist: the merge is about to delete some of
    // them, and the rows that pointed at them have to be told where they went.
    let remap = match plan_reference_remap(&mut *conn, &scope.tables()).await {
        Ok(plan) => plan,
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(error);
        }
    };

    let merged = match merge_tables(&mut *conn, scope).await {
        Ok(merged) => merged,
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(error);
        }
    };

    match apply_reference_remap(&mut *conn, &remap).await {
        Ok(moved) if moved > 0 => tracing::info!(
            moved,
            replaced = remap.len(),
            "re-pointed local rows at the records the restore merged over them"
        ),
        Ok(_) => {}
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(error);
        }
    }

    // The merge just replaced the identity row, and with it the secret every
    // stored credential on this machine is sealed with. Put the local one back
    // and re-seal what the archive brought, so nothing needs a restart and
    // nothing the target already had is lost.
    if !local_secret.is_empty() && local_secret != archive_secret {
        if let Err(error) = restore_local_data_secret(&mut *conn, &local_secret).await {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(error);
        }
        match rekey_restored_secrets(&mut *conn, &archive_secret, &local_secret).await {
            Ok(count) if count > 0 => tracing::info!(count, "re-keyed restored credentials to this install"),
            Ok(_) => {}
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Err(error);
            }
        }
    }

    let orphans_removed = match sweep_orphans(&mut *conn).await {
        Ok(orphans) => orphans,
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(error);
        }
    };

    sqlx::query("COMMIT")
        .execute(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not commit the restore: {error}")))?;

    Ok(MergeOutcome {
        rows_by_table: merged,
        orphans_removed,
    })
}

/// Carries children over when a merge replaced the row they pointed at.
///
/// `INSERT OR REPLACE` resolves a UNIQUE conflict by DELETING the local row and
/// inserting the archive's. Two machines that both seeded the same builtin
/// assistants hold the same `assistant_id` under different row ids, so the
/// merge silently removes the local row, and everything that referenced it —
/// the user's own overlays and preferences — is left pointing at an id that no
/// longer exists.
///
/// Deleting those as orphans was the wrong answer to the right observation: a
/// restore MERGES, and a local customisation of an assistant that is still
/// present afterwards should still be attached to it. The references are moved
/// to the surviving row instead, and only what nothing can be attached to is
/// swept.
///
/// Planned BEFORE the merge, because the local ids have to be read while they
/// still exist.
async fn plan_reference_remap(
    live: &mut sqlx::SqliteConnection,
    tables: &[&str],
) -> Result<Vec<(String, String, String)>, SystemError> {
    let mut plan = Vec::new();
    for table in tables {
        if !table_exists(&mut *live, table).await? || !attached_table_exists(&mut *live, table).await? {
            continue;
        }
        let Some(key) = primary_key_column(&mut *live, table).await? else {
            continue;
        };
        for unique in unique_business_keys(&mut *live, table).await? {
            let join = unique
                .iter()
                .map(|column| format!("main_t.\"{column}\" IS backup_t.\"{column}\""))
                .collect::<Vec<_>>()
                .join(" AND ");
            // Local rows the archive is about to replace: same business key,
            // different identity.
            let sql = format!(
                "SELECT main_t.\"{key}\", backup_t.\"{key}\" FROM main.\"{table}\" AS main_t
                 JOIN backup.\"{table}\" AS backup_t ON {join}
                 WHERE main_t.\"{key}\" <> backup_t.\"{key}\""
            );
            let rows = match sqlx::query(&sql).fetch_all(&mut *live).await {
                Ok(rows) => rows,
                // A key this build cannot compare must not fail the restore:
                // the merge still works, the children are just swept as before.
                Err(error) => {
                    tracing::warn!(table, error = %error, "could not plan a reference remap for this table");
                    continue;
                }
            };
            for row in rows {
                let (Ok(old), Ok(new)) = (row.try_get::<String, _>(0), row.try_get::<String, _>(1)) else {
                    continue;
                };
                plan.push(((*table).to_owned(), old, new));
            }
        }
    }
    Ok(plan)
}

/// Applies the remapping planned before the merge.
async fn apply_reference_remap(
    live: &mut sqlx::SqliteConnection,
    plan: &[(String, String, String)],
) -> Result<u64, SystemError> {
    if plan.is_empty() {
        return Ok(0);
    }
    let mut moved = 0u64;
    for (table, old, new) in plan {
        for (child_table, child_column) in referencing_columns(&mut *live, table).await? {
            // OR IGNORE, because the child can carry a unique key of its own
            // that mentions the column being re-pointed: `assistant_overlays`
            // is unique on `(user_id, assistant_definition_id)`, and the
            // archive's own overlay for that assistant is already sitting on
            // the destination. Moving the local row onto it is a genuine
            // duplicate, not a lost customisation — the archive supplies the
            // same pair — so the row is left where it is and the sweep reports
            // it in `orphans_removed` rather than the whole restore dying on a
            // constraint. A plain UPDATE turned that collision into a rolled
            // back restore and a 500, which is the failure this whole change
            // set exists to remove.
            let affected = sqlx::query(&format!(
                "UPDATE OR IGNORE main.\"{child_table}\" SET \"{child_column}\" = ? WHERE \"{child_column}\" = ?"
            ))
            .bind(new)
            .bind(old)
            .execute(&mut *live)
            .await
            .map_err(|error| {
                SystemError::Internal(format!("Could not re-point {child_table}.{child_column}: {error}"))
            })?
            .rows_affected();
            moved += affected;
        }
    }
    Ok(moved)
}

/// The single-column primary key of a table, if it has one.
async fn primary_key_column(conn: &mut sqlx::SqliteConnection, table: &str) -> Result<Option<String>, SystemError> {
    let rows = sqlx::query(&format!("PRAGMA main.table_info(\"{table}\")"))
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not read the schema of {table}: {error}")))?;
    let keys: Vec<String> = rows
        .iter()
        .filter(|row| row.try_get::<i64, _>(5).unwrap_or(0) > 0)
        .filter_map(|row| row.try_get::<String, _>(1).ok())
        .collect();
    Ok(if keys.len() == 1 { keys.into_iter().next() } else { None })
}

/// Unique indexes that are NOT the primary key — the business keys a merge can
/// collide on, and therefore the ones whose collision destroys a local row.
async fn unique_business_keys(conn: &mut sqlx::SqliteConnection, table: &str) -> Result<Vec<Vec<String>>, SystemError> {
    let indexes = sqlx::query(&format!("PRAGMA main.index_list(\"{table}\")"))
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not read the indexes of {table}: {error}")))?;

    let mut keys = Vec::new();
    for index in indexes {
        let name: String = index.try_get(1).unwrap_or_default();
        let unique: i64 = index.try_get(2).unwrap_or(0);
        let origin: String = index.try_get(3).unwrap_or_default();
        // `pk` is the primary key itself: replacing on it is identity, not a
        // business-key collision, and the children already point at the id that
        // survives.
        if unique == 0 || origin == "pk" || name.is_empty() {
            continue;
        }
        let info = sqlx::query(&format!("PRAGMA main.index_info(\"{name}\")"))
            .fetch_all(&mut *conn)
            .await
            .map_err(|error| SystemError::Internal(format!("Could not read index {name}: {error}")))?;
        let columns: Vec<String> = info.iter().filter_map(|row| row.try_get::<String, _>(2).ok()).collect();
        if !columns.is_empty() {
            keys.push(columns);
        }
    }
    Ok(keys)
}

/// Every `(table, column)` in the live catalog whose foreign key points at
/// `parent`.
async fn referencing_columns(
    conn: &mut sqlx::SqliteConnection,
    parent: &str,
) -> Result<Vec<(String, String)>, SystemError> {
    let tables = sqlx::query("SELECT name FROM main.sqlite_master WHERE type = 'table'")
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not list tables: {error}")))?;

    let mut out = Vec::new();
    for row in tables {
        let table: String = row.try_get(0).unwrap_or_default();
        if table.is_empty() || table.starts_with("sqlite_") {
            continue;
        }
        let keys = sqlx::query(&format!("PRAGMA main.foreign_key_list(\"{table}\")"))
            .fetch_all(&mut *conn)
            .await
            .map_err(|error| SystemError::Internal(format!("Could not read the foreign keys of {table}: {error}")))?;
        for key in keys {
            let target: String = key.try_get(2).unwrap_or_default();
            if target != parent {
                continue;
            }
            if let Ok(column) = key.try_get::<String, _>(3) {
                out.push((table.clone(), column));
            }
        }
    }
    Ok(out)
}

/// Deletes rows whose foreign key points at something that is not there.
///
/// The last resort, and deliberately narrow now that `apply_reference_remap`
/// runs first. What reaches here is a row whose parent is not merely at a
/// different id but genuinely absent — restoring conversations WITHOUT app
/// settings is a supported choice, and it leaves
/// `conversation_assistant_snapshots` referring to assistant definitions that
/// were never brought across (measured at 20 such rows on a real catalog).
///
/// There is nothing to attach those to. Leaving them would also leave the
/// catalog contradicting its own declared constraints, so the next thing to
/// enforce them would meet a database that never validated.
///
/// Looped because removing one row can orphan another, and bounded because a
/// schema cycle must not turn that into a spin.
async fn sweep_orphans(
    conn: &mut sqlx::SqliteConnection,
) -> Result<std::collections::BTreeMap<String, u64>, SystemError> {
    const MAX_PASSES: usize = 8;
    let mut removed: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();

    for _ in 0..MAX_PASSES {
        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *conn)
            .await
            .map_err(|error| SystemError::Internal(format!("Could not check the restored catalog: {error}")))?;
        if violations.is_empty() {
            break;
        }

        // Columns are (table, rowid, parent, fkid). `rowid` is NULL for a
        // WITHOUT ROWID table, which cannot be addressed this way — reported
        // rather than silently skipped.
        let mut by_table: std::collections::BTreeMap<String, Vec<i64>> = std::collections::BTreeMap::new();
        for row in &violations {
            let table: String = row.try_get(0).unwrap_or_default();
            match row.try_get::<Option<i64>, _>(1) {
                Ok(Some(rowid)) => by_table.entry(table).or_default().push(rowid),
                _ => tracing::warn!(
                    table,
                    "restored row breaks a foreign key but has no rowid to delete it by"
                ),
            }
        }
        if by_table.is_empty() {
            break;
        }

        for (table, rowids) in by_table {
            let list = rowids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            let affected = sqlx::query(&format!("DELETE FROM main.\"{table}\" WHERE rowid IN ({list})"))
                .execute(&mut *conn)
                .await
                .map_err(|error| {
                    SystemError::Internal(format!("Could not clean up unusable rows in {table}: {error}"))
                })?
                .rows_affected();
            *removed.entry(table).or_default() += affected;
        }
    }

    Ok(removed)
}

/// This install's data secret, from whichever attached schema is asked.
///
/// Missing column, missing table or no row all read as "no secret" rather than
/// an error: a catalog old enough to lack the column is one with nothing
/// encrypted to worry about.
async fn read_data_secret(conn: &mut sqlx::SqliteConnection, schema: &str) -> Result<String, SystemError> {
    let columns = column_names(&mut *conn, schema, "users").await.unwrap_or_default();
    if !columns.iter().any(|column| column == "data_secret") {
        return Ok(String::new());
    }
    let row = sqlx::query(&format!(
        "SELECT data_secret FROM {schema}.users WHERE data_secret IS NOT NULL AND data_secret <> '' LIMIT 1"
    ))
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| SystemError::Internal(format!("Could not read the data secret: {error}")))?;
    Ok(row.and_then(|row| row.try_get::<String, _>(0).ok()).unwrap_or_default())
}

/// Puts the machine's own data secret back after the merge replaced it.
async fn restore_local_data_secret(conn: &mut sqlx::SqliteConnection, secret: &str) -> Result<(), SystemError> {
    sqlx::query("UPDATE main.users SET data_secret = ?")
        .bind(secret)
        .execute(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not keep this install's data secret: {error}")))?;
    Ok(())
}

/// On-disk shape of an encrypted column, since not all of them agree.
///
/// `providers.api_key_encrypted` has been encrypted since the column existed,
/// so it stores the raw `encrypt_string` output with no marker. The MCP
/// columns added later (`mcp_servers.transport_config`,
/// `oauth_tokens.{access_token,refresh_token}`) were plaintext for a while
/// before being covered by `dream_core_db::encrypt_legacy_plaintext`, so they
/// use the versioned envelope (`encrypt_field`/`decrypt_field`,
/// `ENCRYPTED_FIELD_PREFIX`) that tells encrypted apart from not-yet-migrated
/// plaintext. Rekeying has to use the matching pair for each column or it
/// corrupts the value: calling the raw functions on an enveloped value ignores
/// the prefix and AES-decrypts the wrong bytes, and calling the enveloped
/// functions on a raw value silently treats real ciphertext as "legacy
/// plaintext" and passes it through unchanged instead of decrypting it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EncryptedColumnFormat {
    /// `dream_core_common::{encrypt_string, decrypt_string}` — no envelope.
    Raw,
    /// `dream_core_common::{encrypt_field, decrypt_field}` — `encv1:` envelope;
    /// a value with no prefix is legacy plaintext and passes through untouched.
    Field,
}

/// Columns holding a value encrypted with this install's data secret, by table.
///
/// These cannot simply be copied across. The value is sealed with
/// `derive_encryption_key(users.data_secret)`, and the archive's secret is not
/// the target machine's: merging the rows verbatim hands the target ciphertext
/// it has no key for. See [`rekey_restored_secrets`].
const ENCRYPTED_COLUMNS: &[(&str, &str, EncryptedColumnFormat)] = &[
    ("providers", "api_key_encrypted", EncryptedColumnFormat::Raw),
    ("mcp_servers", "transport_config", EncryptedColumnFormat::Field),
    ("oauth_tokens", "access_token", EncryptedColumnFormat::Field),
    ("oauth_tokens", "refresh_token", EncryptedColumnFormat::Field),
];

/// Re-encrypts values that arrived sealed with the archive's data secret.
///
/// The archive carries `users.data_secret`, and the merge used to let it
/// overwrite the local one. That looked harmless and was not: the running
/// process reads the secret ONCE at startup, so after a restore it still held
/// the old key while the catalog held the new one — every restored provider key
/// failed to decrypt until a restart, and every provider the target machine
/// already had became permanently unreadable after it. Measured on two real
/// catalogs: same user id, different `data_secret`.
///
/// So the local secret wins and the archive's is used only to read what the
/// archive brought. A row whose value cannot be decrypted is left as it is
/// rather than replaced with something wrong — an archive from an install whose
/// secret was rotated is the case that produces this, and blanking the key
/// would turn an unusable credential into a lost one.
async fn rekey_restored_secrets(
    live: &mut sqlx::SqliteConnection,
    archive_secret: &str,
    local_secret: &str,
) -> Result<u64, SystemError> {
    if archive_secret == local_secret || archive_secret.is_empty() || local_secret.is_empty() {
        return Ok(0);
    }
    let from = dream_core_app_key(archive_secret);
    let to = dream_core_app_key(local_secret);

    let mut rekeyed = 0u64;
    for (table, column, format) in ENCRYPTED_COLUMNS {
        if !table_exists(&mut *live, table).await? {
            continue;
        }
        let rows = sqlx::query(&format!(
            "SELECT rowid, \"{column}\" FROM main.\"{table}\" WHERE \"{column}\" IS NOT NULL AND \"{column}\" <> ''"
        ))
        .fetch_all(&mut *live)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not read {table} to re-key it: {error}")))?;

        for row in rows {
            let rowid: i64 = row.try_get(0).unwrap_or_default();
            let sealed: String = row.try_get(1).unwrap_or_default();
            // Already readable with the local key: a row the target machine
            // owned before this restore, OR (Field format only) a value that
            // was never encrypted in the first place — decrypt_field passes
            // unprefixed legacy plaintext through as "success" by design, and
            // there is nothing to re-key for either case. Leave it alone.
            if decrypt_by_format(*format, &sealed, &to).is_ok() {
                continue;
            }
            let Ok(plaintext) = decrypt_by_format(*format, &sealed, &from) else {
                tracing::warn!(
                    table,
                    rowid,
                    "restored credential could not be decrypted; left untouched"
                );
                continue;
            };
            let resealed = encrypt_by_format(*format, &plaintext, &to)
                .map_err(|error| SystemError::Internal(format!("Could not re-key {table}: {error}")))?;
            sqlx::query(&format!("UPDATE main.\"{table}\" SET \"{column}\" = ? WHERE rowid = ?"))
                .bind(resealed)
                .bind(rowid)
                .execute(&mut *live)
                .await
                .map_err(|error| SystemError::Internal(format!("Could not re-key {table}: {error}")))?;
            rekeyed += 1;
        }
    }
    Ok(rekeyed)
}

/// Decrypt one column value with the pair matching its on-disk format. See
/// [`EncryptedColumnFormat`].
fn decrypt_by_format(
    format: EncryptedColumnFormat,
    sealed: &str,
    key: &[u8],
) -> Result<String, dream_core_common::CryptoError> {
    match format {
        EncryptedColumnFormat::Raw => dream_core_common::decrypt_string(sealed, key),
        EncryptedColumnFormat::Field => dream_core_common::decrypt_field(sealed, key),
    }
}

/// Encrypt one column value with the pair matching its on-disk format,
/// preserving the envelope for `Field`-format columns. See
/// [`EncryptedColumnFormat`].
fn encrypt_by_format(
    format: EncryptedColumnFormat,
    plaintext: &str,
    key: &[u8],
) -> Result<String, dream_core_common::CryptoError> {
    match format {
        EncryptedColumnFormat::Raw => dream_core_common::encrypt_string(plaintext, key),
        EncryptedColumnFormat::Field => dream_core_common::encrypt_field(plaintext, key),
    }
}

/// The same derivation `dream-core-app` uses, duplicated rather than imported:
/// that crate sits above this one in the layering, so depending on it would
/// invert the dependency direction. The domain prefix is a frozen legacy value
/// — changing one byte makes every stored credential undecryptable.
fn dream_core_app_key(data_secret: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"aionui-encryption-key:");
    hasher.update(data_secret.as_bytes());
    hasher.finalize().into()
}

async fn merge_tables(
    live: &mut sqlx::SqliteConnection,
    scope: BackupScope,
) -> Result<std::collections::BTreeMap<String, u64>, SystemError> {
    let mut merged = std::collections::BTreeMap::new();
    for table in scope.tables() {
        if !table_exists(&mut *live, table).await? || !attached_table_exists(&mut *live, table).await? {
            continue;
        }

        // Only columns both schemas have. An archive from an older build is
        // missing columns this one added, and vice versa; selecting `*` would
        // fail on the first such difference.
        let live_columns = column_names(&mut *live, "main", table).await?;
        let staged_columns = column_names(&mut *live, "backup", table).await?;
        let shared: Vec<String> = live_columns
            .iter()
            .filter(|column| staged_columns.contains(column))
            .cloned()
            .collect();
        if shared.is_empty() {
            continue;
        }

        // A surrogate INTEGER key means row identity is assigned by the
        // database, not carried by the data: reusing the archive's value would
        // overwrite an unrelated local row that happens to hold that number.
        // Drop the column, let SQLite assign, and let the table's own unique
        // index collapse duplicates.
        let surrogate = surrogate_key_column(&mut *live, table).await?;
        let insertable: Vec<String> = shared
            .iter()
            .filter(|column| surrogate.as_deref() != Some(column.as_str()))
            .cloned()
            .collect();
        if insertable.is_empty() {
            continue;
        }

        let columns = insertable
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(", ");
        // `OR REPLACE` for business-keyed tables: re-running a restore should
        // converge, not fail on a duplicate. `OR IGNORE` where the key was
        // dropped, so an already-present row is kept as it is.
        let conflict = if surrogate.is_some() { "OR IGNORE" } else { "OR REPLACE" };
        let affected = sqlx::query(&format!(
            "INSERT {conflict} INTO main.\"{table}\" ({columns}) SELECT {columns} FROM backup.\"{table}\""
        ))
        .execute(&mut *live)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not restore {table}: {error}")))?
        .rows_affected();
        if affected > 0 {
            merged.insert(table.to_owned(), affected);
        }
    }
    Ok(merged)
}

/// The column that is an auto-assigned INTEGER primary key, if any.
///
/// Takes the connection rather than the pool for the same reason the merge
/// does: it runs between an ATTACH and its DETACH, and those live on one
/// connection.
async fn surrogate_key_column(conn: &mut sqlx::SqliteConnection, table: &str) -> Result<Option<String>, SystemError> {
    let rows = sqlx::query(&format!("PRAGMA main.table_info(\"{table}\")"))
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not read the schema of {table}: {error}")))?;
    let mut keys: Vec<(String, String)> = Vec::new();
    for row in &rows {
        let pk: i64 = row.try_get("pk").unwrap_or(0);
        if pk > 0 {
            let name: String = row.try_get("name").unwrap_or_default();
            let kind: String = row.try_get("type").unwrap_or_default();
            keys.push((name, kind));
        }
    }
    // Only a single-column INTEGER key is a rowid alias. A composite key, or a
    // TEXT key, carries identity from the data and must be preserved.
    match keys.as_slice() {
        [(name, kind)] if kind.eq_ignore_ascii_case("INTEGER") => Ok(Some(name.clone())),
        _ => Ok(None),
    }
}

async fn column_names(
    conn: &mut sqlx::SqliteConnection,
    schema: &str,
    table: &str,
) -> Result<Vec<String>, SystemError> {
    let rows = sqlx::query(&format!("PRAGMA {schema}.table_info(\"{table}\")"))
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not read the schema of {table}: {error}")))?;
    Ok(rows
        .iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect())
}

async fn table_exists(conn: &mut sqlx::SqliteConnection, table: &str) -> Result<bool, SystemError> {
    let found: Option<String> = sqlx::query_scalar("SELECT name FROM main.sqlite_master WHERE type='table' AND name=?")
        .bind(table)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not inspect the catalog: {error}")))?;
    Ok(found.is_some())
}

async fn attached_table_exists(conn: &mut sqlx::SqliteConnection, table: &str) -> Result<bool, SystemError> {
    let found: Option<String> =
        sqlx::query_scalar("SELECT name FROM backup.sqlite_master WHERE type='table' AND name=?")
            .bind(table)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| SystemError::Internal(format!("Could not inspect the backup catalog: {error}")))?;
    Ok(found.is_some())
}

/// Copy the archive's files into the data directory, limited to the subtrees
/// `scope` covers.
///
/// A path in the archive that no selected category claims is skipped rather
/// than written: the archive is untrusted input, and "restore only my skills"
/// must not drop files anywhere else.
fn copy_tree(from: &Path, to: &Path, scope: BackupScope) -> Result<u64, SystemError> {
    let allowed = scope.dirs();
    let allowed_files = scope.files();
    let mut copied = 0;
    for entry in walkdir::WalkDir::new(from).follow_links(false) {
        let entry = entry.map_err(|error| SystemError::Internal(format!("Could not read the backup: {error}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(from)
            .map_err(|_| SystemError::Internal("Restore walked outside the staging directory.".to_owned()))?;
        let first = relative
            .components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let is_allowed_dir = allowed.iter().any(|dir| *dir == first);
        let is_allowed_file = relative.components().count() == 1 && allowed_files.iter().any(|file| *file == first);
        if !is_allowed_dir && !is_allowed_file {
            continue;
        }

        let target = to.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| SystemError::Internal(format!("Could not restore files: {error}")))?;
        }
        std::fs::copy(entry.path(), &target)
            .map_err(|error| SystemError::Internal(format!("Could not restore files: {error}")))?;
        copied += 1;
    }
    Ok(copied)
}

async fn open_sqlite(path: &Path) -> Result<SqlitePool, SystemError> {
    let url = format!("sqlite://{}", path.to_string_lossy().replace('\\', "/"));
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not open the backup catalog: {error}")))
}

fn intersect(archive: BackupScope, requested: BackupScope) -> BackupScope {
    BackupScope {
        conversations: archive.conversations && requested.conversations,
        attachments: archive.attachments && requested.attachments,
        providers: archive.providers && requested.providers,
        skills: archive.skills && requested.skills,
        app_settings: archive.app_settings && requested.app_settings,
    }
}

/// Writes the archive, sealing every payload entry when a key is given.
///
/// `manifest.json` is never sealed — see `backup_crypto`. Payloads are sealed
/// one file at a time rather than the archive as a whole, which keeps peak
/// memory at the size of the largest single file: a real export was 13,304
/// files and 228 MB, and holding that at once to encrypt it would be a very
/// expensive way to save a few lines.
fn write_archive(
    destination: &Path,
    manifest: &BackupManifest,
    entries: &[(String, PathBuf)],
    key: Option<&ArchiveKey>,
) -> Result<(), SystemError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| SystemError::Internal(format!("Could not create the backup folder: {error}")))?;
    }
    let file = std::fs::File::create(destination)
        .map_err(|error| SystemError::Internal(format!("Could not create the backup file: {error}")))?;
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let manifest_json = serde_json::to_vec_pretty(manifest)
        .map_err(|error| SystemError::Internal(format!("Could not write the backup manifest: {error}")))?;
    writer
        .start_file(MANIFEST_NAME, options)
        .map_err(|error| SystemError::Internal(format!("Could not write the backup manifest: {error}")))?;
    std::io::Write::write_all(&mut writer, &manifest_json)
        .map_err(|error| SystemError::Internal(format!("Could not write the backup manifest: {error}")))?;

    for (name, path) in entries {
        writer
            .start_file(name.as_str(), options)
            .map_err(|error| SystemError::Internal(format!("Could not add {name} to the backup: {error}")))?;
        match key {
            Some(key) => {
                let plaintext = std::fs::read(path).map_err(|error| {
                    SystemError::Internal(format!("Could not read {} for backup: {error}", path.display()))
                })?;
                let sealed = key.seal(&plaintext)?;
                std::io::Write::write_all(&mut writer, &sealed)
                    .map_err(|error| SystemError::Internal(format!("Could not add {name} to the backup: {error}")))?;
            }
            None => {
                let mut source = std::fs::File::open(path).map_err(|error| {
                    SystemError::Internal(format!("Could not read {} for backup: {error}", path.display()))
                })?;
                std::io::copy(&mut source, &mut writer)
                    .map_err(|error| SystemError::Internal(format!("Could not add {name} to the backup: {error}")))?;
            }
        }
    }

    writer
        .finish()
        .map_err(|error| SystemError::Internal(format!("Could not finish the backup file: {error}")))?;
    Ok(())
}

fn read_manifest(archive: &Path) -> Result<BackupManifest, SystemError> {
    let file = std::fs::File::open(archive)
        .map_err(|error| SystemError::BadRequest(format!("Could not open the backup file: {error}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| SystemError::BadRequest(format!("This file is not a readable backup: {error}")))?;
    let entry = zip
        .by_name(MANIFEST_NAME)
        .map_err(|_| SystemError::BadRequest("This file is not a One Work backup.".to_owned()))?;
    serde_json::from_reader(entry)
        .map_err(|error| SystemError::BadRequest(format!("This backup's manifest is unreadable: {error}")))
}

fn unpack_archive(archive: &Path, destination: &Path, key: Option<&ArchiveKey>) -> Result<(), SystemError> {
    let file = std::fs::File::open(archive)
        .map_err(|error| SystemError::BadRequest(format!("Could not open the backup file: {error}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| SystemError::BadRequest(format!("This file is not a readable backup: {error}")))?;
    std::fs::create_dir_all(destination)
        .map_err(|error| SystemError::Internal(format!("Could not create the staging directory: {error}")))?;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| SystemError::BadRequest(format!("This backup could not be read: {error}")))?;
        if entry.is_dir() {
            continue;
        }
        // `enclosed_name` rejects absolute paths and `..` traversal. Without
        // it, a crafted archive could write anywhere the process can.
        let Some(relative) = entry.enclosed_name() else {
            return Err(SystemError::BadRequest(
                "This backup contains an unsafe file path and was not restored.".to_owned(),
            ));
        };
        let target = destination.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| SystemError::Internal(format!("Could not unpack the backup: {error}")))?;
        }
        // The manifest is the one entry that was never sealed.
        let sealed_entry = key.is_some() && relative != Path::new(MANIFEST_NAME);
        let mut out = std::fs::File::create(&target)
            .map_err(|error| SystemError::Internal(format!("Could not unpack the backup: {error}")))?;
        if sealed_entry {
            let mut sealed = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut sealed)
                .map_err(|error| SystemError::Internal(format!("Could not unpack the backup: {error}")))?;
            let plaintext = key.expect("checked above").open(&sealed)?;
            std::io::Write::write_all(&mut out, &plaintext)
                .map_err(|error| SystemError::Internal(format!("Could not unpack the backup: {error}")))?;
        } else {
            std::io::copy(&mut entry, &mut out)
                .map_err(|error| SystemError::Internal(format!("Could not unpack the backup: {error}")))?;
        }
    }
    Ok(())
}

/// Zip entry names use `/` on every platform.
fn to_archive_path(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Whether `candidate` is a strictly newer semantic version than `current`.
///
/// An unparseable version on either side returns false: refusing a restore on
/// the strength of a version string nobody can read would block a user from
/// their own data over a formatting detail.
fn is_newer_version(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => false,
    }
}

#[cfg(test)]
#[path = "backup_test.rs"]
mod backup_test;
