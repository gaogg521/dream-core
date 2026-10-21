// Included in service::tests so these exercise the actual service and its ACL fixtures.

#[tokio::test]
async fn rag_versioned_save_rejects_stale_drafts_and_clears_empty_pending() {
    let svc = service().await;
    let doc = svc
        .register_rag_document("versioned", None, None, None, "org", None, "all", "admin1", None)
        .await
        .unwrap();
    let a = svc.read_rag_content_state("admin1", &doc.id).await.unwrap();
    let b = svc.read_rag_content_state("admin1", &doc.id).await.unwrap();
    let revision = svc
        .set_document_content_if_revision("admin1", &doc.id, "A must survive", a.revision)
        .await
        .unwrap();
    assert!(revision > a.revision);
    assert!(matches!(
        svc.set_document_content_if_revision("admin1", &doc.id, "B stale draft", b.revision)
            .await,
        Err(DevopsError::ContentConflict)
    ));
    let state = svc.read_rag_content_state("admin1", &doc.id).await.unwrap();
    assert_eq!(state.content, "A must survive");
    assert_eq!(state.revision, revision);
    svc.set_document_content_if_revision("admin1", &doc.id, "", revision)
        .await
        .unwrap();
    let row = svc
        .list_rag_documents("admin1")
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == doc.id)
        .unwrap();
    assert_eq!(row.status, "ready");
    assert_eq!(row.chunk_count, 0);
}

async fn rag_failed_embedding_endpoint(svc: &DevopsService) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    svc.set_rag_config(&url, "test-embedding", Some("")).await.unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/embeddings",
                axum::routing::post(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
            ),
        )
        .await
        .unwrap();
    })
}

async fn rag_seed_searchable_chunk(svc: &DevopsService, document: &str, text: &str) {
    let chunk = new_id("regression_chunk");
    svc.db
        .execute(
            "UPDATE one_rag_documents SET content = ?, status = 'ready', chunk_count = 1 WHERE id = ?",
            &db_params![text, document],
        )
        .await
        .unwrap();
    svc.db.execute(
        "INSERT INTO one_rag_chunks (id, document_id, chunk_index, content, embedding, created_at) VALUES (?, ?, 0, ?, ?, 0)",
        &db_params![&chunk, document, text, crate::embedding::pack_embedding(&[1.0, 0.0])],
    ).await.unwrap();
    crate::retrieval::sync_document(&svc.db, document, &[(chunk, text.to_owned())])
        .await
        .unwrap();
}

