use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSelectionDto {
    pub selected_ids: Vec<String>,
    pub max_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCatalogItem {
    pub id: String,
    pub name: String,
}

/// Experimental console entry visibility; disabling never deletes saved data.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaboratorySettingsDto {
    #[serde(default)]
    pub enabled_features: Vec<String>,
}
