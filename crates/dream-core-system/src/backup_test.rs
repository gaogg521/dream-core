//! Tests for personal-edition backup and restore.
//!
//! These build a small catalog with `sqlx` rather than going through
//! `init_database`: what is under test is category filtering, merge semantics
//! and archive handling, and running every migration would make the suite slow
//! without exercising any of it. The tables created here mirror the real
//! schema's shape where that shape matters — notably `projects`, which has the
//! surrogate INTEGER key the merge has to special-case.

use super::*;

use sqlx::Executor;
use sqlx::sqlite::SqlitePoolOptions;

fn sqlite_url(path: &Path) -> String {
    format!("sqlite://{}?mode=rwc", path.to_string_lossy().replace('\\', "/"))
}

/// A catalog in WAL mode holding one row per category, none of it
/// checkpointed — the state a running app is normally in.
///
/// The pool size matches production (`MAX_CONNECTIONS` in dream-core-db) and
/// that is load-bearing, not incidental. `ATTACH DATABASE` is per-connection:
/// with a pool of one, an attach issued against the pool and a query issued
/// against the pool necessarily land on the same connection, and a restore
/// that only works by that accident passes. It shipped that way once — the
/// first real run returned "Internal server error" while every test was green.
async fn seeded_catalog(path: &Path) -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&sqlite_url(path))
        .await
        .unwrap();
    pool.execute("PRAGMA journal_mode=WAL;").await.unwrap();
    // Foreign keys are declared here because the real schema declares 28 of
    // them and the live pool runs with enforcement ON. A fixture without them
    // cannot fail the way a real restore fails: it was green through the whole
    // life of the bug this file now covers.
    pool.execute(
        "CREATE TABLE users (id TEXT PRIMARY KEY NOT NULL, username TEXT NOT NULL, jwt_secret TEXT);
         CREATE TABLE conversations (
             id TEXT PRIMARY KEY NOT NULL,
             user_id TEXT NOT NULL REFERENCES users(id),
             name TEXT NOT NULL
         );
         CREATE TABLE messages (
             id TEXT PRIMARY KEY NOT NULL,
             conversation_id TEXT NOT NULL REFERENCES conversations(id),
             content TEXT NOT NULL
         );
         CREATE TABLE assistant_definitions (
             id TEXT PRIMARY KEY NOT NULL,
             user_id TEXT NOT NULL REFERENCES users(id),
             name TEXT NOT NULL
         );
         -- The edge that broke cross-machine restore: a CONVERSATION table
         -- pointing at an APP SETTINGS one, so the child is merged before the
         -- parent it needs.
         CREATE TABLE conversation_assistant_snapshots (
             id TEXT PRIMARY KEY NOT NULL,
             conversation_id TEXT NOT NULL REFERENCES conversations(id),
             assistant_definition_id TEXT NOT NULL REFERENCES assistant_definitions(id)
         );
         CREATE TABLE providers (id TEXT PRIMARY KEY NOT NULL, platform TEXT NOT NULL, api_key TEXT NOT NULL);
         CREATE TABLE skills (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL);
         CREATE TABLE assistants (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL);
         CREATE TABLE projects (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             project_id TEXT NOT NULL,
             name TEXT NOT NULL
         );
         CREATE UNIQUE INDEX idx_projects_project_id_unique ON projects(project_id);",
    )
    .await
    .unwrap();
    pool.execute(
        "INSERT INTO users VALUES ('u1', 'me', 'secret-that-unlocks-keys');
         INSERT INTO conversations VALUES ('c1', 'u1', 'Quarterly plan');
         INSERT INTO messages VALUES ('m1', 'c1', 'hello');
         INSERT INTO assistant_definitions VALUES ('ad1', 'u1', 'Researcher');
         INSERT INTO conversation_assistant_snapshots VALUES ('cs1', 'c1', 'ad1');
         INSERT INTO providers VALUES ('p1', 'openai', 'sk-live-key');
         INSERT INTO skills VALUES ('s1', 'my-skill');
         INSERT INTO assistants VALUES ('a1', 'Helper');
         INSERT INTO projects (project_id, name) VALUES ('proj-alpha', 'Alpha');",
    )
    .await
    .unwrap();
    pool
}

