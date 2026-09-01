//! DreamPro-mode gate: machine startup must not mint local-default-user rows.
//!
//! Historical failure mode: machine-level startup routines (agent probing,
//! generated-assistant reconcile, legacy skill-directory ingestion) ran
//! through "no acting user" convenience paths that mapped to
//! `system_default_user`, silently accumulating business rows under an
//! account that never logs in on an DreamPro machine. This e2e boots the full
//! router in DreamPro mode and then sweeps EVERY ownership column in the live
//! schema: outside the `users` table, no unexpected `system_default_user` rows
//! may exist.
//!
//! Under `--features enterprise` the first-run bootstrap
//! (`bootstrap_default_enterprise`) deliberately seats the deployment admin —
//! `system_default_user` — in the auto-provisioned default enterprise. Those
//! three rows are allow-listed below; every other ownership column must still
//! be clean, so a NEW accidental default-user write is still caught.

use sqlx::Row;

#[tokio::test]
async fn aionpro_startup_writes_no_system_default_user_rows() {
    let db = dream_core_db::init_database_memory().await.unwrap();
    let config = dream_core_app::AppConfig {
        identity_mode: dream_core_app::IdentityMode::AionPro,
        bootstrap_secret: Some("bootstrap-secret".to_string()),
        ..Default::default()
    };
    let services = dream_core_app::AppServices::from_config(db, &config).await.unwrap();
    // Full router construction runs every startup bootstrap (extension
    // registry, assistant storage, cron init, agent hydrate).
    let _router = dream_core_app::create_router(&services).await.expect("build router");

    let pool = services.database.pool();
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
            .fetch_all(pool)
            .await
            .unwrap();

    // The enterprise first-run bootstrap seats the deployment admin in the
    // auto-provisioned default enterprise. Empty in the personal build, so the
    // sweep there is byte-for-byte the historical one.
    let allowed: &[(&str, &str)] = if cfg!(feature = "enterprise") {
        &[
            ("one_user_org", "user_id"),
            ("one_active_tenant", "user_id"),
            ("one_enterprise_members", "user_id"),
        ]
    } else {
        &[]
    };

    let mut offenders = Vec::new();
    for table in tables {
        if table == "users" {
            continue;
        }
        let columns: Vec<String> = sqlx::query("SELECT name FROM pragma_table_info(?)")
            .bind(&table)
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>(0))
            .collect();
        for column in columns {
            if column != "user_id" && column != "owner_user_id" {
                continue;
            }
            let count: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM \"{table}\" WHERE \"{column}\" = 'system_default_user'"
            ))
            .fetch_one(pool)
            .await
            .unwrap();
            if count > 0 && !allowed.contains(&(table.as_str(), column.as_str())) {
                offenders.push(format!("{table}.{column} = {count} row(s)"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "AionPro startup minted local-default-user rows — a machine-level \
         routine is writing through a default-user path: {offenders:?}"
    );

    services.database.close().await;
}
