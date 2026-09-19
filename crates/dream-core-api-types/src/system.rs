use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Response for `GET /api/settings`.
///
/// Returns all backend system settings with their current values.
/// When no settings exist in the database, the service layer returns defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemSettingsResponse {
    pub language: String,
    pub notification_enabled: bool,
    pub cron_notification_enabled: bool,
    pub command_queue_enabled: bool,
    pub save_upload_to_workspace: bool,
}

impl Default for SystemSettingsResponse {
    fn default() -> Self {
        Self {
            language: "en-US".to_owned(),
            notification_enabled: true,
            cron_notification_enabled: false,
            command_queue_enabled: false,
            save_upload_to_workspace: false,
        }
    }
}

/// Request body for `PATCH /api/settings`.
///
/// All fields are optional — only the fields present in the request body
/// are updated. Unknown fields are silently ignored by serde.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateSettingsRequest {
    pub language: Option<String>,
    pub notification_enabled: Option<bool>,
    pub cron_notification_enabled: Option<bool>,
    pub command_queue_enabled: Option<bool>,
    pub save_upload_to_workspace: Option<bool>,
}

impl UpdateSettingsRequest {
    /// Returns `true` if all fields are `None` (no-op update).
    pub fn is_empty(&self) -> bool {
        self.language.is_none()
            && self.notification_enabled.is_none()
            && self.cron_notification_enabled.is_none()
            && self.command_queue_enabled.is_none()
            && self.save_upload_to_workspace.is_none()
    }
}

/// Response for `GET /api/settings/client`.
///
/// A flat key-value map where values can be any JSON type (string,
/// number, boolean). The service layer deserializes stored JSON strings
/// back to their original types.
pub type ClientPreferencesResponse = HashMap<String, Value>;

/// Request body for `PUT /api/settings/client`.
///
/// A flat key-value map for batch updates. A `null` value means
/// the key should be deleted. Non-null values are persisted as-is.
pub type UpdateClientPreferencesRequest = HashMap<String, Value>;

/// Query parameters for `GET /api/system/diagnostics/feedback-report`.
///
/// The UI sends only routing and explicit context. dream-core owns profile
/// resolution, SQL selection, and redaction.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedbackDiagnosticsQuery {
    pub route_at_open: Option<String>,
    pub route_at_submit: Option<String>,
    pub selected_module: Option<String>,
    /// Optional comma-separated kebab-case profile names.
    pub profiles: Option<String>,
    pub conversation_id: Option<String>,
    pub provider_id: Option<String>,
    pub agent_id: Option<String>,
    pub team_id: Option<String>,
    pub mcp_server_id: Option<String>,
}

/// Response for `GET /api/system/diagnostics/feedback-report`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedbackDiagnosticsResponse {
    pub schema_version: String,
    pub context: FeedbackDiagnosticsContextResponse,
    pub profiles: Vec<FeedbackDiagnosticsProfileResponse>,
    pub privacy: FeedbackDiagnosticsPrivacyResponse,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedbackDiagnosticsContextResponse {
    pub route_at_open: Option<String>,
    pub route_at_submit: Option<String>,
    pub selected_module: Option<String>,
    pub conversation_id: Option<String>,
    pub provider_id: Option<String>,
    pub agent_id: Option<String>,
    pub team_id: Option<String>,
    pub mcp_server_id: Option<String>,
    pub selected_profiles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedbackDiagnosticsProfileResponse {
    pub name: String,
    pub mode: String,
    pub data: Value,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedbackDiagnosticsPrivacyResponse {
    pub redaction: String,
    pub raw_content_included: bool,
    pub api_keys_included: bool,
}

/// The kinds of data a backup carries. Selected on export, and again on
/// restore to narrow what gets merged.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupScopeDto {
    #[serde(default)]
    pub conversations: bool,
    #[serde(default)]
    pub attachments: bool,
    #[serde(default)]
    pub providers: bool,
    #[serde(default)]
    pub skills: bool,
    #[serde(default)]
    pub app_settings: bool,
}

/// Write a backup archive to `destination`.
///
/// The path comes from the client because the file dialog lives there: the
/// backend is a local service and writes where the user chose, rather than
/// streaming a multi-gigabyte body through HTTP.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateBackupRequest {
    pub destination: String,
    pub scope: BackupScopeDto,
    /// Passphrase the archive is encrypted with. Required: an archive always
    /// carries the install's identity secret, so there is no scope that is safe
    /// to write in the clear.
    #[serde(default)]
    pub passphrase: String,
}

/// Wire form of `ArchiveEncryption`. Carries only what the client needs to know
/// an archive IS sealed — cipher and KDF name, for display. `salt` and
/// `verifier` stay server-side: the client never decrypts anything itself, it
/// only types a passphrase and the backend does the actual unlock, so there is
/// no reason for those to leave the machine that already has the archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEncryptionResponse {
    pub cipher: String,
    pub kdf: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifestResponse {
    pub format_version: u32,
    pub exported_at: i64,
    pub app_version: String,
    pub scope: BackupScopeDto,
    pub total_bytes: u64,
    /// True when the archive carries decryptable provider credentials, so the
    /// UI can tell the user to store the file accordingly.
    pub contains_credentials: bool,
    /// Present when the archive is sealed, absent for a version-2 archive
    /// written before encryption existed. The desktop UI's ENTIRE decision
    /// about whether to show a passphrase field before restoring reads this
    /// field's truthiness -- it was missing from this DTO from the day
    /// encryption shipped, so that prompt has never once appeared for a real
    /// user: `preview` always came back without it, restore was always
    /// attempted with an empty passphrase, and every restore of an encrypted
    /// archive failed with "That passphrase does not open this backup" before
    /// the passphrase field a person could type into ever appeared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<ArchiveEncryptionResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateBackupResponse {
    pub path: String,
    /// Size of the archive on disk, which is smaller than the manifest's
    /// `totalBytes` because entries are deflated.
    pub archive_bytes: u64,
    pub manifest: BackupManifestResponse,
}

