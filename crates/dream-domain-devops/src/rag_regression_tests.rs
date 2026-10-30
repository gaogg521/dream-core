// Included in service::tests so these exercise the actual service and its ACL fixtures.

#[tokio::test]
async fn document_listing_tracks_content_and_title_edits_without_changing_creation_time() {
    let svc = service().await;
    let doc = svc
        .register_rag_document("before", None, None, None, "org", None, "all", "admin1", None)
        .await
        .unwrap();
    svc.db
        .execute(
            "UPDATE one_rag_documents SET created_at = 1, updated_at = 1 WHERE id = ?",
            &db_params![&doc.id],
        )
        .await
        .unwrap();
    svc.set_document_content("admin1", &doc.id, "").await.unwrap();
    let edited = svc
        .list_rag_documents("admin1")
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == doc.id)
        .unwrap();
    assert_eq!(edited.created_at, 1);
    assert!(edited.updated_at > 1);
    svc.db
        .execute(
            "UPDATE one_rag_documents SET updated_at = 1 WHERE id = ?",
            &db_params![&doc.id],
        )
        .await
        .unwrap();
    svc.update_rag_document_location("admin1", &doc.id, "renamed", None)
        .await
        .unwrap();
    let renamed = svc
        .list_rag_documents("admin1")
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == doc.id)
        .unwrap();
    assert_eq!(renamed.title, "renamed");
    assert_eq!(renamed.created_at, 1);
    assert!(renamed.updated_at > 1);
}

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
    // Returning to identical text must not let the original revision commit.
    svc.set_document_content("admin1", &document.id, "oldconcurrenttoken")
        .await
        .unwrap();
    release.notify_one();
    assert_eq!(worker.await.unwrap().unwrap(), 0);
    assert_eq!(
        svc.read_rag_document_content("admin1", &document.id).await.unwrap(),
        "oldconcurrenttoken"
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

#[tokio::test]
async fn rag_remote_embedding_reuses_the_http_connection() {
    let peers = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new()));
    let captured = peers.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = crate::embedding::EmbeddingConfig {
        base_url: format!("http://{}", listener.local_addr().unwrap()),
        api_key: String::new(), model: "test".into(),
    };
    let server = tokio::spawn(async move {
        let app = axum::Router::new().route("/embeddings", axum::routing::post(
            move |axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>| {
                let captured = captured.clone();
                async move {
                    captured.lock().await.insert(peer);
                    axum::Json(serde_json::json!({"data":[{"embedding":[1.0,0.0]}]}))
                }
            }
        ));
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    for _ in 0..2 {
        let vectors = crate::embedding::embed(&config, &["connection reuse".into()]).await.unwrap();
        assert_eq!(vectors, vec![vec![1.0, 0.0]]);
    }
    assert_eq!(peers.lock().await.len(), 1, "two requests should share one pooled TCP connection");
    server.abort();
}

#[tokio::test]
async fn rag_slow_embedding_search_has_bounded_keyword_fallback() {
    for slow_body in [false, true] {
    let svc = service().await;
    let doc = svc.register_rag_document("slow", None, None, None, "org", None, "all", "admin1", None)
        .await.unwrap();
    rag_seed_searchable_chunk(&svc, &doc.id, "slowendpointtoken").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    svc.set_rag_config(&format!("http://{}", listener.local_addr().unwrap()), "slow", Some(""))
        .await.unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/embeddings", axum::routing::post(move || async move {
            if slow_body {
                return axum::response::Response::new(axum::body::Body::from_stream(
                    futures_util::stream::pending::<Result<axum::body::Bytes, std::io::Error>>()
                ));
            }
            std::future::pending::<()>().await;
            axum::response::Response::new(axum::body::Body::empty())
        }))).await.unwrap();
    });
    let hits = tokio::time::timeout(std::time::Duration::from_secs(4), svc.search_rag("admin1", "slowendpointtoken", 5))
        .await.expect("slow embedding must not block keyword retrieval").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document_id, doc.id);
    server.abort();
    }
}

