//! Per-user delivery settings and durable receipts for completed employee runs.
use crate::{
    EmployeeError,
    models::{EmployeeRunRow, RUN_SUCCESS},
};
use dream_core_api_types::{CronPushTarget, EmployeeNotificationSettings};
use dream_core_common::now_ms;
use dream_core_conversation::ConversationService;
use dream_core_db::{DbPool, IConversationRepository, db_params};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

#[async_trait::async_trait]
pub trait EmployeeResultNotifier: Send + Sync {
    async fn validate(&self, owner: &str, target: &CronPushTarget) -> Result<(), String>;
    async fn send(&self, owner: &str, target: &CronPushTarget, text: &str, files: &[PathBuf]) -> Result<(), String>;
}

pub struct EmployeeNotifications {
    db: DbPool,
    conversations: Arc<ConversationService>,
    work_dir: PathBuf,
    conversation_repo: Arc<dyn IConversationRepository>,
    notifier: OnceLock<Arc<dyn EmployeeResultNotifier>>,
}

impl EmployeeNotifications {
    pub fn new(
        db: DbPool,
        conversations: Arc<ConversationService>,
        work_dir: PathBuf,
        conversation_repo: Arc<dyn IConversationRepository>,
    ) -> Self {
        Self {
            db,
            conversations,
            work_dir,
            conversation_repo,
            notifier: OnceLock::new(),
        }
    }

    pub fn bind(&self, notifier: Arc<dyn EmployeeResultNotifier>) {
        let _ = self.notifier.set(notifier);
    }

    pub async fn get(&self, owner: &str, agent: &str) -> Result<EmployeeNotificationSettings, EmployeeError> {
        let row: Option<(String,)> = self
            .db
            .fetch_optional_as(
                "SELECT target FROM one_employee_notifications WHERE agent_id = ? AND owner_user_id = ?",
                &db_params![agent, owner],
            )
            .await?;
        let target = row
            .map(|(raw,)| serde_json::from_str(&raw))
            .transpose()
            .map_err(|_| EmployeeError::Internal("invalid saved notification target".into()))?;
        Ok(EmployeeNotificationSettings { target })
    }