/// A catalog with the schema but no rows — a machine that has never seen this
/// user. Restoring here is the case that was broken: on the machine the
/// archive came from, every parent row already exists and nothing complains.
async fn empty_catalog(path: &Path) -> SqlitePool {
    let pool = seeded_catalog(path).await;
    pool.execute("PRAGMA foreign_keys=OFF").await.unwrap();
    for table in [
        "conversation_assistant_snapshots",
        "messages",
        "conversations",
        "assistant_definitions",
        "projects",
        "assistants",
        "skills",
        "providers",
        "users",
    ] {
        pool.execute(format!("DELETE FROM {table}").as_str()).await.unwrap();
    }
    pool.execute("PRAGMA foreign_keys=ON").await.unwrap();
    pool
}

fn service(data_dir: &Path) -> BackupService {
    BackupService::new(data_dir.to_path_buf(), "3.0.5".to_owned())
}

fn only_conversations() -> BackupScope {
    BackupScope {
        conversations: true,
        ..empty_scope()
    }
}

fn empty_scope() -> BackupScope {
    BackupScope {
        conversations: false,
        attachments: false,
        providers: false,
        skills: false,
        app_settings: false,
    }
}

/// Open the catalog inside an archive and read one table's row count.
async fn rows_in_archived_catalog(archive: &Path, table: &str) -> i64 {
    let dir = tempfile::tempdir().unwrap();
    unpack_archive(archive, dir.path()).unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&sqlite_url(&dir.path().join(ARCHIVE_DB_NAME)))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{table}\""))
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    count
}

/// The reason the catalog is copied with `VACUUM INTO` rather than by copying
/// the file: a plain copy of a WAL-mode database loses everything not yet
/// checkpointed, which on a running app is the user's most recent work.
#[tokio::test]
async fn the_archive_contains_rows_still_sitting_in_the_wal() {
    let dir = tempfile::tempdir().unwrap();
    let pool = seeded_catalog(&dir.path().join("one-backend.db")).await;
    let archive = dir.path().join("backup.zip");

    service(dir.path())
        .export(&DbPool::Sqlite(pool.clone()), &archive, only_conversations())
        .await
        .unwrap();

    assert_eq!(rows_in_archived_catalog(&archive, "conversations").await, 1);
    assert_eq!(rows_in_archived_catalog(&archive, "messages").await, 1);
}

/// The privacy rule this module is built around. Someone who asks for
/// conversations must not be handed a file with their API keys in it, so the
/// unselected categories are really deleted rather than merely unmarked.
#[tokio::test]
async fn an_unselected_category_is_absent_from_the_archive_not_just_unmarked() {
    let dir = tempfile::tempdir().unwrap();
    let pool = seeded_catalog(&dir.path().join("one-backend.db")).await;
    let archive = dir.path().join("conversations-only.zip");

    let manifest = service(dir.path())
        .export(&DbPool::Sqlite(pool.clone()), &archive, only_conversations())
        .await
        .unwrap();

    assert!(!manifest.contains_credentials);
    assert_eq!(rows_in_archived_catalog(&archive, "providers").await, 0);
    assert_eq!(rows_in_archived_catalog(&archive, "skills").await, 0);
    assert_eq!(rows_in_archived_catalog(&archive, "assistants").await, 0);
    // Identity survives every filter: the conversation rows are scoped to a
    // user, and without it they would restore unreachable.
    assert_eq!(rows_in_archived_catalog(&archive, "users").await, 1);

    // And the key itself is nowhere in the bytes on disk.
    let raw = std::fs::read(&archive).unwrap();
    assert!(
        !raw.windows(11).any(|window| window == b"sk-live-key"),
        "the archive still contains the provider key"
    );
}