#[tokio::test]
async fn rag_edit_invalidates_old_dense_and_lexical_chunks_even_when_empty_or_embedding_fails() {
    let svc = service().await;
    let server = rag_failed_embedding_endpoint(&svc).await;
    let document = svc
        .register_rag_document("editable", None, None, None, "org", None, "all", "admin1", None)
        .await
        .unwrap();
    for replacement in ["", "replacement revision"] {
        rag_seed_searchable_chunk(&svc, &document.id, "obsoleteuniquetoken").await;
        assert_eq!(
            svc.search_rag("admin1", "obsoleteuniquetoken", 5).await.unwrap().len(),
            1
        );
        svc.set_document_content("admin1", &document.id, replacement)
            .await
            .unwrap();
        assert!(
            svc.search_rag("admin1", "obsoleteuniquetoken", 5)
                .await
                .unwrap()
                .is_empty()
        );
        let chunks: i64 = svc
            .db
            .fetch_one_scalar(
                "SELECT COUNT(*) FROM one_rag_chunks WHERE document_id = ?",
                &db_params![&document.id],
            )
            .await
            .unwrap();
        assert_eq!(chunks, 0);
        let lexical: i64 = svc
            .db
            .fetch_one_scalar(
                "SELECT COUNT(*) FROM one_rag_chunks_fts WHERE content MATCH '\"obsoleteuniquetoken\"'",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(
            lexical, 0,
            "old text must also be removed from the lexical index on disk"
        );
        assert!(svc.process_rag_document("admin1", &document.id).await.is_err());
        assert!(
            svc.search_rag("admin1", "obsoleteuniquetoken", 5)
                .await
                .unwrap()
                .is_empty()
        );
    }
    server.abort();
}

#[tokio::test]
async fn rag_embedding_outage_falls_back_to_keywords_with_member_and_personal_acl() {
    let svc = service().await;
    seed_two_group_enterprise(&svc).await;
    let server = rag_failed_embedding_endpoint(&svc).await;
    for (title, scope, team, visibility, author) in [
        ("org", "org", None, "all", "admin1"),
        ("groupA", "team", Some("tA"), "all", "admin1"),
        ("groupB", "team", Some("tB"), "all", "admin2"),
        ("secret", "org", None, "admin", "admin1"),
        ("mine", "personal", None, "all", "memberA"),
        ("otherPrivate", "personal", None, "all", "memberB"),
    ] {
        let doc = svc
            .register_rag_document(title, None, None, None, scope, team, visibility, author, None)
            .await
            .unwrap();
        rag_seed_searchable_chunk(&svc, &doc.id, "outageuniquetoken").await;
    }
    let hits = svc.search_rag("memberA", "outageuniquetoken", 20).await.unwrap();
    let mut titles: Vec<_> = hits.iter().map(|hit| hit.document_title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, ["groupA", "mine", "org"]);
    assert!(
        hits.iter().all(|hit| hit.score >= 0.35),
        "keyword fallback must survive employee dispatch's relevance cutoff"
    );
    assert_eq!(
        svc.search_rag("memberA", "outageuniquetoken", 1).await.unwrap().len(),
        1
    );
    assert!(
        svc.search_rag("memberA", "missinguniquetoken", 5)
            .await
            .unwrap()
            .is_empty()
    );
    let admin = svc.search_rag("admin1", "outageuniquetoken", 20).await.unwrap();
    assert_eq!(
        admin.len(),
        4,
        "administrators still cannot retrieve another member's personal documents"
    );
    server.abort();
}

#[tokio::test]
async fn rag_delayed_embedding_never_resurrects_an_edited_revision() {
    let svc = std::sync::Arc::new(service().await);
    let document = svc
        .register_rag_document("concurrent", None, None, None, "org", None, "all", "admin1", None)
        .await
        .unwrap();
    svc.set_document_content("admin1", &document.id, "oldconcurrenttoken")
        .await
        .unwrap();
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    svc.set_rag_config(
        &format!("http://{}", listener.local_addr().unwrap()),
        "test-embedding",
        Some(""),
    )
    .await
    .unwrap();
    let entered_handler = entered.clone();
    let release_handler = release.clone();
    let server = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/embeddings",
            axum::routing::post(move || {
                let entered = entered_handler.clone();
                let release = release_handler.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    axum::Json(serde_json::json!({"data":[{"embedding":[1.0,0.0]}]}))
                }
            }),
        );
        axum::serve(listener, app).await.unwrap();
    });
    let worker_service = svc.clone();
    let id = document.id.clone();
    let worker = tokio::spawn(async move { worker_service.process_rag_document("admin1", &id).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    svc.set_document_content("admin1", &document.id, "newconcurrenttoken")
        .await
        .unwrap();
    release.notify_one();
    assert_eq!(worker.await.unwrap().unwrap(), 0);
    assert_eq!(
        svc.read_rag_document_content("admin1", &document.id).await.unwrap(),
        "newconcurrenttoken"
    );
    let chunks: i64 = svc
        .db
        .fetch_one_scalar(
            "SELECT COUNT(*) FROM one_rag_chunks WHERE document_id = ?",
            &db_params![&document.id],
        )
        .await
        .unwrap();
    assert_eq!(
        chunks, 0,
        "an obsolete embedding response cannot restore invalidated chunks"
    );
    server.abort();
}
