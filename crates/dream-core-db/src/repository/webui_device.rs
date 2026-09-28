use crate::error::DbError;

#[derive(Debug, Clone)]
pub struct WebuiDeviceSession {
    pub id: String,
    pub user_id: String,
    pub label: String,
    pub token_hash: String,
    pub paired_at: i64,
    pub last_seen_at: i64,
    pub last_ip: Option<String>,
    pub revoked_at: Option<i64>,
}

#[async_trait::async_trait]
pub trait IWebuiDeviceRepository: Send + Sync {
    async fn create(&self, device: &WebuiDeviceSession) -> Result<(), DbError>;
    async fn is_active(&self, user_id: &str, device_id: &str) -> Result<bool, DbError>;
    async fn touch(&self, user_id: &str, device_id: &str, ip: Option<&str>) -> Result<(), DbError>;
    async fn revoke(&self, user_id: &str, device_id: &str) -> Result<bool, DbError>;
    async fn list_active(&self, user_id: &str) -> Result<Vec<WebuiDeviceSession>, DbError>;
    async fn audit(
        &self,
        user_id: &str,
        device_id: Option<&str>,
        action: &str,
        detail: Option<&str>,
        ip: Option<&str>,
    ) -> Result<(), DbError>;
}