/// The data-safety rule. A conversations-only archive has an empty `providers`
/// table; restoring it must merge conversations in, not wipe the model
/// configuration the user already has on this machine.
#[tokio::test]
async fn restoring_one_category_leaves_the_others_untouched() {
    let source = tempfile::tempdir().unwrap();
    let source_pool = seeded_catalog(&source.path().join("one-backend.db")).await;
    let archive = source.path().join("conversations-only.zip");
    service(source.path())
        .export(&DbPool::Sqlite(source_pool.clone()), &archive, only_conversations())
        .await
        .unwrap();

    // A different machine, with its own provider configured and no history.
    let target = tempfile::tempdir().unwrap();
    let target_pool = seeded_catalog(&target.path().join("one-backend.db")).await;
    target_pool
        .execute(
            "DELETE FROM conversation_assistant_snapshots;
             DELETE FROM messages;
             DELETE FROM conversations;",
        )
        .await
        .unwrap();
    target_pool
        .execute("UPDATE providers SET api_key = 'sk-the-local-one'")
        .await
        .unwrap();

    let outcome = service(target.path())
        .restore(&DbPool::Sqlite(target_pool.clone()), &archive, BackupScope::all())
        .await
        .unwrap();

    assert_eq!(outcome.rows_by_table.get("conversations"), Some(&1));
    let restored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversations")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(restored, 1);

    // The category that was not in the archive is exactly as it was.
    let key: String = sqlx::query_scalar("SELECT api_key FROM providers WHERE id = 'p1'")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(key, "sk-the-local-one");
}

/// The case the whole feature exists for, and the one that was broken:
/// restoring onto a machine that has never seen this user.
///
/// On the source machine every parent row is already present, so nothing ever
/// complains — which is exactly why this shipped. On a new machine
/// `conversation_assistant_snapshots` is merged with the conversations while
/// the `assistant_definitions` it points at arrive last with the app settings,
/// and the insert failed with `FOREIGN KEY constraint failed`. The user saw
/// only `500 INTERNAL_ERROR`.
#[tokio::test]
async fn a_full_backup_restores_onto_a_machine_that_has_none_of_this_data() {
    let source = tempfile::tempdir().unwrap();
    let source_pool = seeded_catalog(&source.path().join("one-backend.db")).await;
    let archive = source.path().join("everything.zip");
    service(source.path())
        .export(&DbPool::Sqlite(source_pool.clone()), &archive, BackupScope::all())
        .await
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    let target_pool = empty_catalog(&target.path().join("one-backend.db")).await;

    let outcome = service(target.path())
        .restore(&DbPool::Sqlite(target_pool.clone()), &archive, BackupScope::all())
        .await
        .expect("a full backup must restore onto a fresh machine");

    // Every category arrived, not just the ones before the first foreign key.
    assert_eq!(outcome.rows_by_table.get("conversations"), Some(&1));
    assert_eq!(outcome.rows_by_table.get("conversation_assistant_snapshots"), Some(&1));
    assert_eq!(outcome.rows_by_table.get("providers"), Some(&1));
    assert_eq!(outcome.rows_by_table.get("skills"), Some(&1));
    assert!(
        outcome.orphans_removed.is_empty(),
        "a full restore leaves nothing dangling, got {:?}",
        outcome.orphans_removed
    );

    // And the catalog keeps the constraints it declares.
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&target_pool)
        .await
        .unwrap();
    assert!(violations.is_empty(), "restore left the catalog inconsistent");
}

