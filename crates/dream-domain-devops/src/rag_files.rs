//! Original library files are stored separately from extracted retrieval text.
use crate::{error::DevopsError, models::RagDocumentDto, state::OneDevopsRouterState};
use axum::{
    Extension, Json,
    body::Body,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use dream_core_api_types::ApiResponse;
use dream_core_auth::CurrentUser;
use dream_core_db::db_params;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const LIMIT: usize = 100 * 1024 * 1024;
const PREFIX: &str = "rag-source:";

fn mime(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn storage_key(value: &str) -> Result<&str, DevopsError> {
    let key = value
        .strip_prefix(PREFIX)
        .ok_or_else(|| DevopsError::NotFound("original file is not retained".into()))?;
    uuid::Uuid::parse_str(key).map_err(|_| DevopsError::BadRequest("invalid original file identity".into()))?;
    Ok(key)
}

pub(crate) async fn upload(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<RagDocumentDto>>, DevopsError> {
    state.service.assert_rag_writable(&user.id, &id).await?;
    let root = state
        .rag_source_dir
        .as_ref()
        .ok_or_else(|| DevopsError::Internal("original file storage is not configured".into()))?;
    let mut file = multipart
        .next_field()
        .await
        .map_err(|e| DevopsError::BadRequest(e.to_string()))?
        .ok_or_else(|| DevopsError::BadRequest("file is required".into()))?;
    if file.name() != Some("file") {
        return Err(DevopsError::BadRequest("expected file field".into()));
    }
    let name = file
        .file_name()
        .unwrap_or("document")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("document")
        .to_owned();
    let mut data = Vec::new();
    while let Some(chunk) = file.chunk().await.map_err(|e| DevopsError::BadRequest(e.to_string()))? {
        if data.len() + chunk.len() > LIMIT {
            return Err(DevopsError::BadRequest("file exceeds 100 MiB".into()));
        }
        data.extend_from_slice(&chunk);
    }
    if data.is_empty() {
        return Err(DevopsError::BadRequest("file is empty".into()));
    }
    let key = uuid::Uuid::new_v4().to_string();
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|e| DevopsError::Internal(e.to_string()))?;
    tokio::fs::write(root.join(&key), &data)
        .await
        .map_err(|e| DevopsError::Internal(e.to_string()))?;
    // Keep old versions on disk; replacing a source never destroys its prior bytes.
    let mut transaction = state.service.db.begin().await?;
    transaction
        .execute(
            "UPDATE one_rag_documents SET file_path = ?, file_size = ?, mime_type = ?, \
             content = '', content_revision = content_revision + 1, status = 'ready', chunk_count = 0, last_error = NULL WHERE id = ?",
            &db_params![format!("{PREFIX}{key}"), data.len() as i64, mime(&name), &id],
        )
        .await?;
    transaction
        .execute("DELETE FROM one_rag_chunks WHERE document_id = ?", &db_params![&id])
        .await?;
    transaction.commit().await?;
    // A new original must never keep matches from the previous file's text.
    if let Err(error) = crate::retrieval::sync_document(&state.service.db, &id, &[]).await {
        tracing::warn!(%error, document_id = %id, "could not clear original file lexical index");
    }
    let doc = state
        .service
        .rag_document(&id)
        .await?
        .ok_or_else(|| DevopsError::NotFound("document not found".into()))?;
    tracing::info!(document_id = %id, bytes = data.len(), "knowledge original file retained");
    Ok(Json(ApiResponse::ok(doc)))
}

pub(crate) async fn download(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, DevopsError> {
    if !state.service.rag_document_visible(&user.id, &id).await? {
        return Err(DevopsError::Forbidden("document is not visible to this member".into()));
    }
    let doc = state
        .service
        .rag_document(&id)
        .await?
        .ok_or_else(|| DevopsError::Forbidden("document is not visible to this member".into()))?;
    let key = storage_key(doc.file_path.as_deref().unwrap_or(""))?;
    let root = state
        .rag_source_dir
        .as_ref()
        .ok_or_else(|| DevopsError::Internal("original file storage is not configured".into()))?;
    let mut file = tokio::fs::File::open(root.join(key))
        .await
        .map_err(|_| DevopsError::NotFound("original file is unavailable".into()))?;
    let size = file
        .metadata()
        .await
        .map_err(|e| DevopsError::Internal(e.to_string()))?
        .len();
    if size == 0 {
        return Err(DevopsError::NotFound("original file is empty".into()));
    }
    let range = headers.get(header::RANGE).and_then(|h| h.to_str().ok());
    let (start, end) = match range {
        None => (0, size.saturating_sub(1)),
        Some(value) => match parse_range(value, size) {
            Some(r) => r,
            None => {
                return Ok((
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [(header::CONTENT_RANGE, format!("bytes */{size}"))],
                )
                    .into_response());
            }
        },
    };
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|e| DevopsError::Internal(e.to_string()))?;
    let length = end - start + 1;
    let stream = tokio_util::io::ReaderStream::new(file.take(length));
    let mut response = Response::new(Body::from_stream(stream));
    if range.is_some() {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{size}").parse().unwrap(),
        );
    }
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        doc.mime_type
            .unwrap_or_else(|| "application/octet-stream".into())
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    h.insert(header::CONTENT_LENGTH, length.into());
    h.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    h.insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("content-security-policy", "sandbox".parse().unwrap());
    h.insert(header::CONTENT_DISPOSITION, "attachment".parse().unwrap());
    Ok(response)
}

fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        return (suffix > 0).then_some((size.saturating_sub(suffix), size - 1));
    }
    let start: u64 = start.parse().ok()?;
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>().ok()?.min(size - 1)
    };
    (start < size && start <= end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_identity_cannot_escape_storage() {
        assert!(storage_key("rag-source:../secret").is_err());
        assert!(storage_key("C:/secret").is_err());
    }
    #[test]
    fn media_ranges_are_bounded() {
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 100), None);
    }
    #[tokio::test]
    async fn original_file_roundtrip_ranges_and_private_acl() {
        use axum::http::Request;
        use std::sync::Arc;
        use tower::ServiceExt;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member', created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0); CREATE TABLE one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL); CREATE TABLE one_runtime_nodes (id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL, machine_id TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'approved');").execute(&pool).await.unwrap();
        let db = dream_core_db::DbPool::Sqlite(pool);
        crate::migrate::run_one_devops_migrations(&db).await.unwrap();
        let service = Arc::new(crate::service::DevopsService::new(db));
        let doc = service
            .register_rag_document(
                "private.pdf",
                None,
                None,
                Some("application/pdf"),
                "personal",
                None,
                "all",
                "owner",
                None,
            )
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        service
            .db
            .execute(
                "UPDATE one_rag_documents SET content = 'previous original text', status = 'ready', chunk_count = 1 WHERE id = ?",
                &db_params![&doc.id],
            )
            .await
            .unwrap();
        service.db.execute(
            "INSERT INTO one_rag_chunks (id, document_id, chunk_index, content, embedding, created_at) VALUES (?, ?, 0, ?, ?, 0)",
            &db_params!["previous-chunk", &doc.id, "previous original text", crate::embedding::pack_embedding(&[1.0])],
        ).await.unwrap();
        crate::retrieval::sync_document(
            &service.db,
            &doc.id,
            &[("previous-chunk".into(), "previous original text".into())],
        )
        .await
        .unwrap();
        let app = crate::routes::one_devops_routes(
            OneDevopsRouterState::new(service.clone()).with_rag_source_dir(dir.path().to_path_buf()),
        );
        let uri = format!("/api/one/devops/rag/documents/{}/source", doc.id);
        let body = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"private.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-original\r\n--boundary--\r\n";
        let request = |method: &str, actor: &str, data: Body, range: Option<&str>| {
            let mut builder = Request::builder()
                .method(method)
                .uri(&uri)
                .header("content-type", "multipart/form-data; boundary=boundary");
            if let Some(value) = range {
                builder = builder.header("range", value);
            }
            let mut req = builder.body(data).unwrap();
            let mut user = CurrentUser::local_default();
            user.id = actor.into();
            req.extensions_mut().insert(user);
            req
        };
        assert_eq!(
            app.clone()
                .oneshot(request("POST", "intruder", Body::from(body), None))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let uploaded = app
            .clone()
            .oneshot(request("POST", "owner", Body::from(body), None))
            .await
            .unwrap();
        let status = uploaded.status();
        let details = axum::body::to_bytes(uploaded.into_body(), 16384).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&details));
        assert_eq!(service.read_rag_document_content("owner", &doc.id).await.unwrap(), "");
        let remaining: i64 = service
            .db
            .fetch_optional_scalar(
                "SELECT COUNT(*) FROM one_rag_chunks WHERE document_id = ?",
                &db_params![&doc.id],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remaining, 0, "replacing an original must remove stale retrieval chunks");
        assert_eq!(
            app.clone()
                .oneshot(request("GET", "intruder", Body::empty(), None))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let response = app
            .clone()
            .oneshot(request("GET", "owner", Body::empty(), None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/pdf");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            b"%PDF-original"
        );
        let response = app
            .clone()
            .oneshot(request("GET", "owner", Body::empty(), Some("bytes=1-3")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            b"PDF"
        );
        assert_eq!(
            app.oneshot(request("GET", "owner", Body::empty(), Some("bytes=999-")))
                .await
                .unwrap()
                .status(),
            StatusCode::RANGE_NOT_SATISFIABLE
        );
    }
}
