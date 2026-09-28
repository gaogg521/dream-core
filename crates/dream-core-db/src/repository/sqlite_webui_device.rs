use crate::error::DbError;
use crate::repository::{IWebuiDeviceRepository, WebuiDeviceSession};
use sqlx::SqlitePool;

#[derive(Clone, Debug)]
pub struct SqliteWebuiDeviceRepository {
    pool: SqlitePool,
}
impl SqliteWebuiDeviceRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl IWebuiDeviceRepository for SqliteWebuiDeviceRepository {
    async fn create(&self, d: &WebuiDeviceSession) -> Result<(), DbError> {
        sqlx::query("INSERT INTO webui_device_sessions (id,user_id,label,token_hash,paired_at,last_seen_at,last_ip,revoked_at) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET label=excluded.label,token_hash=excluded.token_hash,paired_at=excluded.paired_at,last_seen_at=excluded.last_seen_at,last_ip=excluded.last_ip,revoked_at=NULL WHERE webui_device_sessions.user_id=excluded.user_id")
            .bind(&d.id)
            .bind(&d.user_id)
            .bind(&d.label)
            .bind(&d.token_hash)
            .bind(d.paired_at)
            .bind(d.last_seen_at)
            .bind(&d.last_ip)
            .bind(d.revoked_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    async fn is_active(&self, user_id: &str, id: &str) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM webui_device_sessions WHERE id=? AND user_id=? AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?
            > 0)
    }
    async fn touch(&self, user_id: &str, id: &str, ip: Option<&str>) -> Result<(), DbError> {
        sqlx::query("UPDATE webui_device_sessions SET last_seen_at=?,last_ip=COALESCE(?,last_ip) WHERE id=? AND user_id=? AND revoked_at IS NULL").bind(dream_core_common::now_ms()).bind(ip).bind(id).bind(user_id).execute(&self.pool).await?;
        Ok(())
    }
    async fn revoke(&self, user_id: &str, id: &str) -> Result<bool, DbError> {
        Ok(
            sqlx::query(
                "UPDATE webui_device_sessions SET revoked_at=? WHERE id=? AND user_id=? AND revoked_at IS NULL",
            )
            .bind(dream_core_common::now_ms())
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?
            .rows_affected()
                == 1,
        )
    }
    async fn list_active(&self, user_id: &str) -> Result<Vec<WebuiDeviceSession>, DbError> {
        let rows = sqlx::query_as::<_, (String, String, String, String, i64, i64, Option<String>, Option<i64>)>(
            "SELECT id,user_id,label,token_hash,paired_at,last_seen_at,last_ip,revoked_at FROM webui_device_sessions WHERE user_id=? AND revoked_at IS NULL ORDER BY last_seen_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, user_id, label, token_hash, paired_at, last_seen_at, last_ip, revoked_at)| WebuiDeviceSession {
                    id,
                    user_id,
                    label,
                    token_hash,
                    paired_at,
                    last_seen_at,
                    last_ip,
                    revoked_at,
                },
            )
            .collect())
    }
    async fn audit(
        &self,
        user_id: &str,
        id: Option<&str>,
        action: &str,
        detail: Option<&str>,
        ip: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query("INSERT INTO webui_device_audit (ts,user_id,device_id,action,detail,ip) VALUES (?,?,?,?,?,?)")
            .bind(dream_core_common::now_ms())
            .bind(user_id)
            .bind(id)
            .bind(action)
            .bind(detail)
            .bind(ip)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repository() -> SqliteWebuiDeviceRepository {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE webui_device_sessions (id TEXT PRIMARY KEY NOT NULL,user_id TEXT NOT NULL,label TEXT NOT NULL,token_hash TEXT NOT NULL UNIQUE,paired_at INTEGER NOT NULL,last_seen_at INTEGER NOT NULL,last_ip TEXT,revoked_at INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE webui_device_audit (id INTEGER PRIMARY KEY AUTOINCREMENT,ts INTEGER NOT NULL,user_id TEXT NOT NULL,device_id TEXT,action TEXT NOT NULL,detail TEXT,ip TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        SqliteWebuiDeviceRepository::new(pool)
    }

    #[tokio::test]
    async fn device_session_can_be_revoked_without_affecting_other_sessions() {
        let repo = repository().await;
        let device = WebuiDeviceSession {
            id: "phone-1".into(),
            user_id: "user-1".into(),
            label: "手机浏览器".into(),
            token_hash: "hash-1".into(),
            paired_at: 1,
            last_seen_at: 1,
            last_ip: None,
            revoked_at: None,
        };
        repo.create(&device).await.unwrap();
        assert!(repo.is_active("user-1", "phone-1").await.unwrap());
        repo.touch("user-1", "phone-1", Some("192.168.1.3")).await.unwrap();
        assert_eq!(
            repo.list_active("user-1").await.unwrap()[0].last_ip.as_deref(),
            Some("192.168.1.3")
        );
        assert!(repo.revoke("user-1", "phone-1").await.unwrap());
        assert!(!repo.is_active("user-1", "phone-1").await.unwrap());
        assert!(repo.list_active("user-1").await.unwrap().is_empty());
    }
}