#[tokio::test]
async fn rag_automatic_indexing_coalesces_saves_and_releases_backoff_capacity() {
    let svc = std::sync::Arc::new(service().await);
    let inputs = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let captured = inputs.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    svc.set_rag_config(&format!("http://{}", listener.local_addr().unwrap()), "test", Some(""))
        .await.unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/embeddings", axum::routing::post(
            move |axum::Json(body): axum::Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    let input = body["input"][0].as_str().unwrap().to_owned();
                    captured.lock().await.push(input.clone());
                    if input == "fail" {
                        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
                    }
                    Ok(axum::Json(serde_json::json!({"data":[{"embedding":[1.0,0.0]}]})))
                }
            }
        ))).await.unwrap();
    });
    let failed = svc.register_rag_document("failed", None, None, None, "org", None, "all", "admin1", None).await.unwrap();
    svc.set_document_content("admin1", &failed.id, "fail").await.unwrap();
    svc.schedule_indexing("admin1".into(), failed.id.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while inputs.lock().await.is_empty() { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
    }).await.unwrap();
    let successful = svc.register_rag_document("latest", None, None, None, "org", None, "all", "admin1", None).await.unwrap();
    for content in ["first", "second", "latest"] {
        svc.set_document_content("admin1", &successful.id, content).await.unwrap();
        svc.schedule_indexing("admin1".into(), successful.id.clone());
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let state = svc.list_rag_documents("admin1").await.unwrap();
            if state.iter().any(|row| row.id == successful.id && row.status == "ready") { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.expect("another document must finish before the failed document's 5-second retry");
    assert_eq!(*inputs.lock().await, vec!["fail", "latest"]);
    // Removing the failed document makes its next retry terminal.
    svc.delete_rag_document("admin1", &failed.id).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn rag_cold_embedding_finishes_in_the_background_and_the_next_search_is_semantic() {
    let svc = service().await;
    let doc = svc.register_rag_document("cold", None, None, None, "org", None, "all", "admin1", None)
        .await.unwrap();
    rag_seed_searchable_chunk(&svc, &doc.id, "coldstartdocument").await;
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    svc.set_rag_config(&format!("http://{}", listener.local_addr().unwrap()), "cold", Some(""))
        .await.unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/embeddings", axum::routing::post(move || {
            let counted = counted.clone();
            async move {
                // The first request is the cold one: the model has to load before it can answer.
                if counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(2600)).await;
                }
                axum::Json(serde_json::json!({"data":[{"embedding":[1.0, 0.0]}]}))
            }
        }))).await.unwrap();
    });
    // Natural language with no literal keyword in common with the stored text: only a vector can match it.
    let question = "which note explains how the boot sequence behaves when nothing has run yet";

    let started = std::time::Instant::now();
    let first = svc.search_rag("admin1", question, 5).await.unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(4), "the first search stays bounded");
    assert!(first.is_empty(), "no keyword match and the vector is not ready within the budget");

    // The cold request was not dropped: it finishes and is cached.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let again = std::time::Instant::now();
    let second = svc.search_rag("admin1", question, 5).await.unwrap();
    assert_eq!(second.len(), 1, "the same question is now answered semantically");
    assert_eq!(second[0].document_id, doc.id);
    assert!(again.elapsed() < std::time::Duration::from_millis(800), "served from the cache, not the endpoint");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "no second embedding request was made");
    server.abort();
}

#[tokio::test]
async fn rag_embedding_warmup_calls_the_endpoint_only_when_there_is_something_to_search() {
    let svc = service().await;
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    svc.set_rag_config(&format!("http://{}", listener.local_addr().unwrap()), "warm", Some(""))
        .await.unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/embeddings", axum::routing::post(move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                axum::Json(serde_json::json!({"data":[{"embedding":[1.0, 0.0]}]}))
            }
        }))).await.unwrap();
    });
    assert!(!svc.has_indexed_chunks().await, "an empty knowledge base has nothing to warm for");
    let doc = svc.register_rag_document("warm", None, None, None, "org", None, "all", "admin1", None)
        .await.unwrap();
    rag_seed_searchable_chunk(&svc, &doc.id, "warmuptoken").await;
    assert!(svc.has_indexed_chunks().await);
    svc.warm_embedding("test").await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn rag_single_document_visibility_matches_the_listing_for_every_viewer() {
    let svc = service().await;
    seed_two_group_enterprise(&svc).await;
    let mut ids = Vec::new();
    for (title, scope, team, visibility, author) in [
        ("org", "org", None, "all", "admin1"),
        ("groupA", "team", Some("tA"), "all", "admin1"),
        ("groupB", "team", Some("tB"), "all", "admin2"),
        ("secret", "org", None, "admin", "admin1"),
        ("mine", "personal", None, "all", "memberA"),
        ("otherPrivate", "personal", None, "all", "memberB"),
        ("adminPrivate", "personal", None, "all", "admin1"),
    ] {
        ids.push(
            svc.register_rag_document(title, None, None, None, scope, team, visibility, author, None)
                .await
                .unwrap()
                .id,
        );
    }
    for viewer in ["admin1", "admin2", "memberA", "memberB", "stranger"] {
        let listed: std::collections::HashSet<String> =
            svc.list_rag_documents(viewer).await.unwrap().into_iter().map(|d| d.id).collect();
        for id in &ids {
            assert_eq!(
                svc.rag_document_visible(viewer, id).await.unwrap(),
                listed.contains(id),
                "viewer {viewer} / document {id}"
            );
        }
    }
}