/// Inspect an archive without applying it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewBackupRequest {
    pub source: String,
}

/// Merge an archive into this install. `scope` narrows what is applied;
/// categories the archive does not carry are ignored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreBackupRequest {
    pub source: String,
    pub scope: BackupScopeDto,
    /// Passphrase that opens the archive. Ignored for a version 2 archive,
    /// written before encryption existed.
    #[serde(default)]
    pub passphrase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreBackupResponse {
    /// Rows merged into the live catalog, keyed by table.
    pub rows_by_table: std::collections::BTreeMap<String, u64>,
    pub files_restored: u64,
    /// Rows dropped because the parent they referenced was not part of the
    /// restore — restoring conversations without app settings leaves the
    /// assistant snapshots pointing at definitions that never arrived. Reported
    /// so a partial restore does not look lossless when it was not.
    #[serde(default)]
    pub orphans_removed: std::collections::BTreeMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- SystemSettingsResponse --

    #[test]
    fn test_settings_response_default() {
        let resp = SystemSettingsResponse::default();
        assert_eq!(resp.language, "en-US");
        assert!(resp.notification_enabled);
        assert!(!resp.cron_notification_enabled);
        assert!(!resp.command_queue_enabled);
        assert!(!resp.save_upload_to_workspace);
    }

    #[test]
    fn test_settings_response_serialization_snake_case() {
        let resp = SystemSettingsResponse::default();
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["language"], "en-US");
        assert_eq!(json["notification_enabled"], true);
        assert_eq!(json["cron_notification_enabled"], false);
        assert_eq!(json["command_queue_enabled"], false);
        assert_eq!(json["save_upload_to_workspace"], false);
        // Verify snake_case, not camelCase
        assert!(json.get("notificationEnabled").is_none());
        assert!(json.get("cronNotificationEnabled").is_none());
    }

    #[test]
    fn test_settings_response_deserialization_snake_case() {
        let raw = json!({
            "language": "zh-CN",
            "notification_enabled": false,
            "cron_notification_enabled": true,
            "command_queue_enabled": true,
            "save_upload_to_workspace": true
        });
        let resp: SystemSettingsResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(resp.language, "zh-CN");
        assert!(!resp.notification_enabled);
        assert!(resp.cron_notification_enabled);
        assert!(resp.command_queue_enabled);
        assert!(resp.save_upload_to_workspace);
    }

    #[test]
    fn test_settings_response_roundtrip() {
        let original = SystemSettingsResponse {
            language: "ja-JP".to_owned(),
            notification_enabled: false,
            cron_notification_enabled: true,
            command_queue_enabled: true,
            save_upload_to_workspace: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: SystemSettingsResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    // -- UpdateSettingsRequest --

    #[test]
    fn test_update_request_partial_fields() {
        let raw = r#"{"language":"zh-CN"}"#;
        let req: UpdateSettingsRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.language.as_deref(), Some("zh-CN"));
        assert!(req.notification_enabled.is_none());
        assert!(req.cron_notification_enabled.is_none());
        assert!(req.command_queue_enabled.is_none());
        assert!(req.save_upload_to_workspace.is_none());
    }

    #[test]
    fn test_update_request_empty_body() {
        let raw = r#"{}"#;
        let req: UpdateSettingsRequest = serde_json::from_str(raw).unwrap();
        assert!(req.is_empty());
    }

    #[test]
    fn test_update_request_multiple_fields() {
        let raw = json!({
            "notification_enabled": false,
            "command_queue_enabled": true
        });
        let req: UpdateSettingsRequest = serde_json::from_value(raw).unwrap();
        assert!(req.language.is_none());
        assert_eq!(req.notification_enabled, Some(false));
        assert_eq!(req.command_queue_enabled, Some(true));
        assert!(!req.is_empty());
    }

    #[test]
    fn test_update_request_camel_case_ignored() {
        // camelCase keys are treated as unknown fields and silently ignored
        let raw = r#"{"notificationEnabled":true}"#;
        let req: UpdateSettingsRequest = serde_json::from_str(raw).unwrap();
        assert!(req.notification_enabled.is_none());
        assert!(req.is_empty());
    }

    #[test]
    fn test_update_request_unknown_field_ignored() {
        let raw = r#"{"unknownField":123}"#;
        let req: UpdateSettingsRequest = serde_json::from_str(raw).unwrap();
        assert!(req.is_empty());
    }

    // -- ClientPreferencesResponse / UpdateClientPreferencesRequest --

    #[test]
    fn test_client_preferences_response_empty() {
        let resp: ClientPreferencesResponse = HashMap::new();
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json, json!({}));
    }

    #[test]
    fn test_client_preferences_response_mixed_types() {
        let mut resp: ClientPreferencesResponse = HashMap::new();
        resp.insert("system.closeToTray".into(), json!(false));
        resp.insert("pet.size".into(), json!(280));
        resp.insert("theme".into(), json!("dark"));
        resp.insert("ui.zoomFactor".into(), json!(1.0));

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["system.closeToTray"], false);
        assert_eq!(json["pet.size"], 280);
        assert_eq!(json["theme"], "dark");
        assert_eq!(json["ui.zoomFactor"], 1.0);
    }

    #[test]
    fn test_update_client_preferences_with_null_delete() {
        let raw = json!({
            "theme": null,
            "pet.size": 360
        });
        let req: UpdateClientPreferencesRequest = serde_json::from_value(raw).unwrap();
        assert!(req["theme"].is_null());
        assert_eq!(req["pet.size"], 360);
    }
}