/// Restoring conversations WITHOUT app settings is a supported choice, and it
/// leaves the assistant snapshots pointing at definitions that were never
/// brought across. Those rows are unusable; keeping them would leave the
/// catalog contradicting its own constraints, so they are dropped and counted.
#[tokio::test]
async fn a_partial_restore_drops_rows_whose_parent_was_not_selected() {
    let source = tempfile::tempdir().unwrap();
    let source_pool = seeded_catalog(&source.path().join("one-backend.db")).await;
    let archive = source.path().join("everything.zip");
    service(source.path())
        .export(&DbPool::Sqlite(source_pool.clone()), &archive, BackupScope::all())
        .await
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    let target_pool = empty_catalog(&target.path().join("one-backend.db")).await;

    let outcome = service(target.path())
        .restore(&DbPool::Sqlite(target_pool.clone()), &archive, only_conversations())
        .await
        .expect("a partial restore must still succeed");

    assert_eq!(outcome.rows_by_table.get("conversations"), Some(&1));
    assert_eq!(
        outcome.orphans_removed.get("conversation_assistant_snapshots"),
        Some(&1),
        "the snapshot has no definition to point at and must be reported, not hidden"
    );

    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&target_pool)
        .await
        .unwrap();
    assert!(
        violations.is_empty(),
        "a partial restore must still leave a consistent catalog"
    );
}

/// A restore that cannot finish must leave nothing behind.
///
/// Before the merge ran in a transaction it committed table by table: measured
/// against a real archive, the target ended up with 43 conversations and 2297
/// messages but zero providers and zero skills, and the only signal was a 500.
#[tokio::test]
async fn a_failed_restore_leaves_the_catalog_untouched() {
    let source = tempfile::tempdir().unwrap();
    let source_pool = seeded_catalog(&source.path().join("one-backend.db")).await;
    let archive = source.path().join("everything.zip");
    service(source.path())
        .export(&DbPool::Sqlite(source_pool.clone()), &archive, BackupScope::all())
        .await
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    let target_pool = empty_catalog(&target.path().join("one-backend.db")).await;
    // A column the archive supplies but the live table now rejects, so the
    // insert fails partway through the merge rather than at the first table.
    target_pool
        .execute("ALTER TABLE providers ADD COLUMN tier TEXT NOT NULL DEFAULT 'x' CHECK (tier = 'only-this')")
        .await
        .unwrap();

    let result = service(target.path())
        .restore(&DbPool::Sqlite(target_pool.clone()), &archive, BackupScope::all())
        .await;
    assert!(result.is_err(), "the merge should have failed on the check constraint");

    let conversations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversations")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(
        conversations, 0,
        "a failed restore must roll back, not leave the target half-merged"
    );
}

/// `projects.id` is an auto-assigned INTEGER key: the number means nothing
/// outside the database that issued it. Carrying it across would overwrite
/// whichever unrelated local project happened to hold the same number.
#[tokio::test]
async fn a_surrogate_key_does_not_overwrite_an_unrelated_local_row() {
    let source = tempfile::tempdir().unwrap();
    let source_pool = seeded_catalog(&source.path().join("one-backend.db")).await;
    let archive = source.path().join("backup.zip");
    service(source.path())
        .export(&DbPool::Sqlite(source_pool.clone()), &archive, only_conversations())
        .await
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    let target_pool = seeded_catalog(&target.path().join("one-backend.db")).await;
    // Same id (1), different project. A naive `INSERT OR REPLACE` would
    // silently destroy this row.
    target_pool
        .execute("DELETE FROM projects; INSERT INTO projects (id, project_id, name) VALUES (1, 'proj-local', 'Local')")
        .await
        .unwrap();

    service(target.path())
        .restore(&DbPool::Sqlite(target_pool.clone()), &archive, BackupScope::all())
        .await
        .unwrap();

    let local: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE project_id = 'proj-local'")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(local, 1, "the local project was overwritten by the archive's row");
    let imported: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE project_id = 'proj-alpha'")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(imported, 1, "the archive's project was not imported");
}

