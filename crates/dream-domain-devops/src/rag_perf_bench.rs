// Included in service::tests. Knowledge-base performance benchmark on a realistic file-backed SQLite library.
// Ignored by default; run with:
//   cargo nextest run -p dream-domain-devops --run-ignored ignored-only rag_perf --no-capture
// Scale is set by RAG_BENCH_DOCS / RAG_BENCH_CHUNKS / RAG_BENCH_DIMS (defaults: 1500 x 30 x 384).

fn bench_env(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Deterministic pseudo-random unit vector, so runs are comparable.
fn bench_vector(seed: u64, dims: usize) -> Vec<f32> {
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut v: Vec<f32> = (0..dims)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        })
        .collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::EPSILON);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

async fn bench_service(dir: &std::path::Path) -> DevopsService {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(dir.join("bench.db"))
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    run_one_devops_migrations(&dream_core_db::DbPool::Sqlite(pool.clone()))
        .await
        .unwrap();
    DevopsService::new(dream_core_db::DbPool::Sqlite(pool))
}

async fn bench_embedding_server(dims: usize) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/embeddings",
                axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    let inputs = body["input"].as_array().map(Vec::len).unwrap_or(0);
                    let data: Vec<serde_json::Value> = (0..inputs)
                        .map(|i| serde_json::json!({ "embedding": bench_vector(i as u64 + 7, dims) }))
                        .collect();
                    axum::Json(serde_json::json!({ "data": data }))
                }),
            ),
        )
        .await
        .unwrap();
    });
    (url, server)
}

async fn timed<T, F: std::future::Future<Output = T>>(label: &str, runs: usize, mut make: impl FnMut() -> F) -> T {
    let mut best = std::time::Duration::MAX;
    let mut total = std::time::Duration::ZERO;
    let mut last = None;
    for _ in 0..runs {
        let started = std::time::Instant::now();
        let value = make().await;
        let elapsed = started.elapsed();
        best = best.min(elapsed);
        total += elapsed;
        last = Some(value);
    }
    println!(
        "RAG_BENCH {label:<44} best {:>8.1} ms   avg {:>8.1} ms",
        best.as_secs_f64() * 1000.0,
        total.as_secs_f64() * 1000.0 / runs as f64
    );
    last.unwrap()
}

