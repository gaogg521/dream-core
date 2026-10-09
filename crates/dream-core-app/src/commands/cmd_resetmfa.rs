//! `dreamcore resetmfa` — break-glass MFA recovery straight in the on-disk
//! database.
//!
//! A mandatory MFA policy plus a failed enrollment can leave no administrator
//! able to reach the console, and the console is the only place MFA can be
//! reset. This gives the host operator a supported way out instead of editing
//! SQLite by hand. Same trust boundary as `resetpass`: it needs filesystem
//! access to the data dir, which already equals full control of the database.
//!
//! The reset is recorded in the MFA audit log (operator `cli`), so it shows up
//! in the console next to resets done there.

use std::process::ExitCode;
use std::sync::Arc;

use dream_core_db::{
    IUserRepository, MfaAuditEntry, MfaMode, MfaStore, SqliteMfaStore, SqliteUserRepository, init_database,
    maybe_copy_legacy_database,
};

use crate::cli::{Cli, ResetmfaArgs};
use crate::commands::error::{CliBoundaryCode, CliBoundaryError};

const SUBCOMMAND: &str = "resetmfa";
const OPERATOR: &str = "cli";

pub async fn run_resetmfa(cli: &Cli, args: &ResetmfaArgs) -> Result<ExitCode, CliBoundaryError> {
    let db_path = dream_core_common::backend_db_path(&cli.data_dir);
    maybe_copy_legacy_database(&db_path).map_err(|_| database_error())?;
    let database = init_database(&db_path).await.map_err(|_| database_error())?;

    let users: Arc<dyn IUserRepository> = Arc::new(SqliteUserRepository::new(database.pool().clone()));
    let store = SqliteMfaStore::new(database.pool().clone());

    let user = match args.username.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(username) => users.find_by_username(username).await.map_err(|_| database_error())?,
        None => users.get_primary_webui_user().await.map_err(|_| database_error())?,
    };
    let Some(user) = user else {
        database.close().await;
        return Err(CliBoundaryError::new(
            CliBoundaryCode::CliResetmfaUserNotFound,
            SUBCOMMAND,
            "resetmfa target user not found",
        ));
    };
    let username = user.username.clone().unwrap_or_else(|| "external_user".into());

    users.clear_mfa_binding(&user.id).await.map_err(|_| database_error())?;
    store
        .clear_pending_enroll_secrets(&user.id)
        .await
        .map_err(|_| database_error())?;
    let _ = store
        .audit_insert(&MfaAuditEntry {
            user_id: Some(user.id.clone()),
            username: Some(username.clone()),
            action: "mfa_reset",
            detail: Some(format!("by {OPERATOR}: break-glass resetmfa")),
            ip: None,
        })
        .await;

    if args.policy_off {
        store
            .policy_set(MfaMode::Off, OPERATOR)
            .await
            .map_err(|_| database_error())?;
        let _ = store
            .audit_insert(&MfaAuditEntry {
                user_id: None,
                username: Some(OPERATOR.into()),
                action: "mfa_policy_changed",
                detail: Some(MfaMode::Off.as_str().to_string()),
                ip: None,
            })
            .await;
    }
    database.close().await;

    println!("MFA binding cleared for user: {username}");
    if args.policy_off {
        println!("MFA policy switched off for this deployment.");
    } else {
        println!("If the policy is mandatory, they will be asked to enroll again at next login.");
    }

    Ok(ExitCode::SUCCESS)
}

fn database_error() -> CliBoundaryError {
    CliBoundaryError::new(
        CliBoundaryCode::CliResetmfaDatabaseFailed,
        SUBCOMMAND,
        "resetmfa failed to open or update the application database",
    )
}
