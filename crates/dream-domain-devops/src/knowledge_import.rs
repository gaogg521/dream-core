//! Import a folder of a registered file storage into a company knowledge space.
//!
//! The storage itself belongs to the platform plane (`dream-domain-platform`),
//! which this crate must not depend on, so the two meet at [`ExternalObjectSource`]:
//! the app layer implements it over the platform's object-storage service and
//! hands it to [`DevopsService`]. Without one (the personal edition) every
//! operation here refuses with a plain message and nothing else changes.
//!
//! A run is a mirror in one direction only — storage to knowledge. It never
//! writes to the storage, and it never deletes a document: a file that is gone
//! from the storage is flagged `missing` and left searchable, because a
//! mislaid folder or a brief outage must not erase company knowledge.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use dream_core_api_types::knowledge_import::{
    KnowledgeImportIssue, KnowledgeImportReport, KnowledgeImportSource, SaveKnowledgeImportSource,
};
use dream_core_common::now_ms;
use dream_core_db::db_params;

use crate::error::DevopsError;
use crate::service::{DevopsService, new_id};

/// How deep and how wide one run walks. A bound is what turns "someone pointed
/// this at the root of the NAS" from an outage into a report that says so.
const MAX_DEPTH: usize = 8;
const MAX_OBJECTS: usize = 5000;
/// One file, as read into memory. Larger files are skipped, not truncated.
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Extracted text beyond this is refused: it would cost an embedding call per
/// few hundred characters and is almost never meant to be searched whole.
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
/// Matches the `source_key` column on MySQL (see migration 023).
const MAX_KEY_CHARS: usize = 512;
/// Issues kept in a report; the totals beside them stay exact.
const ISSUE_CAP: usize = 200;
/// A run that has not finished by now is presumed dead (process killed).
const STALE_RUN_MS: i64 = 2 * 60 * 60 * 1000;
const SCHEDULER_TICK: std::time::Duration = std::time::Duration::from_secs(60);

const TEXT_EXTENSIONS: [&str; 6] = ["md", "markdown", "txt", "csv", "html", "htm"];
const OFFICE_EXTENSIONS: [&str; 3] = ["docx", "pptx", "xlsx"];

/// What the server can read the text of today.
pub fn supported_extensions() -> Vec<&'static str> {
    TEXT_EXTENSIONS
        .iter()
        .chain(OFFICE_EXTENSIONS.iter())
        .copied()
        .collect()
}

/// One object or folder, as the import sees it.
#[derive(Debug, Clone)]
pub struct ExternalEntry {
    /// Full key, used to read the object and as its identity.
    pub key: String,
    /// The path to list next (folders) or display (files), relative to the
    /// storage's configured prefix.
    pub rel: String,
    pub is_prefix: bool,
    pub size_bytes: Option<i64>,
    pub last_modified: Option<i64>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ExternalListing {
    pub entries: Vec<ExternalEntry>,
    pub next_token: Option<String>,
}

/// Read access to a registered file storage, on behalf of an administrator.
#[async_trait]
pub trait ExternalObjectSource: Send + Sync {
    /// Does the actor's company have this storage registered?
    async fn storage_exists(&self, actor: &str, storage_id: &str) -> Result<bool, String>;
    /// One level under `sub_prefix` (relative to the storage's own prefix).
    async fn list(
        &self,
        actor: &str,
        storage_id: &str,
        sub_prefix: &str,
        token: Option<&str>,
    ) -> Result<ExternalListing, String>;
    /// One object, whole, refusing anything above `max_bytes`.
    async fn read(&self, actor: &str, storage_id: &str, key: &str, max_bytes: u64) -> Result<Vec<u8>, String>;
}

#[derive(Debug, sqlx::FromRow)]
struct SourceRow {
    id: String,
    name: String,
    storage_config_id: String,
    prefix: String,
    library_id: String,
    scope: String,
    team_id: Option<String>,
    visibility: String,
    extensions: String,
    schedule: String,
    enabled: i64,
    created_by: String,
    created_at: i64,
    last_run_at: Option<i64>,
    last_report: Option<String>,
    running_since: Option<i64>,
}

const SOURCE_COLUMNS: &str = "id, name, storage_config_id, prefix, library_id, scope, team_id, visibility, \
     extensions, schedule, enabled, created_by, created_at, last_run_at, last_report, running_since";

fn extension_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn file_name(rel: &str) -> &str {
    rel.trim_end_matches('/').rsplit('/').next().unwrap_or(rel)
}

/// Reject `..` and `.` segments, then give the prefix its one shape: no leading
/// slash, a single trailing slash when non-empty.
fn normalize_sub_prefix(raw: &str) -> Result<String, DevopsError> {
    if raw.split('/').any(|segment| segment == ".." || segment == ".") {
        return Err(DevopsError::BadRequest(
            "prefix may not contain \"..\" or \".\" path segments".into(),
        ));
    }
    let trimmed = raw.trim().trim_start_matches('/').trim_end_matches('/');
    Ok(if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    })
}

