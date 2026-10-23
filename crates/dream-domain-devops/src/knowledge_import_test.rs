use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::run_one_devops_migrations;

/// An in-memory file storage that counts nothing and breaks on request.
#[derive(Clone, Default)]
struct FakeStorage {
    objects: Arc<Mutex<BTreeMap<String, (Vec<u8>, i64)>>>,
    fail_under: Arc<Mutex<Option<String>>>,
    unreadable: Arc<AtomicBool>,
}

impl FakeStorage {
    fn put(&self, key: &str, bytes: &[u8], modified: i64) {
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_owned(), (bytes.to_vec(), modified));
    }
    fn remove(&self, key: &str) {
        self.objects.lock().unwrap().remove(key);
    }
}

#[async_trait]
impl ExternalObjectSource for FakeStorage {
    async fn storage_exists(&self, _actor: &str, storage_id: &str) -> Result<bool, String> {
        Ok(storage_id == "store1")
    }

    async fn list(
        &self,
        _actor: &str,
        _storage_id: &str,
        sub_prefix: &str,
        _token: Option<&str>,
    ) -> Result<ExternalListing, String> {
        if let Some(bad) = self.fail_under.lock().unwrap().as_deref()
            && sub_prefix.starts_with(bad)
        {
            return Err("storage went away".into());
        }
        let mut folders = std::collections::BTreeSet::new();
        let mut entries = Vec::new();
        for (key, (bytes, modified)) in self.objects.lock().unwrap().iter() {
            let Some(rest) = key.strip_prefix(sub_prefix) else {
                continue;
            };
            match rest.split_once('/') {
                Some((folder, _)) => {
                    folders.insert(format!("{sub_prefix}{folder}/"));
                }
                None => entries.push(ExternalEntry {
                    key: key.clone(),
                    rel: key.clone(),
                    is_prefix: false,
                    size_bytes: Some(bytes.len() as i64),
                    last_modified: Some(*modified),
                    etag: None,
                }),
            }
        }
        for folder in folders {
            entries.push(ExternalEntry {
                key: folder.clone(),
                rel: folder,
                is_prefix: true,
                size_bytes: None,
                last_modified: None,
                etag: None,
            });
        }
        Ok(ExternalListing {
            entries,
            next_token: None,
        })
    }

    async fn read(&self, _actor: &str, _storage_id: &str, key: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        if self.unreadable.load(Ordering::SeqCst) {
            return Err("AccessDenied".into());
        }
        let objects = self.objects.lock().unwrap();
        let (bytes, _) = objects.get(key).ok_or("NoSuchKey")?;
        if bytes.len() as u64 > max_bytes {
            return Err("too large".into());
        }
        Ok(bytes.clone())
    }
}

/// A service with the real schema, an embedding endpoint that returns a fixed
/// vector for every input, and a scratch directory for retained originals.
async fn service() -> (Arc<DevopsService>, FakeStorage, tempfile::TempDir) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    service_over(dream_core_db::DbPool::Sqlite(pool)).await
}

/// Same fixture over any backend, so the lifecycle can also run on MySQL.
async fn service_over(db: dream_core_db::DbPool) -> (Arc<DevopsService>, FakeStorage, tempfile::TempDir) {
    run_one_devops_migrations(&db).await.unwrap();
    let svc = Arc::new(DevopsService::new(db));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/embeddings",
            axum::routing::post(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                let count = body["input"].as_array().map(|items| items.len()).unwrap_or(1);
                axum::Json(serde_json::json!({
                    "data": (0..count).map(|_| serde_json::json!({"embedding": [1.0, 0.0]})).collect::<Vec<_>>()
                }))
            }),
        );
        axum::serve(listener, app).await.unwrap();
    });
    svc.set_rag_config(&url, "test", Some("")).await.unwrap();

    let storage = FakeStorage::default();
    svc.set_external_object_source(Arc::new(storage.clone()));
    let dir = tempfile::tempdir().unwrap();
    svc.set_rag_source_root(dir.path().to_path_buf());
    (svc, storage, dir)
}

