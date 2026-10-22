//! Query-side embedding for knowledge search.
//!
//! Search gives the embedding endpoint a short budget and falls back to keyword retrieval when it is slow. The
//! trouble was what happened to the call at that point: it was dropped. A cold endpoint (a model that has to be
//! loaded, a first TLS handshake, the local ONNX engine initialising) therefore never got to finish, every early
//! search paid the same cold start again, and a natural-language question with no literal keyword match came
//! back empty.
//!
//! Here the call is detached instead. It keeps running after the budget expires, its vector is cached, and an
//! identical search a moment later is served from the cache with full semantic ranking. Concurrent identical
//! searches share one request.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use dream_core_common::now_ms;
use tokio::sync::watch;

use crate::embedding::{EmbeddingConfig, embed};
use crate::error::DevopsError;

const CACHE_CAPACITY: usize = 64;
/// Different queries each start their own request; cap them so a hung endpoint cannot pile up tasks.
const MAX_IN_FLIGHT: usize = 8;

type Outcome = Option<Result<Vec<f32>, String>>;

#[derive(Default)]
struct Cache {
    entries: HashMap<String, Vec<f32>>,
    order: VecDeque<String>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, watch::Receiver<Outcome>>>> = LazyLock::new(Mutex::default);
static LAST_QUERY_MS: AtomicI64 = AtomicI64::new(0);

/// A cached vector is only valid for the endpoint and model that produced it.
fn cache_key(config: &EmbeddingConfig, query: &str) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        config.base_url.trim(),
        config.model.trim(),
        query.trim()
    )
}

fn cached(key: &str) -> Option<Vec<f32>> {
    let mut cache = CACHE.lock().ok()?;
    let vector = cache.entries.get(key)?.clone();
    // Most recently used goes to the back.
    cache.order.retain(|existing| existing != key);
    cache.order.push_back(key.to_owned());
    Some(vector)
}

fn remember(key: String, vector: Vec<f32>) {
    let Ok(mut cache) = CACHE.lock() else { return };
    cache.order.retain(|existing| existing != &key);
    cache.order.push_back(key.clone());
    cache.entries.insert(key, vector);
    while cache.order.len() > CACHE_CAPACITY {
        if let Some(oldest) = cache.order.pop_front() {
            cache.entries.remove(&oldest);
        }
    }
}

/// Whether a knowledge search ran within `window`. Keep-alive pings only matter while someone is using search.
pub fn queried_within(window: Duration) -> bool {
    recent(LAST_QUERY_MS.load(Ordering::Relaxed), now_ms(), window)
}

fn recent(last_ms: i64, now: i64, window: Duration) -> bool {
    last_ms > 0 && now - last_ms <= window.as_millis() as i64
}

/// Embed one search query, waiting at most `budget` for the answer.
///
/// On timeout the request is NOT cancelled: it finishes in the background and fills the cache (see the module
/// docs), so the returned error means "not yet", not "never".
pub async fn embed_query(config: &EmbeddingConfig, query: &str, budget: Duration) -> Result<Vec<f32>, DevopsError> {
    LAST_QUERY_MS.store(now_ms(), Ordering::Relaxed);
    let key = cache_key(config, query);
    if let Some(vector) = cached(&key) {
        return Ok(vector);
    }

    let mut receiver = {
        let mut in_flight = IN_FLIGHT
            .lock()
            .map_err(|_| DevopsError::Internal("query embedding state is unavailable".into()))?;
        if let Some(existing) = in_flight.get(&key) {
            existing.clone()
        } else {
            if in_flight.len() >= MAX_IN_FLIGHT {
                return Err(DevopsError::Internal("query embedding is busy".into()));
            }
            let (sender, receiver) = watch::channel::<Outcome>(None);
            in_flight.insert(key.clone(), receiver.clone());
            let (config, query, task_key) = (config.clone(), query.trim().to_owned(), key.clone());
            tokio::spawn(async move {
                let started = std::time::Instant::now();
                let outcome = match embed(&config, &[query]).await {
                    Ok(vectors) => vectors
                        .into_iter()
                        .next()
                        .ok_or_else(|| "empty query embedding".to_owned()),
                    Err(error) => Err(error.to_string()),
                };
                match &outcome {
                    Ok(vector) => {
                        remember(task_key.clone(), vector.clone());
                        tracing::debug!(
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            "query embedding finished"
                        );
                    }
                    Err(error) => tracing::debug!(%error, "query embedding failed"),
                }
                if let Ok(mut in_flight) = IN_FLIGHT.lock() {
                    in_flight.remove(&task_key);
                }
                let _ = sender.send(Some(outcome));
            });
            receiver
        }
    };

    match tokio::time::timeout(budget, receiver.wait_for(Option::is_some)).await {
        Ok(Ok(outcome)) => match outcome.as_ref().cloned() {
            Some(Ok(vector)) => Ok(vector),
            Some(Err(error)) => Err(DevopsError::Internal(error)),
            None => Err(DevopsError::Internal("query embedding produced no result".into())),
        },
        Ok(Err(_)) => Err(DevopsError::Internal(
            "query embedding task ended without a result".into(),
        )),
        Err(_) => Err(DevopsError::Internal(
            "query embedding is still running; it will be cached for the next search".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(model: &str) -> EmbeddingConfig {
        EmbeddingConfig {
            base_url: "http://embedding.invalid".into(),
            api_key: String::new(),
            model: model.into(),
        }
    }

    #[test]
    fn a_cached_vector_is_only_valid_for_the_same_endpoint_model_and_text() {
        let key = cache_key(&config("m1"), "  hello ");
        remember(key.clone(), vec![1.0, 2.0]);
        assert_eq!(cached(&cache_key(&config("m1"), "hello")), Some(vec![1.0, 2.0]));
        assert_eq!(
            cached(&cache_key(&config("m2"), "hello")),
            None,
            "another model must not reuse it"
        );
        assert_eq!(cached(&cache_key(&config("m1"), "hello again")), None);
    }

    #[test]
    fn the_cache_forgets_the_least_recently_used_entry() {
        let first = cache_key(&config("lru"), "first");
        remember(first.clone(), vec![0.0]);
        for index in 0..CACHE_CAPACITY {
            remember(cache_key(&config("lru"), &format!("other-{index}")), vec![index as f32]);
        }
        assert_eq!(cached(&first), None);
        assert!(cached(&cache_key(&config("lru"), &format!("other-{}", CACHE_CAPACITY - 1))).is_some());
    }

    #[test]
    fn activity_counts_as_recent_only_inside_the_window() {
        let window = Duration::from_secs(60);
        assert!(!recent(0, 1_000_000, window), "no query has ever run");
        assert!(recent(990_000, 1_000_000, window));
        assert!(!recent(900_000, 1_000_000, window));
    }
}
