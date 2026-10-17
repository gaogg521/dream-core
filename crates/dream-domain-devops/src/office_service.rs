//! Intranet editing lifecycle. Documents remain protected by the library ACL.
use crate::{
    DevopsError, DevopsService,
    models::RagDocumentDto,
    office_session::{OfficeConfig, OfficeTicket},
};
use dream_core_api_types::knowledge_office::{KnowledgeOfficeSaved, KnowledgeOfficeSession};
use dream_core_db::db_params;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

const FILE_LIMIT: usize = 100 * 1024 * 1024;

/// Composition supplies live account/directory admission, independently of the
/// browser cookie. Engine callbacks never inherit an old authorization decision.
#[async_trait::async_trait]
pub trait OfficeAdmission: Send + Sync {
    async fn check(&self, actor: &str) -> Result<(), DevopsError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::atomic::{AtomicBool, Ordering},
    };

    struct Admission(AtomicBool);
    #[async_trait::async_trait]
    impl OfficeAdmission for Admission {
        async fn check(&self, _: &str) -> Result<(), DevopsError> {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(DevopsError::Forbidden("inactive member".into()))
            }
        }
    }
    fn original(text: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(zip, "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>").unwrap();
        zip.finish().unwrap().into_inner()
    }
    use std::io::Cursor;
    struct Fixture {
        service: Arc<OfficeService>,
        admission: Arc<Admission>,
        document: String,
        _directory: tempfile::TempDir,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    async fn fixture() -> Fixture {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member', created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0); CREATE TABLE one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL); CREATE TABLE one_runtime_nodes (id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL, machine_id TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'approved');").execute(&pool).await.unwrap();
        let db = dream_core_db::DbPool::Sqlite(pool);
        crate::run_one_devops_migrations(&db).await.unwrap();
        let documents = Arc::new(DevopsService::new(db));
        let directory = tempfile::tempdir().unwrap();
        let key = uuid::Uuid::new_v4().to_string();
        tokio::fs::write(directory.path().join(&key), original("original text"))
            .await
            .unwrap();
        let document = documents
            .register_rag_document(
                "test.docx",
                Some(&format!("rag-source:{key}")),
                None,
                Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
                "personal",
                None,
                "all",
                "owner",
                None,
            )
            .await
            .unwrap()
            .id;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let bytes = original("saved 中文 &amp; table text");
        let routes = axum::Router::new()
            .route(
                "/cache/files/saved.docx",
                axum::routing::get(move || {
                    let bytes = bytes.clone();
                    async move { bytes }
                }),
            )
            .route(
                "/coauthoring/CommandService.ashx",
                axum::routing::post(|| async { axum::Json(json!({"error":4})) }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        let config = OfficeConfig::new(&url, &url, "http://core/", b"0123456789abcdef0123456789abcdef").unwrap();
        let admission = Arc::new(Admission(AtomicBool::new(true)));
        let service =
            Arc::new(OfficeService::new(config, directory.path().into(), documents, admission.clone()).unwrap());
        Fixture {
            service,
            admission,
            document,
            _directory: directory,
            server,
        }
    }
    fn capability(session: &KnowledgeOfficeSession, field: &str) -> String {
        let url = if field == "source" {
            &session.config["document"]["url"]
        } else {
            &session.config["editorConfig"]["callbackUrl"]
        };
        reqwest::Url::parse(url.as_str().unwrap())
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "token")
            .unwrap()
            .1
            .into_owned()
    }
    async fn signed_save(service: &OfficeService, session: &KnowledgeOfficeSession) -> String {
        let mut lease = service.read_lease(&session.session_id).await.unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        lease.force_request = Some(request.clone());
        service.write_lease(&lease).await.unwrap();
        service
            .config
            .sign_editor_config(&json!({"key":session.session_id,"status":6,"userdata":request,
            "url":service.config.internal_url.join("cache/files/saved.docx").unwrap().to_string()}))
            .unwrap()
    }
    #[tokio::test]
    async fn callback_persists_original_and_text_and_survives_service_restart() {
        let fixture = fixture().await;
        let service = &fixture.service;
        let session = service.start("owner", "Owner", &fixture.document).await.unwrap();
        let editor_claims = service
            .config
            .verify_callback(session.config["token"].as_str().unwrap())
            .unwrap();
        let expiry = editor_claims["exp"].as_u64().unwrap();
        assert!(expiry > now() && expiry <= now() + 24 * 60 * 60);
        let source_token = capability(&session, "source");
        assert!(service.source(&source_token).await.unwrap().0.len() > 100);
        assert!(service.source(&capability(&session, "callback")).await.is_err());
        assert!(service.flush("intruder", &session.session_id).await.is_err());
        service
            .callback(&capability(&session, "callback"), &signed_save(service, &session).await)
            .await
            .unwrap();
        assert_eq!(
            service
                .documents
                .read_rag_document_content("owner", &fixture.document)
                .await
                .unwrap()
                .trim(),
            "saved 中文 & table text"
        );
        let restarted = OfficeService::new(
            service.config.clone(),
            service.source_root.clone(),
            service.documents.clone(),
            fixture.admission.clone(),
        )
        .unwrap();
        let result = restarted.flush("owner", &session.session_id).await.unwrap();
        assert!(result.saved);
        assert_eq!(result.revision, 1);
        let saved = tokio::fs::read(service.source_root.join(result.source_version))
            .await
            .unwrap();
        assert_eq!(
            crate::office_text::extract(&saved, "docx").unwrap().trim(),
            "saved 中文 & table text"
        );
    }
    #[tokio::test]
    async fn revoked_member_cannot_fetch_or_save_with_an_existing_capability() {
        let fixture = fixture().await;
        let session = fixture
            .service
            .start("owner", "Owner", &fixture.document)
            .await
            .unwrap();
        fixture.admission.0.store(false, Ordering::SeqCst);
        assert!(fixture.service.source(&capability(&session, "source")).await.is_err());
        assert!(
            fixture
                .service
                .callback(
                    &capability(&session, "callback"),
                    &signed_save(&fixture.service, &session).await
                )
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn stale_editor_cannot_overwrite_another_saved_original() {
        let fixture = fixture().await;
        let first = fixture
            .service
            .start("owner", "Owner", &fixture.document)
            .await
            .unwrap();
        let stale = fixture
            .service
            .start("owner", "Owner", &fixture.document)
            .await
            .unwrap();
        fixture
            .service
            .callback(
                &capability(&first, "callback"),
                &signed_save(&fixture.service, &first).await,
            )
            .await
            .unwrap();
        assert!(
            fixture
                .service
                .callback(
                    &capability(&stale, "callback"),
                    &signed_save(&fixture.service, &stale).await
                )
                .await
                .is_err()
        );
        assert!(fixture.service.source(&capability(&stale, "source")).await.is_err());
    }
    #[tokio::test]
    async fn repeated_and_delayed_callbacks_cannot_rewrite_a_newer_save() {
        let fixture = fixture().await;
        let session = fixture
            .service
            .start("owner", "Owner", &fixture.document)
            .await
            .unwrap();
        let token = capability(&session, "callback");
        let first = signed_save(&fixture.service, &session).await;
        fixture.service.callback(&token, &first).await.unwrap();
        fixture.service.callback(&token, &first).await.unwrap();
        assert_eq!(
            fixture.service.read_lease(&session.session_id).await.unwrap().revision,
            1
        );
        let second = signed_save(&fixture.service, &session).await;
        fixture.service.callback(&token, &second).await.unwrap();
        assert!(fixture.service.callback(&token, &first).await.is_err());
        assert_eq!(
            fixture.service.read_lease(&session.session_id).await.unwrap().revision,
            2
        );
    }
    #[tokio::test]
    async fn missing_callback_is_not_reported_saved_when_the_engine_has_no_new_changes() {
        let fixture = fixture().await;
        let session = fixture
            .service
            .start("owner", "Owner", &fixture.document)
            .await
            .unwrap();
        assert!(fixture.service.flush("owner", &session.session_id).await.unwrap().saved);
        let _pending = signed_save(&fixture.service, &session).await;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(500),
                fixture.service.flush("owner", &session.session_id)
            )
            .await
            .is_err()
        );
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Lease {
    id: String,
    actor: String,
    document: String,
    initial_source: String,
    current_source: String,
    pending_source: Option<String>,
    #[serde(default)]
    force_request: Option<String>,
    #[serde(default)]
    last_request: Option<String>,
    extension: String,
    expires: u64,
    revision: u64,
    closed: bool,
}

pub struct OfficeService {
    config: OfficeConfig,
    source_root: PathBuf,
    documents: Arc<DevopsService>,
    admission: Arc<dyn OfficeAdmission>,
    mutations: Mutex<()>,
    http: reqwest::Client,
}

fn now() -> u64 {
    (dream_core_common::now_ms() / 1000) as u64
}
fn source_key(document: &RagDocumentDto) -> Result<String, DevopsError> {
    let key = document
        .file_path
        .as_deref()
        .unwrap_or("")
        .strip_prefix("rag-source:")
        .ok_or_else(|| DevopsError::BadRequest("Office editing requires a retained original".into()))?;
    uuid::Uuid::parse_str(key).map_err(|_| DevopsError::BadRequest("invalid original identity".into()))?;
    Ok(key.to_owned())
}
fn extension(document: &RagDocumentDto) -> Result<&'static str, DevopsError> {
    match document.mime_type.as_deref() {
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document") => Ok("docx"),
        Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet") => Ok("xlsx"),
        Some("application/vnd.openxmlformats-officedocument.presentationml.presentation") => Ok("pptx"),
        _ => Err(DevopsError::BadRequest(
            "intranet editing currently accepts DOCX, XLSX and PPTX originals".into(),
        )),
    }
}

impl OfficeService {
    pub fn new(
        config: OfficeConfig,
        source_root: PathBuf,
        documents: Arc<DevopsService>,
        admission: Arc<dyn OfficeAdmission>,
    ) -> Result<Self, DevopsError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(40))
            .build()
            .map_err(|_| DevopsError::Internal("Office service client unavailable".into()))?;
        Ok(Self {
            config,
            source_root,
            documents,
            admission,
            mutations: Mutex::new(()),
            http,
        })
    }

    async fn document(&self, actor: &str, id: &str, writable: bool) -> Result<RagDocumentDto, DevopsError> {
        self.admission.check(actor).await?;
        if writable {
            self.documents.assert_rag_writable(actor, id).await?;
        }
        self.documents
            .list_rag_documents(actor)
            .await?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| DevopsError::Forbidden("document is not visible to this member".into()))
    }

    fn lease_path(&self, id: &str) -> Result<PathBuf, DevopsError> {
        uuid::Uuid::parse_str(id).map_err(|_| DevopsError::Forbidden("invalid Office session".into()))?;
        Ok(self.source_root.join("office-sessions").join(format!("{id}.json")))
    }

    async fn read_lease(&self, id: &str) -> Result<Lease, DevopsError> {
        let path = self.lease_path(id)?;
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|_| DevopsError::NotFound("Office session unavailable".into()))?;
        if metadata.len() > 8192 {
            return Err(DevopsError::Forbidden("invalid Office session".into()));
        }
        let lease: Lease = serde_json::from_slice(
            &tokio::fs::read(path)
                .await
                .map_err(|_| DevopsError::Internal("Office session storage unavailable".into()))?,
        )
        .map_err(|_| DevopsError::Internal("Office session storage invalid".into()))?;
        if lease.id != id || lease.expires <= now() || lease.closed {
            return Err(DevopsError::Forbidden("Office session expired or closed".into()));
        }
        Ok(lease)
    }

    async fn write_lease(&self, lease: &Lease) -> Result<(), DevopsError> {
        let path = self.lease_path(&lease.id)?;
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .map_err(|_| DevopsError::Internal("Office session storage unavailable".into()))?;
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::write(
            &temporary,
            serde_json::to_vec(lease)
                .map_err(|_| DevopsError::Internal("Office session serialization failed".into()))?,
        )
        .await
        .map_err(|_| DevopsError::Internal("Office session storage unavailable".into()))?;
        tokio::fs::rename(temporary, path)
            .await
            .map_err(|_| DevopsError::Internal("Office session storage unavailable".into()))
    }

    /// Recover a crash between the database commit and the lease-file update.
    fn reconcile(lease: &mut Lease, source: &str) -> Result<(), DevopsError> {
        if lease.pending_source.as_deref() == Some(source) {
            lease.current_source = source.to_owned();
            lease.pending_source = None;
            lease.revision += 1;
            lease.last_request = lease.force_request.take();
        }
        if lease.current_source != source {
            return Err(DevopsError::BadRequest(
                "original changed in another session; reopen the current file before editing".into(),
            ));
        }
        Ok(())
    }

    async fn authorized_lease(&self, id: &str, actor: &str) -> Result<Lease, DevopsError> {
        let mut lease = self.read_lease(id).await?;
        if lease.actor != actor {
            return Err(DevopsError::Forbidden(
                "Office session belongs to another member".into(),
            ));
        }
        let document = self.document(actor, &lease.document, true).await?;
        Self::reconcile(&mut lease, &source_key(&document)?)?;
        Ok(lease)
    }

    pub async fn start(
        &self,
        actor: &str,
        display_name: &str,
        id: &str,
    ) -> Result<KnowledgeOfficeSession, DevopsError> {
        let document = self.document(actor, id, true).await?;
        let source = source_key(&document)?;
        let extension = extension(&document)?.to_owned();
        let lease = Lease {
            id: uuid::Uuid::new_v4().to_string(),
            actor: actor.into(),
            document: id.into(),
            initial_source: source.clone(),
            current_source: source,
            pending_source: None,
            force_request: None,
            last_request: None,
            extension,
            expires: now() + 24 * 60 * 60,
            revision: 0,
            closed: false,
        };
        self.write_lease(&lease).await?;
        let capability = |purpose: &str| {
            self.config.ticket(&OfficeTicket {
                iss: "onework-knowledge-office".into(),
                aud: purpose.into(),
                exp: lease.expires,
                actor: actor.into(),
                document: id.into(),
                session: lease.id.clone(),
                source: lease.initial_source.clone(),
            })
        };
        let endpoint = |name: &str, token: String| -> Result<String, DevopsError> {
            let mut url = self
                .config
                .callback_base
                .join(&format!("api/one/devops/office-engine/{name}"))
                .map_err(|_| DevopsError::Internal("Office callback URL invalid".into()))?;
            url.query_pairs_mut().append_pair("token", &token);
            Ok(url.to_string())
        };
        let kind = match lease.extension.as_str() {
            "xlsx" => "cell",
            "pptx" => "slide",
            _ => "word",
        };
        let title = if document
            .title
            .to_ascii_lowercase()
            .ends_with(&format!(".{}", lease.extension))
        {
            document.title
        } else {
            format!("{}.{}", document.title, lease.extension)
        };
        let mut config = json!({"documentType":kind, "type":"desktop", "width":"100%", "height":"100%",
            "document":{"fileType":lease.extension, "key":lease.id, "title":title,
                "url":endpoint("source", capability("source")?)?, "permissions":{"edit":true,"download":true,"print":true}},
            "editorConfig":{"mode":"edit","lang":"zh-CN","callbackUrl":endpoint("callback",capability("callback")?)?,
                "user":{"id":actor,"name":display_name},"customization":{"autosave":true,"forcesave":false}}});
        let mut claims = config.clone();
        claims["exp"] = json!(lease.expires);
        config["token"] = Value::String(self.config.sign_editor_config(&claims)?);
        let script_url = self
            .config
            .public_url
            .join("web-apps/apps/api/documents/api.js")
            .map_err(|_| DevopsError::Internal("Office editor URL invalid".into()))?
            .to_string();
        tracing::info!(document_id = id, session_id = %lease.id, "knowledge intranet Office session opened");
        Ok(KnowledgeOfficeSession {
            session_id: lease.id,
            script_url,
            config,
        })
    }

    async fn capability(&self, token: &str, purpose: &str) -> Result<Lease, DevopsError> {
        let claims = self.config.verify_ticket(token, purpose)?;
        let lease = self.authorized_lease(&claims.session, &claims.actor).await?;
        if claims.document != lease.document || claims.source != lease.initial_source {
            return Err(DevopsError::Forbidden(
                "Office capability does not match its session".into(),
            ));
        }
        Ok(lease)
    }

    pub async fn source(&self, token: &str) -> Result<(Vec<u8>, String), DevopsError> {
        let lease = self.capability(token, "source").await?;
        let path = self.source_root.join(&lease.initial_source);
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|_| DevopsError::NotFound("original unavailable".into()))?;
        if metadata.len() > FILE_LIMIT as u64 {
            return Err(DevopsError::BadRequest("original exceeds 100 MiB".into()));
        }
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|_| DevopsError::NotFound("original unavailable".into()))?;
        Ok((bytes, lease.extension))
    }

    pub async fn callback(self: &Arc<Self>, token: &str, engine_token: &str) -> Result<(), DevopsError> {
        let signed = self.config.verify_callback(engine_token)?;
        let lease = self.capability(token, "callback").await?;
        if signed.get("key").and_then(Value::as_str) != Some(&lease.id) {
            return Err(DevopsError::Forbidden("Office callback key mismatch".into()));
        }
        let status = signed
            .get("status")
            .and_then(Value::as_u64)
            .ok_or_else(|| DevopsError::BadRequest("Office callback status missing".into()))?;
        if ![2, 6].contains(&status) {
            if [3, 7].contains(&status) {
                return Err(DevopsError::BadRequest(
                    "Office engine could not save the original".into(),
                ));
            }
            return Ok(());
        }
        let request = signed.get("userdata").and_then(Value::as_str);
        if status == 6 {
            if request.is_some() && request == lease.last_request.as_deref() {
                return Ok(());
            }
            if request.is_none() || request != lease.force_request.as_deref() {
                return Err(DevopsError::Forbidden(
                    "Office callback does not match the pending save".into(),
                ));
            }
        } else if lease.force_request.is_some() {
            return Err(DevopsError::BadRequest("Office force-save is still pending".into()));
        }
        let url = self.config.callback_download_url(
            signed
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| DevopsError::BadRequest("Office callback download missing".into()))?,
        )?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|_| DevopsError::Internal("Office saved file unavailable".into()))?;
        if !response.status().is_success() || response.content_length().is_some_and(|size| size > FILE_LIMIT as u64) {
            return Err(DevopsError::BadRequest("Office saved file response rejected".into()));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| DevopsError::Internal("Office saved file download failed".into()))?;
            if bytes.len() + chunk.len() > FILE_LIMIT {
                return Err(DevopsError::BadRequest("Office saved file exceeds 100 MiB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(DevopsError::BadRequest("Office saved file is empty".into()));
        }
        let extension = lease.extension.clone();
        let extract_bytes = bytes.clone();
        let extracted = tokio::task::spawn_blocking(move || crate::office_text::extract(&extract_bytes, &extension))
            .await
            .map_err(|_| DevopsError::Internal("Office text extraction unavailable".into()))?;
        let (text, extraction_error) = match extracted {
            Ok(text) => (text, None),
            Err(_) => (String::new(), Some("original saved; retrieval text extraction failed")),
        };
        let _guard = self.mutations.lock().await;
        // Recheck after download: a removed member or replaced original must not be written.
        let mut lease = self.capability(token, "callback").await?;
        if status == 6 && request != lease.force_request.as_deref() {
            if request == lease.last_request.as_deref() {
                return Ok(());
            }
            return Err(DevopsError::Forbidden("Office save was superseded".into()));
        }
        let key = uuid::Uuid::new_v4().to_string();
        tokio::fs::write(self.source_root.join(&key), &bytes)
            .await
            .map_err(|_| DevopsError::Internal("original storage unavailable".into()))?;
        lease.pending_source = Some(key.clone());
        self.write_lease(&lease).await?;
        let mut transaction = self.documents.db.begin().await?;
        let updated = transaction.execute("UPDATE one_rag_documents SET file_path = ?, file_size = ?, content = ?, status = ?, chunk_count = 0, last_error = ? WHERE id = ? AND file_path = ?",
            &db_params![format!("rag-source:{key}"), bytes.len() as i64, &text, if text.trim().is_empty() {"ready"} else {"pending"}, extraction_error, &lease.document, format!("rag-source:{}",lease.current_source)]).await?;
        if updated != 1 {
            return Err(DevopsError::BadRequest(
                "original changed while saving; reopen the current file".into(),
            ));
        }
        transaction
            .execute(
                "DELETE FROM one_rag_chunks WHERE document_id = ?",
                &db_params![&lease.document],
            )
            .await?;
        transaction.commit().await?;
        lease.current_source = key;
        lease.pending_source = None;
        lease.revision += 1;
        lease.last_request = lease.force_request.take();
        lease.closed = status == 2;
        self.write_lease(&lease).await?;
        if let Err(error) = crate::retrieval::sync_document(&self.documents.db, &lease.document, &[]).await {
            tracing::warn!(document_id=%lease.document, %error, "could not clear Office lexical index");
        }
        tracing::info!(document_id=%lease.document, session_id=%lease.id, revision=lease.revision, bytes=bytes.len(), "knowledge intranet Office original saved");
        if !text.trim().is_empty() {
            let service = self.clone();
            tokio::spawn(async move {
                if service.admission.check(&lease.actor).await.is_ok()
                    && let Err(error) = service
                        .documents
                        .process_rag_document(&lease.actor, &lease.document)
                        .await
                {
                    tracing::warn!(document_id=%lease.document, %error, "Office original saved but retrieval indexing failed");
                }
            });
        }
        Ok(())
    }

    pub async fn flush(&self, actor: &str, id: &str) -> Result<KnowledgeOfficeSaved, DevopsError> {
        let (initial, had_pending) = {
            let _guard = self.mutations.lock().await;
            let mut lease = self.authorized_lease(id, actor).await?;
            let had_pending = lease.force_request.is_some();
            if !had_pending {
                lease.force_request = Some(uuid::Uuid::new_v4().to_string());
                self.write_lease(&lease).await?;
            }
            (lease, had_pending)
        };
        let mut command = json!({"c":"forcesave","key":id,"userdata":initial.force_request});
        // Default Docs installations authenticate commands in the header;
        // installations opting into inBody consume the direct body signature.
        let header_token = self.config.sign_editor_config(&json!({"payload":command}))?;
        command["token"] = Value::String(self.config.sign_editor_config(&command)?);
        let endpoint = self
            .config
            .internal_url
            .join("coauthoring/CommandService.ashx")
            .map_err(|_| DevopsError::Internal("Office command URL invalid".into()))?;
        let response: Value = self
            .http
            .post(endpoint)
            .bearer_auth(header_token)
            .json(&command)
            .send()
            .await
            .map_err(|_| DevopsError::Internal("Office save request failed".into()))?
            .error_for_status()
            .map_err(|_| DevopsError::Internal("Office save request rejected".into()))?
            .json()
            .await
            .map_err(|_| DevopsError::Internal("Office save response invalid".into()))?;
        match response.get("error").and_then(Value::as_u64) {
            Some(4) if !had_pending => {
                let _guard = self.mutations.lock().await;
                let mut current = self.authorized_lease(id, actor).await?;
                if current.force_request == initial.force_request {
                    current.force_request = None;
                    self.write_lease(&current).await?;
                }
                return Ok(KnowledgeOfficeSaved {
                    saved: true,
                    revision: current.revision,
                    source_version: current.current_source,
                });
            }
            Some(0 | 4) => {}
            _ => {
                return Err(DevopsError::BadRequest(
                    "Office editor could not flush its changes".into(),
                ));
            }
        }
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let current = self.authorized_lease(id, actor).await?;
            if current.revision > initial.revision {
                return Ok(KnowledgeOfficeSaved {
                    saved: true,
                    revision: current.revision,
                    source_version: current.current_source,
                });
            }
        }
        Err(DevopsError::Internal(
            "Office save timed out; keep this editor open and retry".into(),
        ))
    }
}
