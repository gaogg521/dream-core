//! The deployment worker owns Docker and downloaded artifacts. The app only
//! forwards fixed operations after instance-level authorization.
use crate::error::PlatformError;
use dream_core_api_types::EnterpriseUpdateStatus;

pub(crate) async fn request(operation: &str, post: bool) -> Result<EnterpriseUpdateStatus, PlatformError> {
    let Ok(base) = std::env::var("ONE_ENTERPRISE_UPDATER_URL") else {
        return Ok(EnterpriseUpdateStatus {
            configured: false,
            phase: "unconfigured".into(),
            message: Some("当前部署尚未安装企业在线更新服务。请按部署手册完成一次初始化，后续可在此下载安装。".into()),
            ..Default::default()
        });
    };
    let token = std::env::var("ONE_ENTERPRISE_UPDATER_TOKEN")
        .map_err(|_| PlatformError::Internal("Updater authentication is not configured".into()))?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(40))
        .build()
        .map_err(|e| PlatformError::Internal(e.to_string()))?;
    let url = format!("{}/{operation}", base.trim_end_matches('/'));
    let request = if post { client.post(url) } else { client.get(url) };
    let response = request
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| PlatformError::Internal("企业更新服务暂时不可用，请检查部署服务状态后重试。".into()))?;
    let status = response.status();
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|_| PlatformError::Internal("Invalid updater response".into()))?;
    if !status.is_success() {
        return Err(PlatformError::BadRequest(
            value["message"].as_str().unwrap_or("Update operation failed").into(),
        ));
    }
    serde_json::from_value(value).map_err(|_| PlatformError::Internal("Invalid updater status".into()))
}

pub(crate) fn authorize(actor: &crate::service::PlatformActor) -> Result<(), PlatformError> {
    if !matches!(actor.role.as_str(), "system_admin" | "admin") {
        return Err(PlatformError::Forbidden(
            "Only a system administrator may manage deployment updates".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn org_admin_cannot_update_the_host() {
        let mut actor = crate::service::PlatformActor {
            tenant_id: "t".into(),
            role: "org_admin".into(),
        };
        assert!(authorize(&actor).is_err());
        actor.role = "system_admin".into();
        assert!(authorize(&actor).is_ok());
    }
}