/// Restoring the same archive twice must converge rather than fail or double
/// the rows.
#[tokio::test]
async fn restoring_the_same_archive_twice_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let pool = seeded_catalog(&dir.path().join("one-backend.db")).await;
    let archive = dir.path().join("backup.zip");
    service(dir.path())
        .export(&DbPool::Sqlite(pool.clone()), &archive, only_conversations())
        .await
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    let target_pool = seeded_catalog(&target.path().join("one-backend.db")).await;
    target_pool
        .execute(
            "DELETE FROM conversation_assistant_snapshots;
             DELETE FROM messages;
             DELETE FROM conversations;",
        )
        .await
        .unwrap();

    for _ in 0..2 {
        service(target.path())
            .restore(&DbPool::Sqlite(target_pool.clone()), &archive, BackupScope::all())
            .await
            .unwrap();
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversations")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let projects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE project_id = 'proj-alpha'")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(
        projects, 1,
        "a surrogate-keyed row was duplicated on the second restore"
    );
}

/// An archive from an older build is missing columns this one added. Selecting
/// `*` would fail on the first such difference, so the merge uses the shared
/// columns.
#[tokio::test]
async fn a_catalog_whose_columns_differ_still_merges_on_the_shared_ones() {
    let source = tempfile::tempdir().unwrap();
    let source_pool = seeded_catalog(&source.path().join("one-backend.db")).await;
    let archive = source.path().join("backup.zip");
    service(source.path())
        .export(&DbPool::Sqlite(source_pool.clone()), &archive, only_conversations())
        .await
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    let target_pool = seeded_catalog(&target.path().join("one-backend.db")).await;
    target_pool
        .execute(
            "DELETE FROM conversation_assistant_snapshots;
             DELETE FROM messages;
             DELETE FROM conversations;",
        )
        .await
        .unwrap();
    // A column the archive's catalog does not have.
    target_pool
        .execute("ALTER TABLE conversations ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0")
        .await
        .unwrap();

    service(target.path())
        .restore(&DbPool::Sqlite(target_pool.clone()), &archive, BackupScope::all())
        .await
        .unwrap();

    let name: String = sqlx::query_scalar("SELECT name FROM conversations WHERE id = 'c1'")
        .fetch_one(&target_pool)
        .await
        .unwrap();
    assert_eq!(name, "Quarterly plan");
}

#[tokio::test]
async fn files_are_restored_only_for_the_selected_categories() {
    let source = tempfile::tempdir().unwrap();
    let source_pool = seeded_catalog(&source.path().join("one-backend.db")).await;
    std::fs::create_dir_all(source.path().join("skills/my-skill")).unwrap();
    std::fs::write(source.path().join("skills/my-skill/SKILL.md"), b"# mine").unwrap();
    std::fs::create_dir_all(source.path().join("conversations/c1")).unwrap();
    std::fs::write(source.path().join("conversations/c1/out.txt"), b"generated").unwrap();

    let archive = source.path().join("backup.zip");
    service(source.path())
        .export(&DbPool::Sqlite(source_pool.clone()), &archive, BackupScope::all())
        .await
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    let target_pool = seeded_catalog(&target.path().join("one-backend.db")).await;
    service(target.path())
        .restore(
            &DbPool::Sqlite(target_pool.clone()),
            &archive,
            BackupScope {
                skills: true,
                ..empty_scope()
            },
        )
        .await
        .unwrap();

    assert!(target.path().join("skills/my-skill/SKILL.md").is_file());
    assert!(
        !target.path().join("conversations/c1/out.txt").exists(),
        "attachments were restored despite not being selected"
    );
}

#[tokio::test]
async fn an_export_with_nothing_selected_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let pool = seeded_catalog(&dir.path().join("one-backend.db")).await;

    let error = service(dir.path())
        .export(
            &DbPool::Sqlite(pool.clone()),
            &dir.path().join("empty.zip"),
            empty_scope(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SystemError::BadRequest(_)), "got {error:?}");
}

#[tokio::test]
async fn restoring_a_category_the_archive_lacks_is_refused_rather_than_silently_doing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let pool = seeded_catalog(&dir.path().join("one-backend.db")).await;
    let archive = dir.path().join("conversations-only.zip");
    service(dir.path())
        .export(&DbPool::Sqlite(pool.clone()), &archive, only_conversations())
        .await
        .unwrap();

    let error = service(dir.path())
        .restore(
            &DbPool::Sqlite(pool.clone()),
            &archive,
            BackupScope {
                providers: true,
                ..empty_scope()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SystemError::BadRequest(_)), "got {error:?}");
}

