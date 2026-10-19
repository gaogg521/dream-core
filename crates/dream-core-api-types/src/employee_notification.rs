use serde::{Deserialize, Serialize};

/// A recipient selected from the current user's connected bot sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmployeeNotificationSettings {
    pub target: Option<crate::CronPushTarget>,
}
