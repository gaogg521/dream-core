// Included in service::tests. Retrieval quality and indexing speed of the bundled local model for different chunk
// sizes, on a real corpus with hand-written questions. Ignored by default; run with:
//   RAG_EVAL_DIR=<dir with corpus/*.txt and questions.json> RAG_EVAL_CONFIGS=800:100,400:60,250:40 \
//   ORT_DYLIB_PATH=<onnxruntime.dll> cargo nextest run --release -p dream-domain-devops \
//     --run-ignored ignored-only rag_chunk_eval --no-capture
// questions.json: [{"q": "...", "key": "a phrase from the passage that answers it"}]

/// A retrieved chunk answers the question when it holds the key, or (for a key split across a chunk boundary) any
/// 12 consecutive characters of it.
fn eval_hit(chunk: &str, key: &str) -> bool {
    let chars: Vec<char> = key.chars().collect();
    let window = chars.len().min(12);
    chars
        .windows(window)
        .any(|w| chunk.contains(&w.iter().collect::<String>()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "evaluation; needs a corpus, the local model and ONNX Runtime"]
async fn rag_chunk_eval() {
    let dir = std::path::PathBuf::from(std::env::var("RAG_EVAL_DIR").expect("RAG_EVAL_DIR"));
    let configs: Vec<(usize, usize)> = std::env::var("RAG_EVAL_CONFIGS")
        .unwrap_or_else(|_| "800:100".into())
        .split(',')
        .map(|pair| {
            let (size, overlap) = pair.split_once(':').unwrap();
            (size.trim().parse().unwrap(), overlap.trim().parse().unwrap())
        })
        .collect();
    let mut corpus: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(dir.join("corpus")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        if !text.trim().is_empty() {
            corpus.push((path.file_stem().unwrap().to_string_lossy().into_owned(), text));
        }
    }
    corpus.sort();
    let questions: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("questions.json")).unwrap()).unwrap();
    let config = crate::embedding::EmbeddingConfig {
        base_url: "local://embedding".into(),
        api_key: String::new(),
        model: "paraphrase-multilingual-MiniLM-L12-v2".into(),
    };
    crate::embedding::embed(&config, &["warm".to_string()]).await.unwrap();

    for (size, overlap) in configs {
        let work = tempfile::tempdir().unwrap();
        let svc = bench_service(work.path()).await;
        svc.set_rag_config("local://embedding", "paraphrase-multilingual-MiniLM-L12-v2", Some(""))
            .await
            .unwrap();
        let mut chunk_total = 0usize;
        let mut embed_time = std::time::Duration::ZERO;
        for (name, text) in &corpus {
            let doc = svc
                .register_rag_document(name, None, None, None, "org", None, "all", "admin1", None)
                .await
                .unwrap();
            let chunks = crate::embedding::chunk_text(text, size, overlap);
            let started = std::time::Instant::now();
            let vectors = crate::embedding::embed(&config, &chunks).await.unwrap();
            embed_time += started.elapsed();
            let mut rows = Vec::new();
            for (index, (chunk, vector)) in chunks.iter().zip(&vectors).enumerate() {
                let id = format!("{}_{index}", doc.id);
                sqlx::query("INSERT INTO one_rag_chunks (id, document_id, chunk_index, content, embedding, created_at) VALUES (?, ?, ?, ?, ?, 0)")
                    .bind(&id).bind(&doc.id).bind(index as i64).bind(chunk)
                    .bind(crate::embedding::pack_embedding(vector))
                    .execute(svc.db.sqlite()).await.unwrap();
                rows.push((id, chunk.clone()));
            }
            sqlx::query("UPDATE one_rag_documents SET status = 'ready', chunk_count = ? WHERE id = ?")
                .bind(chunks.len() as i64)
                .bind(&doc.id)
                .execute(svc.db.sqlite())
                .await
                .unwrap();
            crate::retrieval::sync_document(&svc.db, &doc.id, &rows).await.unwrap();
            chunk_total += chunks.len();
        }

        // Hybrid = the production search path; dense = vectors alone, to see what the model itself contributes.
        let all_chunks: Vec<(String, Vec<u8>)> = sqlx::query_as("SELECT content, embedding FROM one_rag_chunks")
            .fetch_all(svc.db.sqlite())
            .await
            .unwrap();
        let (mut hybrid_hits, mut hybrid_top1, mut hybrid_rr) = (0usize, 0usize, 0f64);
        let (mut dense_hits, mut dense_top1, mut dense_rr) = (0usize, 0usize, 0f64);
        let mut search_time = std::time::Duration::ZERO;
        for question in &questions {
            let (q, key) = (question["q"].as_str().unwrap(), question["key"].as_str().unwrap());
            let started = std::time::Instant::now();
            let hits = svc.search_rag("admin1", q, 5).await.unwrap();
            search_time += started.elapsed();
            if let Some(rank) = hits.iter().position(|hit| eval_hit(&hit.content, key)) {
                hybrid_hits += 1;
                hybrid_top1 += usize::from(rank == 0);
                hybrid_rr += 1.0 / (rank as f64 + 1.0);
            }
            let query = crate::embedding::embed(&config, &[q.to_string()]).await.unwrap().remove(0);
            let mut scored: Vec<(f32, &str)> = all_chunks
                .iter()
                .map(|(content, blob)| {
                    (
                        crate::embedding::cosine_similarity(&query, &crate::embedding::unpack_embedding(blob)),
                        content.as_str(),
                    )
                })
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            if let Some(rank) = scored.iter().take(5).position(|(_, content)| eval_hit(content, key)) {
                dense_hits += 1;
                dense_top1 += usize::from(rank == 0);
                dense_rr += 1.0 / (rank as f64 + 1.0);
            }
        }
        let n = questions.len() as f64;
        let chars: usize = corpus.iter().map(|(_, text)| text.chars().count()).sum();
        println!(
            "RAG_EVAL chunk={size}/{overlap} chunks={chunk_total} index={:.1}s ({:.0} chars/s) | hybrid hit@5={:.0}% hit@1={:.0}% mrr={:.3} | dense hit@5={:.0}% hit@1={:.0}% mrr={:.3} | search avg {:.0}ms",
            embed_time.as_secs_f64(),
            chars as f64 / embed_time.as_secs_f64(),
            hybrid_hits as f64 / n * 100.0,
            hybrid_top1 as f64 / n * 100.0,
            hybrid_rr / n,
            dense_hits as f64 / n * 100.0,
            dense_top1 as f64 / n * 100.0,
            dense_rr / n,
            search_time.as_secs_f64() * 1000.0 / n
        );
    }
}