/// One document a source already brought: id, key, signature, source state, indexing status.
type ExistingRow = (String, String, Option<String>, Option<String>, String);
/// The same without its key (the key is the map's): id, signature, source state, indexing status.
type ExistingDoc = (String, Option<String>, Option<String>, String);

/// How a file is judged unchanged: the storage's own marker plus size and
/// modified time, so a storage with no ETag (WebDAV) still notices a rewrite.
fn signature(entry: &ExternalEntry) -> String {
    format!(
        "{}|{}|{}",
        entry.etag.as_deref().unwrap_or(""),
        entry.size_bytes.unwrap_or(-1),
        entry.last_modified.unwrap_or(-1)
    )
}

/// Plain text out of an HTML page, for search only (the original is kept).
fn html_to_text(html: &str) -> String {
    static BLOCKS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?is)<(script|style)\b.*?</(script|style)>").unwrap());
    static TAGS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)<[^>]*>").unwrap());
    let without_blocks = BLOCKS.replace_all(html, " ");
    let text = TAGS
        .replace_all(&without_blocks, " ")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_text(extension: &str, bytes: &[u8]) -> Result<String, DevopsError> {
    let text = if OFFICE_EXTENSIONS.contains(&extension) {
        crate::office_text::extract(bytes, extension)?
    } else if matches!(extension, "html" | "htm") {
        html_to_text(&String::from_utf8_lossy(bytes))
    } else {
        String::from_utf8_lossy(bytes).trim_start_matches('\u{feff}').to_owned()
    };
    if text.len() > MAX_TEXT_BYTES {
        return Err(DevopsError::BadRequest(
            "extracted text is larger than 8 MiB; split the file".into(),
        ));
    }
    Ok(text)
}

/// A bounded list that still counts what it had to drop.
struct Issues {
    items: Vec<KnowledgeImportIssue>,
    total: u32,
}

impl Issues {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            total: 0,
        }
    }
    fn push(&mut self, key: &str, reason: impl Into<String>) {
        self.total += 1;
        if self.items.len() < ISSUE_CAP {
            self.items.push(KnowledgeImportIssue {
                key: key.to_owned(),
                reason: reason.into(),
            });
        }
    }
}

impl DevopsService {
    /// Wire the file-storage reader. Called once by the app router under the
    /// `enterprise` feature; `&self` so it can run after the service is shared.
    pub fn set_external_object_source(&self, source: Arc<dyn ExternalObjectSource>) {
        if self.external_source.set(source).is_err() {
            tracing::warn!("external object source already wired; ignoring the second attempt");
        }
    }

    /// Where retained originals live, for imports that run without a request.
    pub fn set_rag_source_root(&self, root: std::path::PathBuf) {
        let _ = self.rag_source_root.set(root);
    }