#[tokio::test]
async fn a_backup_from_a_newer_app_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let pool = seeded_catalog(&dir.path().join("one-backend.db")).await;
    let archive = dir.path().join("newer.zip");
    write_manifest_only(
        &archive,
        &BackupManifest {
            format_version: BACKUP_FORMAT_VERSION,
            exported_at: 0,
            app_version: "9.9.9".to_owned(),
            scope: BackupScope::all(),
            total_bytes: 0,
            contains_credentials: true,
        },
    );

    let error = service(dir.path())
        .restore(&DbPool::Sqlite(pool.clone()), &archive, BackupScope::all())
        .await
        .unwrap_err();
    assert!(matches!(error, SystemError::BadRequest(_)), "got {error:?}");
}

#[test]
fn an_unknown_format_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("future.zip");
    write_manifest_only(
        &archive,
        &BackupManifest {
            format_version: BACKUP_FORMAT_VERSION + 1,
            exported_at: 0,
            app_version: "3.0.0".to_owned(),
            scope: BackupScope::all(),
            total_bytes: 0,
            contains_credentials: false,
        },
    );

    assert!(matches!(
        service(dir.path()).preview(&archive).unwrap_err(),
        SystemError::BadRequest(_)
    ));
}

#[test]
fn a_file_that_is_not_a_backup_is_rejected_with_a_readable_message() {
    let dir = tempfile::tempdir().unwrap();
    let not_an_archive = dir.path().join("holiday.jpg");
    std::fs::write(&not_an_archive, b"\xff\xd8\xff\xe0 not a zip").unwrap();

    assert!(matches!(
        service(dir.path()).preview(&not_an_archive).unwrap_err(),
        SystemError::BadRequest(_)
    ));
}

/// Zip entries may name any path they like. Without `enclosed_name`, an entry
/// called `../../evil` would be written outside the staging directory.
#[test]
fn an_archive_with_a_traversal_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("evil.zip");
    {
        let file = std::fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        writer.start_file("../../escaped.txt", options).unwrap();
        std::io::Write::write_all(&mut writer, b"owned").unwrap();
        writer.finish().unwrap();
    }

    let staging = dir.path().join("staging");
    let error = unpack_archive(&archive, &staging).unwrap_err();
    assert!(matches!(error, SystemError::BadRequest(_)), "got {error:?}");
    assert!(!dir.path().parent().unwrap().join("escaped.txt").exists());
}

#[test]
fn version_comparison_only_blocks_genuinely_newer_backups() {
    assert!(is_newer_version("3.1.0", "3.0.5"));
    assert!(is_newer_version("v3.1.0", "3.0.5"));
    assert!(!is_newer_version("3.0.5", "3.0.5"));
    assert!(!is_newer_version("3.0.4", "3.0.5"));
    // Unparseable on either side must not block a user from their own data.
    assert!(!is_newer_version("nightly", "3.0.5"));
    assert!(!is_newer_version("3.1.0", "dev-build"));
}

#[test]
fn a_scope_covers_the_categories_it_declares_and_no_others() {
    let skills_only = BackupScope {
        skills: true,
        ..empty_scope()
    };
    let tables = skills_only.tables();
    assert!(tables.contains(&"skills"));
    assert!(tables.contains(&"users"), "identity must survive every filter");
    assert!(!tables.contains(&"providers"));
    assert!(!tables.contains(&"conversations"));
    assert_eq!(skills_only.dirs(), vec!["skills"]);
}

fn write_manifest_only(archive: &Path, manifest: &BackupManifest) {
    let file = std::fs::File::create(archive).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    writer.start_file(MANIFEST_NAME, options).unwrap();
    std::io::Write::write_all(&mut writer, &serde_json::to_vec(manifest).unwrap()).unwrap();
    writer.finish().unwrap();
}
