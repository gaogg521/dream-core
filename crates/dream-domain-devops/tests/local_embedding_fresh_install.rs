//! Fresh-install acceptance for the **default** embedding path.
//!
//! The lexical half of hybrid retrieval is covered by unit tests against a
//! fake provider, but the *dense* half's default had no coverage at all:
//! every install that configures no endpoint resolves to the bundled local
//! fastembed model (`local://embedding`), and a keyword hit in the UI proves
//! nothing about it. A fresh install must:
//!
//! 1. resolve to the local model with zero configuration;
//! 2. download and initialize it from a **clean** cache dir (first boot) —
//!    this is what a new machine or a new profile actually does;
//! 3. produce vectors that rank a *paraphrase* above unrelated text — i.e.
//!    semantic retrieval, not merely "an endpoint answered".
//!
//! The first test is `#[ignore]`d on purpose: on a clean machine it pulls
//! ~100 MB from the Hugging Face Hub, so it is a network- and time-dependent
//! acceptance check, not a unit test. Run it deliberately:
//!
//! ```text
//! cargo test -p dream-domain-devops --test local_embedding_fresh_install -- --ignored --nocapture
//! ```
//!
//! The second test needs no network: a malformed endpoint must fail fast
//! with a message that names the embedding step, because that string is what
//! lands in `one_rag_documents.last_error` and reaches the admin.

use dream_domain_devops::embedding::{cosine_similarity, embed, resolve_embedding_config, EmbeddingConfig};
use std::path::PathBuf;
use std::sync::OnceLock;

/// A per-process temp dir standing in for `ONE_DATA_DIR` on a fresh install.
/// Removed up front so a re-run genuinely re-downloads the model.
fn fresh_data_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("onework-embedding-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create fresh data dir");
    dir
}

/// Guards the process-global `ONE_DATA_DIR` write: only one test in this
/// binary touches it, but state it explicitly so a future second test does
/// not silently race the first.
static ENV_SET: OnceLock<()> = OnceLock::new();

#[tokio::test]
#[ignore = "downloads the embedding model from HuggingFace into a clean cache dir; run as the deliberate fresh-install acceptance check"]
async fn fresh_install_resolves_local_model_and_retrieves_semantically() {
    let data_dir = fresh_data_dir();
    ENV_SET.get_or_init(|| {
        // SAFETY: this is the only env mutation in this test binary, it
        // happens before any other test reads the variable, and integration
        // tests each run in their own process.
        unsafe { std::env::set_var("ONE_DATA_DIR", &data_dir) };
    });

    // 1. Zero configuration must resolve to the bundled local model — this
    //    is the resolution every personal install and every enterprise
    //    install without the bundled TEI container actually takes.
    let config: EmbeddingConfig = resolve_embedding_config(None, None, None, None)
        .expect("a fresh install must resolve to the bundled local model, not an error");
    assert_eq!(config.base_url, "local://embedding");
    assert_eq!(config.model, "paraphrase-multilingual-MiniLM-L12-v2");

    // 2 + 3. First boot from a clean cache, then semantic ranking. The query
    //    paraphrases document 0 and shares no content words with any of them,
    //    so only the dense ranker can put it first.
    let texts = vec![
        "员工餐补每季度发放一次，直接打入工资卡，无需单独申请。".to_string(),
        "新员工入职当天到行政部领取笔记本电脑、门禁卡和工牌。".to_string(),
        "夏季空调温度不得低于二十六度，最后离开办公室的人负责关闭空调。".to_string(),
        "公司给的午餐补助隔多久才发一次".to_string(),
    ];
    let vectors = embed(&config, &texts)
        .await
        .expect("local embedding must initialize on a clean cache dir (first boot)");

    assert_eq!(vectors.len(), texts.len(), "one vector per input, in order");
    for v in &vectors {
        assert_eq!(v.len(), 384, "paraphrase-multilingual-MiniLM-L12-v2 is a 384-dim model");
    }

    let query = &vectors[3];
    let sims: Vec<f32> = vectors[..3].iter().map(|v| cosine_similarity(query, v)).collect();
    assert!(
        sims[0] > sims[1] && sims[0] > sims[2],
        "the paraphrase must rank the meal-subsidy document first; cosines were {sims:?}"
    );

    // The model must now live in the cache dir — proof the download path ran,
    // not some pre-seeded fallback.
    let model_cache = data_dir.join("models/embedding");
    assert!(
        model_cache.join("models--Qdrant--paraphrase-multilingual-MiniLM-L12-v2-onnx-Q").exists(),
        "model files must be cached under ONE_DATA_DIR/models/embedding"
    );
    let _ = std::fs::remove_dir_all(&data_dir);
}

/// A malformed endpoint must fail fast and name the embedding step: that
/// error string is persisted to `one_rag_documents.last_error` and surfaced
/// to the admin, so "request failed" without context is a support ticket.
/// No network needed — an empty host never leaves the process.
#[tokio::test]
async fn malformed_endpoint_fails_with_a_named_error() {
    let config = EmbeddingConfig {
        base_url: "http://".to_string(),
        api_key: String::new(),
        model: "some-model".to_string(),
    };
    let error = embed(&config, &[String::from("hello")])
        .await
        .expect_err("an endpoint with no host must not silently succeed");
    let message = error.to_string();
    assert!(
        message.contains("embedding"),
        "the failure must name the embedding step so last_error is actionable, got: {message}"
    );
}