    fn external(&self) -> Result<&Arc<dyn ExternalObjectSource>, DevopsError> {
        self.external_source
            .get()
            .ok_or_else(|| DevopsError::BadRequest("file storage is not available on this deployment".into()))
    }

    async fn require_registry_admin(&self, actor: &str) -> Result<(), DevopsError> {
        if self.is_registry_admin(actor).await? {
            Ok(())
        } else {
            Err(DevopsError::Forbidden(
                "only administrators can manage knowledge import sources".into(),
            ))
        }
    }

    async fn source_row(&self, id: &str) -> Result<SourceRow, DevopsError> {
        self.db
            .fetch_optional_as::<SourceRow>(
                &format!("SELECT {SOURCE_COLUMNS} FROM one_rag_import_sources WHERE id = ?"),
                &db_params![id],
            )
            .await?
            .ok_or_else(|| DevopsError::NotFound(format!("import source {id}")))
    }

    async fn source_dto(&self, row: SourceRow) -> Result<KnowledgeImportSource, DevopsError> {
        let document_count: i64 = self
            .db
            .fetch_one_scalar(
                "SELECT COUNT(*) FROM one_rag_documents WHERE source_id = ?",
                &db_params![&row.id],
            )
            .await?;
        let running = row
            .running_since
            .is_some_and(|since| now_ms() as i64 - since < STALE_RUN_MS);
        Ok(KnowledgeImportSource {
            id: row.id,
            name: row.name,
            storage_config_id: row.storage_config_id,
            prefix: row.prefix,
            library_id: row.library_id,
            scope: row.scope,
            team_id: row.team_id,
            visibility: row.visibility,
            extensions: serde_json::from_str(&row.extensions).unwrap_or_default(),
            schedule: row.schedule,
            enabled: row.enabled != 0,
            created_by: row.created_by,
            created_at: row.created_at,
            last_run_at: row.last_run_at,
            last_report: row.last_report.and_then(|json| serde_json::from_str(&json).ok()),
            running,
            document_count,
        })
    }

    pub async fn list_import_sources(&self, actor: &str) -> Result<Vec<KnowledgeImportSource>, DevopsError> {
        self.require_registry_admin(actor).await?;
        let rows: Vec<SourceRow> = self
            .db
            .fetch_all_as::<SourceRow>(
                &format!("SELECT {SOURCE_COLUMNS} FROM one_rag_import_sources ORDER BY created_at DESC"),
                &[],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(self.source_dto(row).await?);
        }
        Ok(out)
    }

