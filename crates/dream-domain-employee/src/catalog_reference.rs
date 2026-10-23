//! Versioned, backend-independent reference workflows for the employee catalog.
use dream_core_db::{DbPool, db_params};

use crate::error::EmployeeError;
use crate::models::{CatalogEntryRow, PersonalAgentRow};

pub struct CatalogReferenceGuide {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
}

pub fn reference_skill_id(key: &str) -> String {
    format!("builtin-employee-{key}-guide-v1")
}

pub(crate) fn reference_config(entry: &CatalogEntryRow) -> serde_json::Value {
    serde_json::json!({
        "persona": entry.persona,
        "boundSkillIds": [reference_skill_id(&entry.key)],
        "starterPrompts": [
            format!("请先说明完成“{}”需要我提供哪些资料，并列出执行步骤和验收标准。", entry.description),
            format!("请根据我提供的材料完成{}任务，区分已核实结论、风险和待确认事项。", entry.name),
            format!("请为{}生成一份可复用的检查清单和结果模板；没有实际证据的项目标记为待验证。", entry.name)
        ],
        "catalogReferenceVersion": 1
    })
}

pub async fn reference_guides(pool: &DbPool) -> Result<Vec<CatalogReferenceGuide>, EmployeeError> {
    let entries = pool
        .fetch_all_as::<CatalogEntryRow>("SELECT * FROM one_employee_catalog ORDER BY id", &[])
        .await?;
    Ok(entries.into_iter().map(|entry| {
        let tools: Vec<String> = serde_json::from_str(&entry.recommended_skills).unwrap_or_default();
        CatalogReferenceGuide {
            id: reference_skill_id(&entry.key),
            name: format!("onework-{}-guide", entry.key),
            description: format!("内置参考 · {}：{}", entry.name, entry.description),
            content: format!(
                "---\nname: onework-{}-guide\ndescription: {}工作指南\n---\n\n# {}工作指南\n\n## 使用场景\n{}\n\n## 工作步骤\n1. 明确任务范围、资料来源、时间范围和交付对象；资料不足时先列出缺口。\n2. 依据下面的岗位职责核对输入，给出可执行步骤；优先使用用户提供的材料。\n3. 区分事实、推断和待验证事项，保留证据出处，不虚构工具调用或执行结果。\n4. 输出结论、证据、风险、建议、负责人和验收标准；变更前列出影响及回滚方案。\n\n## 岗位职责与边界\n{}\n\n## 推荐工具\n{}\n工具名是配置参考，不表示已安装或已经授权。需要连接真实系统时，先确认管理员提供的连接和权限。\n\n## 参考任务\n请基于提供的材料完成{}，输出一份可复核的结果及待办清单。\n\n## 输出格式\n| 检查项 | 证据 | 结论 | 风险/待确认 | 下一步 |\n| --- | --- | --- | --- | --- |\n| 示例检查项 | 用户提供的资料或实际只读查询 | 已核实/待验证 | 缺失资料 | 负责人及验收标准 |\n",
                entry.key, entry.name, entry.name, entry.description, entry.persona,
                tools.join("、"), entry.description
            ),
        }
    }).collect())
}

/// Upgrade only untouched legacy catalog configs. Explicit empty bindings,
/// customer tasks, extra settings and customer personas are intentional edits.
pub async fn backfill_reference_bindings(pool: &DbPool) -> Result<u64, EmployeeError> {
    let entries = pool
        .fetch_all_as::<CatalogEntryRow>("SELECT * FROM one_employee_catalog ORDER BY id", &[])
        .await?;
    let mut changed = 0;
    for entry in entries {
        let rows = pool
            .fetch_all_as::<PersonalAgentRow>(
                "SELECT * FROM one_personal_agents WHERE origin = 'catalog' AND name = ?",
                &db_params![&entry.name],
            )
            .await?;
        for row in rows {
            let Ok(serde_json::Value::Object(config)) = serde_json::from_str(&row.automation_config) else {
                continue;
            };
            if config.len() != 1
                || config.get("persona").and_then(serde_json::Value::as_str) != Some(entry.persona.as_str())
            {
                continue;
            }
            changed += pool
                .execute(
                    "UPDATE one_personal_agents SET automation_config = ? WHERE id = ? AND automation_config = ?",
                    &db_params![reference_config(&entry).to_string(), &row.id, &row.automation_config],
                )
                .await?;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_catalog_employee_has_a_matching_guide_and_three_tasks() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let pool = DbPool::Sqlite(db.pool().clone());
        crate::run_one_employee_migrations(&pool).await.unwrap();
        let entries = crate::catalog::list_catalog(&pool, "tenant-a").await.unwrap();
        let guides = reference_guides(&pool).await.unwrap();
        assert_eq!(guides.len(), 28);
        assert_eq!(entries.len(), 28);
        let rows = pool
            .fetch_all_as::<PersonalAgentRow>("SELECT * FROM one_personal_agents", &[])
            .await
            .unwrap();
        for row in rows {
            let config: serde_json::Value = serde_json::from_str(&row.automation_config).unwrap();
            let id = config["boundSkillIds"][0].as_str().unwrap();
            assert!(
                guides
                    .iter()
                    .any(|guide| guide.id == id && guide.content.contains(&row.name))
            );
            assert_eq!(config["starterPrompts"].as_array().unwrap().len(), 3);
            assert!(
                config.get("instructions").is_none(),
                "examples must not become a scheduled task"
            );
        }
        assert_eq!(backfill_reference_bindings(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn legacy_backfill_is_idempotent_and_keeps_explicit_customer_settings() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let pool = DbPool::Sqlite(db.pool().clone());
        crate::run_one_employee_migrations(&pool).await.unwrap();
        crate::catalog::list_catalog(&pool, "tenant-a").await.unwrap();
        let rows = pool
            .fetch_all_as::<PersonalAgentRow>("SELECT * FROM one_personal_agents ORDER BY name", &[])
            .await
            .unwrap();
        let original: serde_json::Value = serde_json::from_str(&rows[0].automation_config).unwrap();
        let legacy = serde_json::json!({"persona": original["persona"]}).to_string();
        let explicit_empty = serde_json::json!({"persona": "Customer", "boundSkillIds": []}).to_string();
        let customer_task = serde_json::json!({"persona": "Customer", "instructions": "Customer task"}).to_string();
        for (row, config) in [
            (&rows[0], &legacy),
            (&rows[1], &explicit_empty),
            (&rows[2], &customer_task),
        ] {
            pool.execute(
                "UPDATE one_personal_agents SET automation_config = ? WHERE id = ?",
                &db_params![config, &row.id],
            )
            .await
            .unwrap();
        }
        assert_eq!(backfill_reference_bindings(&pool).await.unwrap(), 1);
        assert_eq!(backfill_reference_bindings(&pool).await.unwrap(), 0);
        for (row, config) in [(&rows[1], explicit_empty), (&rows[2], customer_task)] {
            let saved: String = pool
                .fetch_one_scalar(
                    "SELECT automation_config FROM one_personal_agents WHERE id = ?",
                    &db_params![&row.id],
                )
                .await
                .unwrap();
            assert_eq!(saved, config);
        }
    }
}