fn docx(text: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(
        format!(
            r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

fn input(library: &str) -> SaveKnowledgeImportSource {
    SaveKnowledgeImportSource {
        name: "NAS handbook".into(),
        storage_config_id: "store1".into(),
        prefix: String::new(),
        library_id: library.into(),
        scope: None,
        team_id: None,
        visibility: None,
        extensions: Vec::new(),
        schedule: None,
        enabled: None,
    }
}

async fn make_source(svc: &Arc<DevopsService>) -> KnowledgeImportSource {
    let library = svc.upsert_rag_library(None, "Shared", "", "admin1").await.unwrap();
    svc.save_import_source("admin1", None, input(&library.id))
        .await
        .unwrap()
}

/// Run the source and wait for its report.
async fn sync(svc: &Arc<DevopsService>, id: &str) -> KnowledgeImportReport {
    let before = svc
        .list_import_sources("admin1")
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == id)
        .and_then(|s| s.last_run_at);
    svc.start_import_run("admin1", id).await.unwrap();
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let current = svc
            .list_import_sources("admin1")
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        if !current.running && current.last_run_at.is_some() && current.last_run_at != before {
            return current.last_report.unwrap();
        }
    }
    panic!("the import did not finish");
}

async fn docs(svc: &DevopsService) -> Vec<crate::models::RagDocumentDto> {
    svc.list_rag_documents("admin1").await.unwrap()
}

/// The text of every stored chunk. Asserting on these rather than on search
/// results keeps the checks honest: the embedding stub gives every text the
/// same vector, so a dense search would "find" anything.
async fn chunk_texts(svc: &DevopsService) -> Vec<String> {
    svc.db
        .fetch_all_scalar::<String>("SELECT content FROM one_rag_chunks", &[])
        .await
        .unwrap()
}

async fn wait_for_chunk(svc: &DevopsService, token: &str) {
    for _ in 0..100 {
        if chunk_texts(svc).await.iter().any(|chunk| chunk.contains(token)) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("{token:?} was never indexed");
}

#[tokio::test]
async fn a_first_run_imports_every_supported_kind_and_makes_it_searchable() {
    let (svc, storage, dir) = service().await;
    storage.put("handbook/intro.md", b"# Intro\nzebratoken lives here", 1);
    storage.put("handbook/notes.txt", b"plain notes about quokkatoken", 1);
    storage.put(
        "handbook/page.html",
        b"<html><style>p{}</style><body><p>giraffetoken &amp; friends</p></body></html>",
        1,
    );
    storage.put("handbook/plan.docx", &docx("ocelottoken in a word file"), 1);
    let source = make_source(&svc).await;

    let report = sync(&svc, &source.id).await;

    assert_eq!(report.status, "ok", "{report:?}");
    assert_eq!(report.added, 4);
    assert_eq!(report.updated + report.unchanged + report.failed_total, 0);
    let all = docs(&svc).await;
    assert_eq!(all.len(), 4);
    assert!(all.iter().all(|d| d.scope == "org" && d.created_by == "admin1"));
    // The original is retained, as for an upload.
    assert!(
        all.iter()
            .all(|d| d.file_path.as_deref().is_some_and(|p| p.starts_with("rag-source:")))
    );
    assert!(std::fs::read_dir(dir.path()).unwrap().count() >= 4);
    for token in ["zebratoken", "quokkatoken", "giraffetoken", "ocelottoken"] {
        wait_for_chunk(&svc, token).await;
    }
}

#[tokio::test]
async fn a_second_run_only_touches_what_changed() {
    let (svc, storage, _dir) = service().await;
    storage.put("a.md", b"first version with oldtoken", 1);
    storage.put("b.md", b"stays the same", 1);
    let source = make_source(&svc).await;
    sync(&svc, &source.id).await;
    wait_for_chunk(&svc, "oldtoken").await;

    let untouched = sync(&svc, &source.id).await;
    assert_eq!((untouched.added, untouched.updated, untouched.unchanged), (0, 0, 2));

    storage.put("a.md", b"second version with newtoken", 2);
    let changed = sync(&svc, &source.id).await;
    assert_eq!((changed.added, changed.updated, changed.unchanged), (0, 1, 1));
    assert_eq!(docs(&svc).await.len(), 2, "an update must not create a second document");
    wait_for_chunk(&svc, "newtoken").await;
    let chunks = chunk_texts(&svc).await;
    assert!(
        !chunks.iter().any(|chunk| chunk.contains("oldtoken")),
        "the previous version must not stay indexed: {chunks:?}"
    );
}

#[tokio::test]
async fn a_file_removed_from_storage_is_flagged_not_deleted_and_can_return() {
    let (svc, storage, _dir) = service().await;
    storage.put("keep.md", b"keep me", 1);
    storage.put("gone.md", b"goneword is here", 1);
    let source = make_source(&svc).await;
    sync(&svc, &source.id).await;

    storage.remove("gone.md");
    let report = sync(&svc, &source.id).await;
    assert_eq!(report.missing, 1);
    assert_eq!(docs(&svc).await.len(), 2, "nothing is deleted");
    let state: Option<String> = svc
        .db
        .fetch_optional_scalar(
            "SELECT source_state FROM one_rag_documents WHERE source_key = 'gone.md'",
            &[],
        )
        .await
        .unwrap()
        .flatten();
    assert_eq!(state.as_deref(), Some("missing"));
    // The library listing carries it, so the console can flag the document.
    assert!(
        docs(&svc)
            .await
            .iter()
            .any(|d| d.source_key.as_deref() == Some("gone.md") && d.source_state.as_deref() == Some("missing"))
    );
    wait_for_chunk(&svc, "goneword").await;

    storage.put("gone.md", b"goneword is here", 1);
    let back = sync(&svc, &source.id).await;
    assert_eq!((back.missing, back.unchanged), (0, 2));
}

#[tokio::test]
async fn files_that_cannot_be_imported_are_listed_with_why() {
    let (svc, storage, _dir) = service().await;
    storage.put("ok.md", b"fine", 1);
    storage.put("manual.pdf", b"%PDF-1.4", 1);
    storage.put("photo.png", b"png", 1);
    storage.put("~$lock.docx", b"lock", 1);
    storage.put(".hidden.md", b"hidden", 1);
    let source = make_source(&svc).await;

    let report = sync(&svc, &source.id).await;

    assert_eq!(report.added, 1);
    assert_eq!(report.skipped_total, 2);
    let pdf = report.skipped.iter().find(|i| i.key == "manual.pdf").unwrap();
    assert!(pdf.reason.contains("PDF"), "{}", pdf.reason);
    assert!(report.skipped.iter().any(|i| i.key == "photo.png"));
    assert!(
        !report
            .skipped
            .iter()
            .any(|i| i.key.contains("lock") || i.key.contains("hidden"))
    );
}

#[tokio::test]
async fn a_broken_file_is_reported_and_does_not_stop_the_others() {
    let (svc, storage, _dir) = service().await;
    storage.put("good.md", b"good content", 1);
    storage.put("bad.docx", b"this is not a zip archive", 1);
    let source = make_source(&svc).await;

    let report = sync(&svc, &source.id).await;

    assert_eq!(report.status, "partial");
    assert_eq!((report.added, report.failed_total), (1, 1));
    assert_eq!(report.failed[0].key, "bad.docx");
    assert_eq!(docs(&svc).await.len(), 1, "a failed file leaves no half-made document");
}

#[tokio::test]
async fn a_failed_listing_never_flags_unvisited_files_as_missing() {
    let (svc, storage, _dir) = service().await;
    storage.put("top.md", b"top", 1);
    storage.put("sub/inner.md", b"inner", 1);
    let source = make_source(&svc).await;
    sync(&svc, &source.id).await;

    *storage.fail_under.lock().unwrap() = Some("sub/".into());
    let report = sync(&svc, &source.id).await;

    assert_eq!(report.status, "partial");
    assert_eq!(report.missing, 0, "an unreadable folder is not a deleted folder");
    assert!(report.failed.iter().any(|i| i.key == "sub/"));
}

#[tokio::test]
async fn only_administrators_manage_sources() {
    let (svc, _storage, _dir) = service().await;
    sqlx::raw_sql(
        "CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member', created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));
         CREATE TABLE one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, updated_at INTEGER NOT NULL DEFAULT 0);
         INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('member1', 't1', 'member');
         INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('admin1', 't1', 'org_admin');",
    )
    .execute(svc.db.sqlite())
    .await
    .unwrap();
    let source = make_source(&svc).await;

    assert!(matches!(
        svc.list_import_sources("member1").await,
        Err(DevopsError::Forbidden(_))
    ));
    assert!(matches!(
        svc.save_import_source("member1", None, input(&source.library_id)).await,
        Err(DevopsError::Forbidden(_))
    ));
    assert!(matches!(
        svc.start_import_run("member1", &source.id).await,
        Err(DevopsError::Forbidden(_))
    ));
    assert!(matches!(
        svc.delete_import_source("member1", &source.id).await,
        Err(DevopsError::Forbidden(_))
    ));
}

#[tokio::test]
async fn a_source_whose_owner_lost_the_role_pauses_instead_of_importing() {
    let (svc, storage, _dir) = service().await;
    sqlx::raw_sql(
        "CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member', created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));
         CREATE TABLE one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, updated_at INTEGER NOT NULL DEFAULT 0);
         INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('admin1', 't1', 'org_admin');
         INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('admin2', 't1', 'org_admin');",
    )
    .execute(svc.db.sqlite())
    .await
    .unwrap();
    storage.put("a.md", b"content", 1);
    let source = make_source(&svc).await;
    sqlx::query("UPDATE one_user_org SET role = 'member' WHERE user_id = 'admin1'")
        .execute(svc.db.sqlite())
        .await
        .unwrap();

    // admin2 triggers it; the source still belongs to admin1.
    svc.start_import_run("admin2", &source.id).await.unwrap();
    let mut report = None;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let current = svc.list_import_sources("admin2").await.unwrap().remove(0);
        if !current.running && current.last_report.is_some() {
            report = current.last_report;
            break;
        }
    }
    let report = report.expect("a report");
    assert_eq!(report.status, "paused");
    assert_eq!(
        svc.list_rag_documents("admin2").await.unwrap().len(),
        0,
        "a paused source imports nothing"
    );
}