#[tokio::test]
#[ignore = "benchmark; run explicitly"]
async fn rag_perf_bench() {
    let docs = bench_env("RAG_BENCH_DOCS", 1500);
    let chunks_per_doc = bench_env("RAG_BENCH_CHUNKS", 30);
    let dims = bench_env("RAG_BENCH_DIMS", 384);
    let dir = tempfile::tempdir().unwrap();
    let svc = bench_service(dir.path()).await;
    seed_two_group_enterprise(&svc).await;
    let (url, server) = bench_embedding_server(dims).await;
    svc.set_rag_config(&url, "bench", Some("")).await.unwrap();

    // Seed: documents through the service (realistic rows), chunks in one transaction.
    let seeded = std::time::Instant::now();
    let words = ["报销", "流程", "部署", "runbook", "审批", "季度", "预算", "接口", "故障", "复盘", "客户", "合同"];
    let mut ids = Vec::with_capacity(docs);
    for d in 0..docs {
        let (scope, team, author) = match d % 4 {
            0 => ("org", None, "admin1"),
            1 => ("team", Some("tA"), "admin1"),
            2 => ("team", Some("tB"), "admin2"),
            _ => ("personal", None, "memberA"),
        };
        let doc = svc
            .register_rag_document(&format!("文档 {d}"), None, None, None, scope, team, "all", author, None)
            .await
            .unwrap();
        ids.push(doc.id);
    }
    {
        let pool = svc.db.sqlite().clone();
        let mut tx = pool.begin().await.unwrap();
        for (d, id) in ids.iter().enumerate() {
            let mut body = String::new();
            for c in 0..chunks_per_doc {
                let text = format!(
                    "第{d}篇第{c}段：{} {} {} {} 编号 ERR_{d}_{c} 说明文字。",
                    words[(d + c) % words.len()],
                    words[(d * 3 + c) % words.len()],
                    words[(d + c * 5) % words.len()],
                    "这是一段用于基准测试的较长正文，模拟真实知识库里的段落内容。".repeat(12)
                );
                body.push_str(&text);
                body.push_str("\n\n");
                sqlx::query(
                    "INSERT INTO one_rag_chunks (id, document_id, chunk_index, content, embedding, created_at) VALUES (?, ?, ?, ?, ?, 0)",
                )
                .bind(format!("{id}_{c}"))
                .bind(id)
                .bind(c as i64)
                .bind(&text)
                .bind(crate::embedding::pack_embedding(&bench_vector((d * chunks_per_doc + c) as u64, dims)))
                .execute(&mut *tx)
                .await
                .unwrap();
            }
            sqlx::query("UPDATE one_rag_documents SET content = ?, status = 'ready', chunk_count = ? WHERE id = ?")
                .bind(&body)
                .bind(chunks_per_doc as i64)
                .bind(id)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
    }
    crate::retrieval::rebuild_index(&svc.db).await.unwrap();
    println!(
        "RAG_BENCH seeded {docs} docs x {chunks_per_doc} chunks ({} chunks, {dims} dims) in {:.1}s",
        docs * chunks_per_doc,
        seeded.elapsed().as_secs_f64()
    );

    let probe = ids[0].clone(); // org-scoped: admin1 may re-index it
    let question = "季度预算审批流程是怎样的";
    timed("search_rag first call (index load)", 1, || svc.search_rag("admin1", "首次检索", 8)).await.unwrap();
    timed("search_rag admin (semantic question)", 5, || svc.search_rag("admin1", question, 8)).await.unwrap();
    timed("search_rag member (semantic question)", 5, || svc.search_rag("memberA", question, 8)).await.unwrap();
    timed("search_rag admin (exact code)", 5, || svc.search_rag("admin1", "ERR_700_3", 8)).await.unwrap();
    let docs_listed = timed("list_rag_documents admin", 5, || svc.list_rag_documents("admin1")).await.unwrap();
    assert!(docs_listed.len() > docs / 2);
    timed("list_rag_documents member", 5, || svc.list_rag_documents("memberA")).await.unwrap();
    timed("filter_rag_documents admin '复盘'", 5, || svc.filter_rag_documents("admin1", "复盘")).await.unwrap();
    timed("filter_rag_documents member 'ERR_9'", 5, || svc.filter_rag_documents("memberA", "ERR_9")).await.unwrap();
    timed("read_rag_content_state member", 5, || svc.read_rag_content_state("admin1", &probe)).await.unwrap();

    // Indexing one large document end to end (chunking, embedding round trip, chunk + lexical writes).
    let big = svc
        .register_rag_document("大文档", None, None, None, "org", None, "all", "admin1", None)
        .await
        .unwrap();
    let paragraph = "这是一段用于测量建索引耗时的长段落，包含若干关键字：报销、审批、部署、故障复盘。".repeat(20);
    let text: String = (0..1500).map(|i| format!("第{i}节 {paragraph}\n\n")).collect();
    svc.set_document_content("admin1", &big.id, &text).await.unwrap();
    // After a write, the next search replays the change log for just that document.
    svc.set_document_content("admin1", &probe, "报销 审批 预算 修改后的正文").await.unwrap();
    svc.process_rag_document("admin1", &probe).await.unwrap();
    timed("search_rag right after re-indexing one doc", 1, || svc.search_rag("admin1", question, 8)).await.unwrap();

    match timed("process_rag_document (large doc)", 1, || svc.process_rag_document("admin1", &big.id)).await {
        Ok(chunks) => println!("RAG_BENCH   large doc: {} chars -> {chunks} chunks", text.chars().count()),
        Err(error) => println!("RAG_BENCH   large doc FAILED: {error}"),
    }
    server.abort();
}

/// Local (bundled ONNX) model: indexing throughput, and how long a search's query waits while indexing runs.
/// Needs ORT_DYLIB_PATH pointing at onnxruntime.dll and the model already cached; threads via
/// DREAM_LOCAL_EMBEDDING_THREADS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "benchmark; needs the local model and ONNX Runtime"]
async fn rag_local_embedding_bench() {
    let config = crate::embedding::EmbeddingConfig {
        base_url: "local://embedding".into(),
        api_key: String::new(),
        model: "paraphrase-multilingual-MiniLM-L12-v2".into(),
    };
    let paragraph = "这是一段用于测量建索引耗时的长段落，包含若干关键字：报销、审批、部署、故障复盘。".repeat(20);
    let chunks: Vec<String> = (0..96).map(|i| format!("第{i}节 {paragraph}")).collect();
    // Load the model first so neither measurement includes initialization.
    crate::embedding::embed(&config, &["warm".to_string()]).await.unwrap();

    let started = std::time::Instant::now();
    let indexing = {
        let config = config.clone();
        let chunks = chunks.clone();
        tokio::spawn(async move { crate::embedding::embed(&config, &chunks).await.unwrap().len() })
    };
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let query_started = std::time::Instant::now();
    crate::embedding::embed(&config, &["报销流程需要谁审批".to_string()]).await.unwrap();
    let query_ms = query_started.elapsed().as_millis();
    let embedded = indexing.await.unwrap();
    let total = started.elapsed().as_secs_f64();
    println!(
        "RAG_BENCH local threads={} chunks={embedded} total={total:.1}s rate={:.1}/s query-during-indexing={query_ms}ms",
        std::env::var("DREAM_LOCAL_EMBEDDING_THREADS").unwrap_or_else(|_| "default".into()),
        embedded as f64 / total
    );
}