    pub async fn save_import_source(
        &self,
        actor: &str,
        id: Option<&str>,
        input: SaveKnowledgeImportSource,
    ) -> Result<KnowledgeImportSource, DevopsError> {
        self.require_registry_admin(actor).await?;
        let external = self.external()?;

        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 255 {
            return Err(DevopsError::BadRequest("name must be 1 to 255 characters".into()));
        }
        let storage_id = input.storage_config_id.trim();
        if storage_id.is_empty()
            || !external
                .storage_exists(actor, storage_id)
                .await
                .map_err(DevopsError::BadRequest)?
        {
            return Err(DevopsError::BadRequest("file storage not found".into()));
        }
        let prefix = normalize_sub_prefix(&input.prefix)?;
        if prefix.chars().count() > MAX_KEY_CHARS {
            return Err(DevopsError::BadRequest("prefix is too long".into()));
        }

        let library_id = input.library_id.trim();
        let scope = input.scope.as_deref().unwrap_or("org");
        let visibility = input.visibility.as_deref().unwrap_or("all");
        // Same rules a hand-registered company document is held to.
        let team_id = self
            .validate_resource_scope(actor, scope, input.team_id.as_deref(), visibility)
            .await?
            .map(str::to_owned);
        self.assert_rag_library_writable(actor, library_id).await?;
        self.assert_rag_library_target(actor, library_id, scope).await?;

        let supported = supported_extensions();
        let mut extensions: Vec<String> = input
            .extensions
            .iter()
            .map(|ext| ext.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect();
        if let Some(bad) = extensions.iter().find(|ext| !supported.contains(&ext.as_str())) {
            return Err(DevopsError::BadRequest(format!(
                "file type .{bad} cannot be imported; supported: {}",
                supported.join(", ")
            )));
        }
        extensions.sort();
        extensions.dedup();
        if extensions.is_empty() {
            extensions = supported.iter().map(|ext| (*ext).to_owned()).collect();
        }
        let schedule = input.schedule.as_deref().unwrap_or("manual");
        if !matches!(schedule, "manual" | "hourly" | "daily") {
            return Err(DevopsError::BadRequest(
                "schedule must be manual, hourly or daily".into(),
            ));
        }
        let enabled = i64::from(input.enabled.unwrap_or(true));
        let extensions_json = serde_json::to_string(&extensions).map_err(|e| DevopsError::Internal(e.to_string()))?;
        let now = now_ms() as i64;

        let id = match id {
            Some(existing) => {
                self.source_row(existing).await?;
                self.db
                    .execute(
                        "UPDATE one_rag_import_sources SET name = ?, storage_config_id = ?, prefix = ?, library_id = ?, \
                         scope = ?, team_id = ?, visibility = ?, extensions = ?, schedule = ?, enabled = ?, \
                         updated_at = ? WHERE id = ?",
                        &db_params![
                            name,
                            storage_id,
                            &prefix,
                            library_id,
                            scope,
                            team_id.as_deref(),
                            visibility,
                            &extensions_json,
                            schedule,
                            enabled,
                            now,
                            existing
                        ],
                    )
                    .await?;
                existing.to_owned()
            }
            None => {
                let new = new_id("oimp");
                self.db
                    .execute(
                        "INSERT INTO one_rag_import_sources (id, name, storage_config_id, prefix, library_id, scope, \
                         team_id, visibility, extensions, schedule, enabled, created_by, created_at, updated_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        &db_params![
                            &new,
                            name,
                            storage_id,
                            &prefix,
                            library_id,
                            scope,
                            team_id.as_deref(),
                            visibility,
                            &extensions_json,
                            schedule,
                            enabled,
                            actor,
                            now,
                            now
                        ],
                    )
                    .await?;
                new
            }
        };
        let row = self.source_row(&id).await?;
        self.source_dto(row).await
    }

    /// Remove the source. Its documents stay as ordinary company documents —
    /// deleting a configuration must not delete knowledge.
    pub async fn delete_import_source(&self, actor: &str, id: &str) -> Result<(), DevopsError> {
        self.require_registry_admin(actor).await?;
        self.source_row(id).await?;
        let mut tx = self.db.begin().await?;
        tx.execute(
            "UPDATE one_rag_documents SET source_id = NULL, source_key = NULL, source_signature = NULL, \
             source_state = NULL WHERE source_id = ?",
            &db_params![id],
        )
        .await?;
        tx.execute("DELETE FROM one_rag_import_sources WHERE id = ?", &db_params![id])
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Start a run in the background. Returns once it has been claimed.
    pub async fn start_import_run(self: &Arc<Self>, actor: &str, id: &str) -> Result<(), DevopsError> {
        self.require_registry_admin(actor).await?;
        self.external()?;
        self.launch_import_run(id).await
    }

    async fn launch_import_run(self: &Arc<Self>, id: &str) -> Result<(), DevopsError> {
        let row = self.source_row(id).await?;
        let now = now_ms() as i64;
        // Atomic across the app and admin processes that share this database.
        let claimed = self
            .db
            .execute(
                "UPDATE one_rag_import_sources SET running_since = ? \
                 WHERE id = ? AND (running_since IS NULL OR running_since < ?)",
                &db_params![now, id, now - STALE_RUN_MS],
            )
            .await?;
        if claimed == 0 {
            return Err(DevopsError::BadRequest("this import source is already running".into()));
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let report = service.run_import(&row).await;
            let json = serde_json::to_string(&report).unwrap_or_default();
            if let Err(error) = service
                .db
                .execute(
                    "UPDATE one_rag_import_sources SET running_since = NULL, last_run_at = ?, last_report = ? \
                     WHERE id = ?",
                    &db_params![report.finished_at, &json, &row.id],
                )
                .await
            {
                tracing::warn!(%error, source_id = %row.id, "could not record the import report");
            }
            tracing::info!(
                source_id = %row.id,
                status = %report.status,
                added = report.added,
                updated = report.updated,
                unchanged = report.unchanged,
                missing = report.missing,
                skipped = report.skipped_total,
                failed = report.failed_total,
                "knowledge import finished"
            );
        });
        Ok(())
    }