#[tokio::test]
async fn deleting_a_source_keeps_the_knowledge_it_brought() {
    let (svc, storage, _dir) = service().await;
    storage.put("a.md", b"keepword", 1);
    let source = make_source(&svc).await;
    sync(&svc, &source.id).await;

    svc.delete_import_source("admin1", &source.id).await.unwrap();

    assert_eq!(docs(&svc).await.len(), 1);
    assert!(svc.list_import_sources("admin1").await.unwrap().is_empty());
    let source_id: Option<String> = svc
        .db
        .fetch_optional_scalar("SELECT source_id FROM one_rag_documents", &[])
        .await
        .unwrap()
        .flatten();
    assert_eq!(source_id, None, "it becomes an ordinary document");
}

#[tokio::test]
async fn a_second_run_cannot_start_while_one_is_running() {
    let (svc, storage, _dir) = service().await;
    storage.put("a.md", b"x", 1);
    let source = make_source(&svc).await;
    sqlx::query("UPDATE one_rag_import_sources SET running_since = ? WHERE id = ?")
        .bind(now_ms() as i64)
        .bind(&source.id)
        .execute(svc.db.sqlite())
        .await
        .unwrap();

    assert!(matches!(
        svc.start_import_run("admin1", &source.id).await,
        Err(DevopsError::BadRequest(message)) if message.contains("already running")
    ));
}