async fn rag_vector_server(dims: usize) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/embeddings", axum::routing::post(
            move |axum::Json(body): axum::Json<serde_json::Value>| async move {
                // A vector that encodes the text: texts mentioning "alpha" point one way, others another.
                let data: Vec<serde_json::Value> = body["input"].as_array().unwrap().iter().map(|text| {
                    let alpha = text.as_str().unwrap_or("").contains("alpha");
                    let mut v = vec![0.0f32; dims];
                    v[if alpha { 0 } else { 1 }] = 1.0;
                    serde_json::json!({ "embedding": v })
                }).collect();
                axum::Json(serde_json::json!({ "data": data }))
            }
        ))).await.unwrap();
    });
    (url, server)
}

#[tokio::test]
async fn rag_vector_index_follows_reprocessing_deletion_and_outside_writes() {
    let svc = service().await;
    let (url, server) = rag_vector_server(4).await;
    svc.set_rag_config(&url, "vec", Some("")).await.unwrap();
    let doc = svc.register_rag_document("d", None, None, None, "org", None, "all", "admin1", None)
        .await.unwrap();
    svc.set_document_content("admin1", &doc.id, "alpha topic body").await.unwrap();
    svc.process_rag_document("admin1", &doc.id).await.unwrap();

    let hits = svc.search_rag("admin1", "about alpha", 5).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].score > 0.99, "exact cosine is reported, not the quantized estimate");
    assert_eq!(svc.vectors.chunk_count().await, 1);

    // Re-processed with different content: the old vector must not linger.
    svc.set_document_content("admin1", &doc.id, "beta topic body").await.unwrap();
    svc.process_rag_document("admin1", &doc.id).await.unwrap();
    let hits = svc.search_rag("admin1", "about alpha", 5).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].score < 0.01, "the vector now points away from alpha");
    assert!(hits[0].content.contains("beta"));

    // A write the service did not make (backup restore, another process) is picked up through the change log.
    let other = svc.register_rag_document("o", None, None, None, "org", None, "all", "admin1", None)
        .await.unwrap();
    sqlx::query("INSERT INTO one_rag_chunks (id, document_id, chunk_index, content, embedding, created_at) VALUES ('outside', ?, 0, 'alpha from outside', ?, 0)")
        .bind(&other.id)
        .bind(crate::embedding::pack_embedding(&[1.0, 0.0, 0.0, 0.0]))
        .execute(svc.db.sqlite())
        .await
        .unwrap();
    let hits = svc.search_rag("admin1", "about alpha", 5).await.unwrap();
    assert_eq!(hits[0].document_id, other.id, "the outside write ranks first for alpha");
    assert_eq!(svc.vectors.chunk_count().await, 2);

    svc.delete_rag_document("admin1", &other.id).await.unwrap();
    let hits = svc.search_rag("admin1", "about alpha", 5).await.unwrap();
    assert!(hits.iter().all(|hit| hit.document_id != other.id));
    assert_eq!(svc.vectors.chunk_count().await, 1);
    server.abort();
}

#[tokio::test]
async fn rag_large_document_is_embedded_in_batches_the_endpoint_accepts() {
    let svc = service().await;
    let largest = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = largest.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    svc.set_rag_config(&format!("http://{}", listener.local_addr().unwrap()), "small-batches", Some(""))
        .await.unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/embeddings", axum::routing::post(
            move |axum::Json(body): axum::Json<serde_json::Value>| {
                let seen = seen.clone();
                async move {
                    let inputs = body["input"].as_array().unwrap().len();
                    // Like providers that cap a batch: anything over 10 inputs is refused.
                    if inputs > 10 {
                        return axum::response::IntoResponse::into_response((axum::http::StatusCode::BAD_REQUEST, "batch size exceeds 10"));
                    }
                    seen.fetch_max(inputs, std::sync::atomic::Ordering::SeqCst);
                    let data: Vec<serde_json::Value> =
                        (0..inputs).map(|_| serde_json::json!({ "embedding": [1.0, 0.0] })).collect();
                    axum::response::IntoResponse::into_response(axum::Json(serde_json::json!({ "data": data })))
                }
            }
        ))).await.unwrap();
    });
    let doc = svc.register_rag_document("big", None, None, None, "org", None, "all", "admin1", None)
        .await.unwrap();
    let text: String = (0..300).map(|i| format!("Section {i}. {}\n\n", "words ".repeat(120))).collect();
    svc.set_document_content("admin1", &doc.id, &text).await.unwrap();
    let chunks = svc.process_rag_document("admin1", &doc.id).await.unwrap();
    assert!(chunks > 100, "a large document produces many chunks ({chunks})");
    let stored: i64 = svc.db
        .fetch_one_scalar("SELECT COUNT(*) FROM one_rag_chunks WHERE document_id = ?", &db_params![&doc.id])
        .await.unwrap();
    assert_eq!(stored, chunks);
    assert!(largest.load(std::sync::atomic::Ordering::SeqCst) <= 10);
    server.abort();
}
