//! DTOs for the memory console. CamelCase over the wire; `tags` is stored as
//! a JSON array string and parsed once on the way out.

use serde::Serialize;

/// One memory collection, as every view (admin inventory / member's readable
/// list) shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCollectionDto {
    pub id: String,
    pub tenant_id: String,
    /// `"global" | "department" | "personal"`.
    pub scope: String,
    /// Set only for `department` collections.
    pub department_id: Option<String>,
    /// Set only for `personal` collections.
    pub owner_user_id: Option<String>,
    pub name: String,
    pub description: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One memory item. `trimmed` items still appear in listings (with their
/// status) — refinement is soft, so history stays auditable.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItemDto {
    pub id: String,
    pub collection_id: String,
    pub content: String,
    /// Hex SHA-256 of `content`, the refine job's duplicate-grouping key.
    pub content_hash: String,
    pub importance: f64,
    pub source_conversation_id: Option<String>,
    pub tags: Vec<String>,
    /// `"active" | "trimmed"`.
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Per-tenant memory-extraction settings (P2-2 followups §A.6). Absence of a
/// row = extraction disabled: the turn extractor keeps honouring explicit
/// 「记住…」 requests but never invokes an LLM.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct MemoryConfigDto {
    pub tenant_id: String,
    /// one-devops `one_provider_registry` id the extraction LLM calls go to.
    pub extraction_channel_id: Option<String>,
    /// Channel model to call; `None` = the channel's first configured model.
    pub extraction_model: Option<String>,
    pub updated_at: i64,
}

/// One synchronous refine run: how many duplicates were merged away and how
/// many low-value items were trimmed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRefineJobDto {
    pub id: String,
    pub collection_id: String,
    /// `"done" | "failed"`.
    pub status: String,
    pub merged_count: i64,
    pub trimmed_count: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

/// One read/write delegation on a collection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGrantDto {
    pub id: String,
    pub collection_id: String,
    /// `"member" | "department"`.
    pub subject_type: String,
    pub subject_id: String,
    /// `"read" | "write"`.
    pub access: String,
    pub granted_by: String,
    pub created_at: i64,
}

/// How much of the tenant can reach at least one active memory.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantCoverageDto {
    pub member_count: i64,
    pub covered_count: i64,
    /// `covered_count / member_count * 100`, rounded to two decimals.
    pub coverage_percent: f64,
}
