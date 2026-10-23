//! Knowledge import sources: a folder in a registered file storage that is
//! mirrored into a company knowledge space.

use serde::{Deserialize, Serialize};

/// One thing a run could not take as-is, with the reason an administrator can
/// act on. Kept short and bounded so a huge folder cannot bloat the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeImportIssue {
    pub key: String,
    pub reason: String,
}

/// What the last run did. Stored with the source so the console can show it
/// without a run being in progress.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeImportReport {
    /// `ok`, `partial` (some files failed), `error` (nothing could be read),
    /// or `paused` (the source's owner can no longer manage company knowledge).
    pub status: String,
    pub message: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    /// Documents whose source file is no longer in the storage. Kept, flagged.
    pub missing: u32,
    /// Files seen but not imported, with why (PDF, unsupported type, too large).
    pub skipped: Vec<KnowledgeImportIssue>,
    pub skipped_total: u32,
    /// Files that were attempted and failed.
    pub failed: Vec<KnowledgeImportIssue>,
    pub failed_total: u32,
    /// The walk hit its object or depth cap, so "missing" was not judged.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeImportSource {
    pub id: String,
    pub name: String,
    pub storage_config_id: String,
    pub prefix: String,
    pub library_id: String,
    /// `org` or `team`.
    pub scope: String,
    pub team_id: Option<String>,
    /// `all` or `admin`.
    pub visibility: String,
    pub extensions: Vec<String>,
    /// `manual`, `hourly` or `daily`.
    pub schedule: String,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: i64,
    pub last_run_at: Option<i64>,
    pub last_report: Option<KnowledgeImportReport>,
    /// A run is in progress right now.
    pub running: bool,
    /// Documents this source currently owns.
    pub document_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveKnowledgeImportSource {
    pub name: String,
    pub storage_config_id: String,
    #[serde(default)]
    pub prefix: String,
    pub library_id: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}