    /// Start every scheduled source that is due. One tick; called by the loop.
    pub async fn run_due_imports(self: &Arc<Self>) {
        if self.external_source.get().is_none() {
            return;
        }
        let rows: Vec<(String, String, Option<i64>)> = match self
            .db
            .fetch_all_as::<(String, String, Option<i64>)>(
                "SELECT id, schedule, last_run_at FROM one_rag_import_sources \
                 WHERE enabled = 1 AND schedule IN ('hourly', 'daily')",
                &[],
            )
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, "could not read scheduled knowledge imports");
                return;
            }
        };
        let now = now_ms() as i64;
        for (id, schedule, last_run_at) in rows {
            let every = if schedule == "hourly" { 3_600_000 } else { 86_400_000 };
            if last_run_at.is_none_or(|last| now - last >= every) {
                // Already running (another process) is not an error here.
                let _ = self.launch_import_run(&id).await;
            }
        }
    }

    /// Check for due imports every minute for the life of the process. A run
    /// cut short by exit is marked stale after `STALE_RUN_MS` and picked up
    /// again, so no shutdown handshake is needed.
    pub fn spawn_import_scheduler(self: &Arc<Self>) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SCHEDULER_TICK).await;
                service.run_due_imports().await;
            }
        });
    }

    async fn run_import(self: &Arc<Self>, source: &SourceRow) -> KnowledgeImportReport {
        let mut report = KnowledgeImportReport {
            started_at: now_ms() as i64,
            ..Default::default()
        };
        let outcome = self.run_import_inner(source, &mut report).await;
        match outcome {
            Ok(()) => {
                if report.status.is_empty() {
                    report.status = if report.failed_total > 0 { "partial" } else { "ok" }.into();
                }
            }
            Err(message) => {
                report.status = "error".into();
                report.message = message;
            }
        }
        report.finished_at = now_ms() as i64;
        report
    }

    async fn run_import_inner(
        self: &Arc<Self>,
        source: &SourceRow,
        report: &mut KnowledgeImportReport,
    ) -> Result<(), String> {
        let external = self.external().map_err(|e| e.to_string())?.clone();
        let root = self
            .rag_source_root
            .get()
            .cloned()
            .ok_or_else(|| "original file storage is not configured".to_owned())?;

        // The owner must still be allowed to manage company knowledge: a source
        // outlives the administrator who made it, and must not outlive their role.
        if !self
            .is_registry_admin(&source.created_by)
            .await
            .map_err(|e| e.to_string())?
        {
            report.status = "paused".into();
            report.message = "the administrator who created this source can no longer manage knowledge".into();
            return Ok(());
        }
        let actor = source.created_by.as_str();
        let wanted: HashSet<String> = serde_json::from_str::<Vec<String>>(&source.extensions)
            .unwrap_or_default()
            .into_iter()
            .collect();

        // --- walk the folder -------------------------------------------------
        let mut files: Vec<ExternalEntry> = Vec::new();
        let mut skipped = Issues::new();
        let mut failed = Issues::new();
        let mut complete = true;
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((source.prefix.clone(), 0));
        let mut first_listing = true;
        'walk: while let Some((prefix, depth)) = queue.pop_front() {
            let mut token: Option<String> = None;
            loop {
                let listing = match external
                    .list(actor, &source.storage_config_id, &prefix, token.as_deref())
                    .await
                {
                    Ok(listing) => listing,
                    Err(message) if first_listing => return Err(format!("could not list the storage: {message}")),
                    Err(message) => {
                        failed.push(&prefix, format!("could not list this folder: {message}"));
                        complete = false;
                        continue 'walk;
                    }
                };
                first_listing = false;
                for entry in listing.entries {
                    if entry.is_prefix {
                        if depth + 1 > MAX_DEPTH {
                            complete = false;
                        } else {
                            queue.push_back((entry.rel.clone(), depth + 1));
                        }
                    } else {
                        if files.len() >= MAX_OBJECTS {
                            complete = false;
                            break 'walk;
                        }
                        files.push(entry);
                    }
                }
                match listing.next_token {
                    Some(next) if !next.is_empty() => token = Some(next),
                    _ => break,
                }
            }
        }
        report.truncated = !complete;

        // --- what we already hold for this source ---------------------------
        let existing_rows: Vec<ExistingRow> = self
            .db
            .fetch_all_as::<ExistingRow>(
                "SELECT id, source_key, source_signature, source_state, status FROM one_rag_documents \
                 WHERE source_id = ? AND source_key IS NOT NULL",
                &db_params![&source.id],
            )
            .await
            .map_err(|e| e.to_string())?;
        let existing: HashMap<String, ExistingDoc> = existing_rows
            .into_iter()
            .map(|(id, key, sig, state, status)| (key, (id, sig, state, status)))
            .collect();

        // --- bring each file up to date --------------------------------------
        let mut seen: HashSet<String> = HashSet::new();
        for entry in files {
            let name = file_name(&entry.rel).to_owned();
            // Editor lock files and dotfiles are never knowledge.
            if name.starts_with('.') || name.starts_with("~$") {
                continue;
            }
            seen.insert(entry.key.clone());
            let extension = extension_of(&name);
            if extension == "pdf" {
                skipped.push(
                    &entry.key,
                    "PDF: the server cannot read PDF text yet, so it was not imported",
                );
                continue;
            }
            let known = supported_extensions().contains(&extension.as_str());
            if !known {
                skipped.push(&entry.key, "file type is not supported for import");
                continue;
            }
            if !wanted.contains(&extension) {
                continue;
            }
            if entry.key.chars().count() > MAX_KEY_CHARS {
                skipped.push(&entry.key, "path is longer than 512 characters");
                continue;
            }
            if entry.size_bytes.is_some_and(|size| size as u64 > MAX_FILE_BYTES) {
                skipped.push(&entry.key, "file is larger than 50 MiB");
                continue;
            }
            let sig = signature(&entry);
            let previous = existing.get(&entry.key);
            if let Some((id, Some(old_sig), state, status)) = previous
                && *old_sig == sig
            {
                if status == "error" {
                    // The file is unchanged but its indexing failed (the
                    // embedding service was down or overloaded). The text is
                    // already stored, so this run only has to queue it again;
                    // otherwise it would stay unsearchable until someone
                    // edited the source file.
                    self.schedule_indexing(source.created_by.clone(), id.clone());
                }
                if state.as_deref() == Some("missing") {
                    // It came back; nothing to re-read.
                    let _ = self
                        .db
                        .execute(
                            "UPDATE one_rag_documents SET source_state = 'ok' WHERE id = ?",
                            &db_params![id],
                        )
                        .await;
                }
                report.unchanged += 1;
                continue;
            }
            match self
                .import_one(
                    &external,
                    &root,
                    source,
                    &entry,
                    &name,
                    &extension,
                    &sig,
                    previous.map(|p| p.0.as_str()),
                )
                .await
            {
                Ok(true) => report.added += 1,
                Ok(false) => report.updated += 1,
                Err(error) => failed.push(&entry.key, error.to_string()),
            }
        }

        // --- what is gone ------------------------------------------------------
        // Only judged when the whole folder was read: a capped or half-listed
        // walk would otherwise flag everything it did not reach.
        if complete && failed.total == 0 {
            for (key, (id, _, state, _)) in &existing {
                if !seen.contains(key) && state.as_deref() != Some("missing") {
                    let _ = self
                        .db
                        .execute(
                            "UPDATE one_rag_documents SET source_state = 'missing' WHERE id = ?",
                            &db_params![id],
                        )
                        .await;
                }
            }
        }
        report.missing = self
            .db
            .fetch_one_scalar::<i64>(
                "SELECT COUNT(*) FROM one_rag_documents WHERE source_id = ? AND source_state = 'missing'",
                &db_params![&source.id],
            )
            .await
            .unwrap_or(0) as u32;

        report.skipped = skipped.items;
        report.skipped_total = skipped.total;
        report.failed = failed.items;
        report.failed_total = failed.total;
        if report.truncated {
            report.message = format!(
                "the folder is larger than one run reads ({MAX_OBJECTS} files, {MAX_DEPTH} levels); \
                 point the source at a smaller folder. Missing files were not judged."
            );
        }
        Ok(())
    }

    /// Returns `true` when a new document was created, `false` when an existing
    /// one was updated.
    #[allow(clippy::too_many_arguments)]
    async fn import_one(
        self: &Arc<Self>,
        external: &Arc<dyn ExternalObjectSource>,
        root: &std::path::Path,
        source: &SourceRow,
        entry: &ExternalEntry,
        name: &str,
        extension: &str,
        sig: &str,
        existing_id: Option<&str>,
    ) -> Result<bool, DevopsError> {
        let actor = source.created_by.as_str();
        let bytes = external
            .read(actor, &source.storage_config_id, &entry.key, MAX_FILE_BYTES)
            .await
            .map_err(DevopsError::BadRequest)?;
        if bytes.is_empty() {
            return Err(DevopsError::BadRequest("file is empty".into()));
        }
        let ext = extension.to_owned();
        let for_text = bytes.clone();
        let text = tokio::task::spawn_blocking(move || extract_text(&ext, &for_text))
            .await
            .map_err(|e| DevopsError::Internal(e.to_string()))??;

        let (id, created) = match existing_id {
            Some(id) => (id.to_owned(), false),
            None => {
                let doc = self
                    .register_rag_document(
                        name,
                        None,
                        None,
                        None,
                        &source.scope,
                        source.team_id.as_deref(),
                        &source.visibility,
                        actor,
                        Some(&source.library_id),
                    )
                    .await?;
                (doc.id, true)
            }
        };
        let finish = async {
            // Tie the document to its source first, with no signature: if this
            // run dies from here on, the next one finds the document by key and
            // finishes it instead of creating a duplicate.
            self.db
                .execute(
                    "UPDATE one_rag_documents SET source_id = ?, source_key = ?, source_state = 'ok' WHERE id = ?",
                    &db_params![&source.id, &entry.key, &id],
                )
                .await?;
            crate::rag_files::retain_original(self, root, &id, name, &bytes).await?;
            self.set_document_content(actor, &id, &text).await?;
            // The signature is the last write: only a fully imported file is
            // ever treated as unchanged.
            self.db
                .execute(
                    "UPDATE one_rag_documents SET source_signature = ? WHERE id = ?",
                    &db_params![sig, &id],
                )
                .await?;
            Ok::<(), DevopsError>(())
        }
        .await;
        if let Err(error) = finish {
            // A half-made document would sit in the list with no original or
            // text; remove what this run created. An existing one keeps its
            // old signature, so the next run tries it again.
            if created {
                let _ = self.delete_rag_document(actor, &id).await;
            }
            return Err(error);
        }
        if !text.trim().is_empty() {
            self.schedule_indexing(actor.to_owned(), id);
        }
        Ok(created)
    }
}

#[cfg(test)]
#[path = "knowledge_import_test.rs"]
mod knowledge_import_test;