#[tokio::test]
async fn saving_a_source_validates_its_inputs() {
    let (svc, _storage, _dir) = service().await;
    let library = svc.upsert_rag_library(None, "Shared", "", "admin1").await.unwrap();
    let with = |change: fn(&mut SaveKnowledgeImportSource)| {
        let mut body = input(&library.id);
        change(&mut body);
        body
    };
    for (label, body) in [
        ("blank name", with(|b| b.name = "  ".into())),
        ("unknown storage", with(|b| b.storage_config_id = "nope".into())),
        ("traversal", with(|b| b.prefix = "a/../b".into())),
        ("pdf type", with(|b| b.extensions = vec!["pdf".into()])),
        ("bad schedule", with(|b| b.schedule = Some("weekly".into()))),
        ("bad scope", with(|b| b.scope = Some("personal".into()))),
    ] {
        assert!(
            matches!(
                svc.save_import_source("admin1", None, body).await,
                Err(DevopsError::BadRequest(_))
            ),
            "{label} must be refused"
        );
    }
    let saved = svc
        .save_import_source(
            "admin1",
            None,
            with(|b| {
                b.prefix = "/handbook//".into();
                b.extensions = vec![".MD".into(), "docx".into(), "md".into()];
                b.schedule = Some("daily".into());
            }),
        )
        .await
        .unwrap();
    assert_eq!(saved.prefix, "handbook/");
    assert_eq!(saved.extensions, vec!["docx", "md"]);
    assert_eq!(saved.schedule, "daily");
}

#[tokio::test]
async fn the_scheduler_starts_only_sources_that_are_due() {
    let (svc, storage, _dir) = service().await;
    storage.put("a.md", b"scheduledword", 1);
    let library = svc.upsert_rag_library(None, "Shared", "", "admin1").await.unwrap();
    let mut body = input(&library.id);
    body.schedule = Some("hourly".into());
    let due = svc.save_import_source("admin1", None, body).await.unwrap();
    let mut body = input(&library.id);
    body.name = "manual one".into();
    let manual = svc.save_import_source("admin1", None, body).await.unwrap();

    svc.run_due_imports().await;
    // The report is written when the run ends, which is after the document
    // appears; wait for that, not for the document.
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let sources = svc.list_import_sources("admin1").await.unwrap();
        if sources.iter().any(|s| s.id == due.id && s.last_run_at.is_some()) {
            break;
        }
    }

    let sources = svc.list_import_sources("admin1").await.unwrap();
    let find = |id: &str| sources.iter().find(|s| s.id == id).unwrap();
    assert!(find(&due.id).last_run_at.is_some());
    assert!(
        find(&manual.id).last_run_at.is_none(),
        "manual sources never run by themselves"
    );
}

