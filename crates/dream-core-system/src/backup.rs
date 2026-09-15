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

use crate::error::SystemError;

/// Archive format version. A restore refuses anything it does not recognize
/// rather than half-applying it.
pub const BACKUP_FORMAT_VERSION: u32 = 2;

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
}

/// Outcome of a restore, per category.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    /// Rows merged into the live catalog, by table.
    pub rows_by_table: std::collections::BTreeMap<String, u64>,
    /// Files copied out of the archive.
    pub files_restored: u64,
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
    pub async fn export(
        &self,
        pool: &DbPool,
        destination: &Path,
        scope: BackupScope,
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
        let manifest = BackupManifest {
            format_version: BACKUP_FORMAT_VERSION,
            exported_at: now_ms(),
            app_version: self.app_version.clone(),
            scope,
            total_bytes,
            contains_credentials: scope.providers,
        };

        // Compressing runs on the blocking pool, not an async worker.
        //
        // This is not a formality: a real export was 13,304 files / 228 MB and
        // took about half an hour, all of it inside one synchronous call. Left
        // on an async worker that is one of tokio's few threads pinned for the
        // whole run, and every other request contends for what is left.
        let manifest_for_write = manifest.clone();
        let destination = destination.to_path_buf();
        tokio::task::spawn_blocking(move || write_archive(&destination, &manifest_for_write, &entries))
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
    pub async fn restore(
        &self,
        pool: &DbPool,
        archive: &Path,
        requested: BackupScope,
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

        let staging = tempfile::tempdir()
            .map_err(|error| SystemError::Internal(format!("Could not create a staging directory: {error}")))?;
        unpack_archive(archive, staging.path())?;

        let mut outcome = RestoreOutcome::default();
        let staged_catalog = staging.path().join(ARCHIVE_DB_NAME);
        if staged_catalog.is_file() {
            outcome.rows_by_table = merge_catalog(live, &staged_catalog, effective).await?;
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
async fn merge_catalog(
    live: &SqlitePool,
    staged: &Path,
    scope: BackupScope,
) -> Result<std::collections::BTreeMap<String, u64>, SystemError> {
    let mut conn = live
        .acquire()
        .await
        .map_err(|error| SystemError::Internal(format!("Could not open the catalog for restore: {error}")))?;

    let staged_path = staged.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("ATTACH DATABASE '{staged_path}' AS backup"))
        .execute(&mut *conn)
        .await
        .map_err(|error| SystemError::Internal(format!("Could not open the backup catalog: {error}")))?;

    let result = merge_tables(&mut conn, scope).await;

    // Detach even when the merge failed: the connection goes back to the pool
    // either way, and an attached database left on it would make the next
    // restore's ATTACH fail with "database backup is already in use".
    let _ = sqlx::query("DETACH DATABASE backup").execute(&mut *conn).await;
    result
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

fn write_archive(
    destination: &Path,
    manifest: &BackupManifest,
    entries: &[(String, PathBuf)],
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
        let mut source = std::fs::File::open(path)
            .map_err(|error| SystemError::Internal(format!("Could not read {} for backup: {error}", path.display())))?;
        writer
            .start_file(name.as_str(), options)
            .map_err(|error| SystemError::Internal(format!("Could not add {name} to the backup: {error}")))?;
        std::io::copy(&mut source, &mut writer)
            .map_err(|error| SystemError::Internal(format!("Could not add {name} to the backup: {error}")))?;
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

fn unpack_archive(archive: &Path, destination: &Path) -> Result<(), SystemError> {
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
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| SystemError::Internal(format!("Could not unpack the backup: {error}")))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|error| SystemError::Internal(format!("Could not unpack the backup: {error}")))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|error| SystemError::Internal(format!("Could not unpack the backup: {error}")))?;
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