    pub async fn set(
        &self,
        owner: &str,
        agent: &str,
        settings: EmployeeNotificationSettings,
    ) -> Result<(), EmployeeError> {
        if let Some(target) = &settings.target {
            let notifier = self
                .notifier
                .get()
                .ok_or_else(|| EmployeeError::BadRequest("Bot notifications are unavailable".into()))?;
            notifier
                .validate(owner, target)
                .await
                .map_err(EmployeeError::BadRequest)?;
        }
        let mut tx = self.db.begin().await?;
        tx.execute(
            "DELETE FROM one_employee_notifications WHERE agent_id = ? AND owner_user_id = ?",
            &db_params![agent, owner],
        )
        .await?;
        if let Some(target) = settings.target {
            let raw = serde_json::to_string(&target).map_err(|e| EmployeeError::Internal(e.to_string()))?;
            tx.execute(
                "INSERT INTO one_employee_notifications (agent_id, owner_user_id, target) VALUES (?, ?, ?)",
                &db_params![agent, owner, raw],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn deliver(&self, run_id: &str) {
        if let Err(error) = self.deliver_inner(run_id).await {
            let _ = error;
            tracing::warn!(run_id, "employee result notification failed; task outcome is unchanged");
            let _ = self
                .db
                .execute(
                    "UPDATE one_employee_deliveries SET status = 'failed', updated_at = ? WHERE run_id = ?",
                    &db_params![now_ms() as i64, run_id],
                )
                .await;
        }
    }

    async fn deliver_inner(&self, run_id: &str) -> Result<(), EmployeeError> {
        let run: EmployeeRunRow = self
            .db
            .fetch_one_as("SELECT * FROM one_employee_runs WHERE id = ?", &db_params![run_id])
            .await?;
        if run.trigger_source == "chat" || run.status == "running" {
            return Ok(());
        }
        let Some(target) = self.get(&run.owner_user_id, &run.agent_id).await?.target else {
            return Ok(());
        };
        let notifier = self
            .notifier
            .get()
            .ok_or_else(|| EmployeeError::Internal("Bot notifications are unavailable".into()))?;
        // The unique receipt prevents duplicate sends. Ambiguous network failures are
        // deliberately not retried automatically, because the platform may have sent it.
        if !claim_delivery(&self.db, run_id).await? {
            return Ok(());
        }
        notifier
            .validate(&run.owner_user_id, &target)
            .await
            .map_err(EmployeeError::BadRequest)?;
        let name: (String,) = self
            .db
            .fetch_one_as(
                "SELECT name FROM one_personal_agents WHERE id = ?",
                &db_params![&run.agent_id],
            )
            .await?;
        let success = run.status == RUN_SUCCESS;
        let reply = if success {
            self.conversations
                .latest_assistant_reply(&run.owner_user_id, &run.conversation_id)
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            String::new()
        };
        let detail = if success {
            run.summary.as_deref().unwrap_or("Task completed")
        } else {
            run.error.as_deref().unwrap_or("Task failed without error details")
        };
        let text = format!(
            "{} · {}\n{}\nRun: {}",
            name.0,
            if success { "Completed" } else { "Failed" },
            detail.chars().take(1200).collect::<String>(),
            run_id
        );
        let mut files = Vec::new();
        if success {
            let dir = self.work_dir.join("employee-results").join(run_id);
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| EmployeeError::Internal(e.to_string()))?;
            let report = dir.join("result.md");
            tokio::fs::write(
                &report,
                format!("# {}\n\n{}", name.0, if reply.is_empty() { detail } else { &reply }),
            )
            .await
            .map_err(|e| EmployeeError::Internal(e.to_string()))?;
            files.push(report);
            if let Some(conversation) = self
                .conversation_repo
                .get(&run.owner_user_id, &run.conversation_id)
                .await?
            {
                let extra: serde_json::Value = serde_json::from_str(&conversation.extra).unwrap_or_default();
                if let Some(workspace) = extra["workspace"].as_str() {
                    files.extend(result_attachments(&reply, std::path::Path::new(workspace)).await);
                }
            }
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(120),
            notifier.send(&run.owner_user_id, &target, &text, &files),
        )
        .await
        .map_err(|_| EmployeeError::Internal("notification delivery timed out; receipt is uncertain".into()))?
        .map_err(EmployeeError::Internal)?;
        self.db
            .execute(
                "UPDATE one_employee_deliveries SET status = 'sent', updated_at = ? WHERE run_id = ?",
                &db_params![now_ms() as i64, run_id],
            )
            .await?;
        tracing::info!(run_id, "employee result notification sent");
        Ok(())
    }
}

async fn claim_delivery(db: &DbPool, run: &str) -> Result<bool, EmployeeError> {
    let existing: Option<(String,)> = db
        .fetch_optional_as(
            "SELECT status FROM one_employee_deliveries WHERE run_id = ?",
            &db_params![run],
        )
        .await?;
    if existing.is_some() {
        return Ok(false);
    }
    match db
        .execute(
            "INSERT INTO one_employee_deliveries (run_id, status, updated_at) VALUES (?, 'sending', ?)",
            &db_params![run, now_ms() as i64],
        )
        .await
    {
        Ok(_) => Ok(true),
        Err(error) => {
            let receipt: Option<(String,)> = db
                .fetch_optional_as(
                    "SELECT status FROM one_employee_deliveries WHERE run_id = ?",
                    &db_params![run],
                )
                .await?;
            if receipt.is_some() {
                Ok(false)
            } else {
                Err(error.into())
            }
        }
    }
}

/// Only files linked in the final reply and contained in the owned workspace
/// are eligible. Never attach arbitrary model-mentioned paths or remote URLs.
async fn result_attachments(reply: &str, workspace: &std::path::Path) -> Vec<PathBuf> {
    let Ok(root) = tokio::fs::canonicalize(workspace).await else {
        return vec![];
    };
    let mut files = Vec::new();
    let mut total = 0;
    for part in reply.split("](").skip(1) {
        let Some(link) = part.split(')').next() else {
            continue;
        };
        let raw = link.trim().trim_matches(['<', '>']);
        if raw.contains("://") && !raw.starts_with("file://") {
            continue;
        }
        let path = if raw.starts_with("file://") {
            let Ok(url) = url::Url::parse(raw) else {
                continue;
            };
            let Ok(path) = url.to_file_path() else {
                continue;
            };
            path
        } else if std::path::Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            root.join(raw)
        };
        let Ok(path) = tokio::fs::canonicalize(path).await else {
            continue;
        };
        if !path.starts_with(&root) || files.contains(&path) {
            continue;
        }
        let Ok(meta) = tokio::fs::metadata(&path).await else {
            continue;
        };
        if !meta.is_file() || meta.len() == 0 || meta.len() > 20 * 1024 * 1024 || total + meta.len() > 40 * 1024 * 1024
        {
            continue;
        }
        total += meta.len();
        files.push(path);
        if files.len() == 5 {
            break;
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_urls_decode_spaces_and_reject_files_outside_workspace() {
        let root = std::env::temp_dir().join(format!("employee-notification-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let report = workspace.join("report with spaces.csv");
        let secret = root.join("secret.txt");
        tokio::fs::write(&report, "report").await.unwrap();
        tokio::fs::write(&secret, "private").await.unwrap();
        let reply = format!(
            "[report]({}) [outside]({})",
            url::Url::from_file_path(&report).unwrap(),
            url::Url::from_file_path(&secret).unwrap()
        );
        let files = result_attachments(&reply, &workspace).await;
        assert_eq!(files, vec![tokio::fs::canonicalize(report).await.unwrap()]);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn delivery_receipt_is_durable_and_prevents_duplicates() {
        let database = dream_core_db::init_database_memory().await.unwrap();
        let db = DbPool::Sqlite(database.pool().clone());
        crate::run_one_employee_migrations(&db).await.unwrap();
        assert!(claim_delivery(&db, "run-notify-test").await.unwrap());
        assert!(!claim_delivery(&db, "run-notify-test").await.unwrap());
    }

    #[tokio::test]
    async fn attachments_reject_outside_workspace_and_deduplicate() {
        let root = std::env::temp_dir().join(format!("employee-notification-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("workspace")).await.unwrap();
        tokio::fs::write(root.join("secret.txt"), "private").await.unwrap();
        tokio::fs::write(root.join("workspace/report.csv"), "a,b\n1,2")
            .await
            .unwrap();
        let files = result_attachments(
            "[report](report.csv) [duplicate](report.csv) [secret](../secret.txt) [web](https://example.com/x)",
            &root.join("workspace"),
        )
        .await;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap(), "report.csv");
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