/// The whole lifecycle again on a real MySQL (set `DREAM_TEST_MYSQL_URL`): the
/// migration, the claim/stale-run SQL, the unique source index and the upserts
/// are dialect-sensitive, and the SQLite run cannot see a MySQL-only failure.
#[tokio::test]
async fn the_import_lifecycle_also_works_on_mysql() {
    let Some(mysql) = dream_core_db::testing::mysql_test_pool().await else {
        eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
        return;
    };
    let (svc, storage, _dir) = service_over(mysql.pool.clone()).await;
    storage.put("keep.md", b"keepmysqlword", 1);
    storage.put("gone.md", b"gonemysqlword", 1);
    storage.put("sub/nested.docx", &docx("nestedmysqlword"), 1);
    storage.put("skip.pdf", b"%PDF-1.4", 1);
    let source = make_source(&svc).await;

    let first = sync(&svc, &source.id).await;
    assert_eq!(
        (first.added, first.skipped_total, first.failed_total),
        (3, 1, 0),
        "{first:?}"
    );
    wait_for_chunk(&svc, "nestedmysqlword").await;
    assert!(
        docs(&svc)
            .await
            .iter()
            .all(|d| d.source_key.is_some() && d.source_state.as_deref() == Some("ok")),
        "the listing carries the source columns on MySQL"
    );

    let again = sync(&svc, &source.id).await;
    assert_eq!((again.added, again.updated, again.unchanged), (0, 0, 3), "{again:?}");

    storage.put("keep.md", b"changedmysqlword", 2);
    storage.remove("gone.md");
    let changed = sync(&svc, &source.id).await;
    assert_eq!(
        (changed.updated, changed.unchanged, changed.missing),
        (1, 1, 1),
        "{changed:?}"
    );
    wait_for_chunk(&svc, "changedmysqlword").await;
    assert!(!chunk_texts(&svc).await.iter().any(|c| c.contains("keepmysqlword")));
    assert_eq!(docs(&svc).await.len(), 3, "nothing was deleted");

    // A second claim while one is held is refused (the atomic UPDATE).
    sqlx::query("UPDATE one_rag_import_sources SET running_since = ? WHERE id = ?")
        .bind(now_ms() as i64)
        .bind(&source.id)
        .execute(match &svc.db {
            dream_core_db::DbPool::MySql(pool) => pool,
            _ => unreachable!("this test runs on MySQL"),
        })
        .await
        .unwrap();
    assert!(matches!(
        svc.start_import_run("admin1", &source.id).await,
        Err(DevopsError::BadRequest(message)) if message.contains("already running")
    ));

    svc.delete_import_source("admin1", &source.id).await.unwrap();
    assert_eq!(docs(&svc).await.len(), 3);
    mysql.cleanup().await.unwrap();
}

#[test]
fn html_is_reduced_to_its_visible_words() {
    let text = html_to_text(
        "<html><head><script>var secret = 1;</script><style>p{}</style></head><body><h1>Title</h1><p>a&nbsp;&amp;&nbsp;b</p></body></html>",
    );
    assert_eq!(text, "Title a & b");
}

#[test]
fn prefixes_take_one_shape_and_refuse_traversal() {
    assert_eq!(normalize_sub_prefix("").unwrap(), "");
    assert_eq!(normalize_sub_prefix("/a//").unwrap(), "a/");
    assert!(normalize_sub_prefix("a/../b").is_err());
    assert!(normalize_sub_prefix("./a").is_err());
}

#[test]
fn a_changed_marker_changes_the_signature() {
    let entry = |etag: Option<&str>, size: i64, modified: i64| ExternalEntry {
        key: "k".into(),
        rel: "k".into(),
        is_prefix: false,
        size_bytes: Some(size),
        last_modified: Some(modified),
        etag: etag.map(str::to_owned),
    };
    assert_eq!(signature(&entry(Some("a"), 1, 1)), signature(&entry(Some("a"), 1, 1)));
    assert_ne!(signature(&entry(Some("a"), 1, 1)), signature(&entry(Some("b"), 1, 1)));
    // No ETag (WebDAV): size and time still notice a rewrite.
    assert_ne!(signature(&entry(None, 1, 1)), signature(&entry(None, 2, 1)));
    assert_ne!(signature(&entry(None, 1, 1)), signature(&entry(None, 1, 2)));
}
