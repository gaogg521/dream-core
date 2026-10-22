//! In-memory dense index for knowledge search (SQLite deployments).
//!
//! Search used to read every visible chunk — its text AND its embedding blob — out of SQLite on every query and
//! compute a cosine against each one in turn. On a 45 000-chunk library that is ~70 MB pulled through SQLite per
//! search. Here the embeddings live in memory instead, quantized to `i8` (one byte per dimension, a quarter of the
//! `f32` size), and a query only ranks them approximately in memory. The handful of best candidates are then read
//! back from SQLite with their exact vectors and text, so the scores callers see (and threshold on) are still the
//! exact cosines, and the viewer's access predicate is still applied in SQL.
//!
//! # Staying in step with the table
//!
//! Every insert or delete on `one_rag_chunks` — from this service, from the office editor, from a backup restore in
//! another crate, from another process sharing the database file — appends the document id to a change log via
//! SQLite triggers. Before ranking, the index replays the log since its cursor and reloads just those documents. A
//! log it cannot account for (pruned past its cursor, or a database that went backwards) triggers a full reload.

use std::collections::{HashMap, HashSet};

use dream_core_db::{DbBackend, DbPool};
use sqlx::Row;

use crate::error::DevopsError;

pub(crate) const CHANGE_LOG: &str = "one_rag_chunk_changes";
/// Log entries older than this many writes behind the newest are pruned.
const CHANGE_LOG_KEEP: i64 = 50_000;
/// Document ids per reload query.
const RELOAD_BATCH: usize = 400;

/// How many approximate candidates to re-score exactly for a ranker that wants `candidates`. Generous on purpose:
/// quantization can swap near-ties, and re-scoring a few dozen rows is cheap.
pub(crate) fn rescore_pool(candidates: usize) -> usize {
    (candidates * 3).max(60)
}

/// One document's chunk vectors, quantized.
struct DocBlock {
    chunk_ids: Vec<String>,
    dims: usize,
    /// `chunk_ids.len() * dims` codes, row-major.
    codes: Vec<i8>,
    /// Per chunk: multiply an `i8` dot product by this to get back to the normalized `f32` scale.
    scales: Vec<f32>,
}

#[derive(Default)]
struct State {
    loaded: bool,
    log_ready: bool,
    cursor: i64,
    docs: HashMap<String, DocBlock>,
}

#[derive(Default)]
pub(crate) struct VectorIndex {
    state: tokio::sync::RwLock<State>,
}

/// Normalize, then quantize to `i8`. `None` for an empty or all-zero vector.
fn quantize(vector: &[f32]) -> Option<(Vec<i8>, f32)> {
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if vector.is_empty() || norm <= f32::EPSILON || !norm.is_finite() {
        return None;
    }
    let max_abs = vector.iter().fold(0.0f32, |acc, x| acc.max((x / norm).abs()));
    if max_abs <= f32::EPSILON {
        return None;
    }
    let scale = max_abs / 127.0;
    let codes = vector
        .iter()
        .map(|x| ((x / norm) / scale).round().clamp(-127.0, 127.0) as i8)
        .collect();
    Some((codes, scale))
}

impl DocBlock {
    fn from_rows(rows: Vec<(String, Vec<u8>)>) -> Option<Self> {
        let mut block = DocBlock {
            chunk_ids: Vec::with_capacity(rows.len()),
            dims: 0,
            codes: Vec::new(),
            scales: Vec::with_capacity(rows.len()),
        };
        for (chunk_id, blob) in rows {
            let vector = crate::embedding::unpack_embedding(&blob);
            if block.dims == 0 {
                block.dims = vector.len();
            }
            // One model per index: a chunk left over from another dimension can never be compared.
            if vector.len() != block.dims {
                continue;
            }
            let Some((codes, scale)) = quantize(&vector) else {
                continue;
            };
            block.chunk_ids.push(chunk_id);
            block.codes.extend_from_slice(&codes);
            block.scales.push(scale);
        }
        (!block.chunk_ids.is_empty()).then_some(block)
    }
}

/// Create the change log and its triggers. Runtime rather than a migration, like the FTS index: it is a derived
/// structure, and MySQL deployments do not use it.
async fn ensure_change_log(pool: &DbPool) -> Result<(), DevopsError> {
    for sql in [
        format!(
            "CREATE TABLE IF NOT EXISTS {CHANGE_LOG} (seq INTEGER PRIMARY KEY AUTOINCREMENT, document_id TEXT NOT NULL)"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS one_rag_chunks_log_insert AFTER INSERT ON one_rag_chunks \
             BEGIN INSERT INTO {CHANGE_LOG} (document_id) VALUES (NEW.document_id); END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS one_rag_chunks_log_delete AFTER DELETE ON one_rag_chunks \
             BEGIN INSERT INTO {CHANGE_LOG} (document_id) VALUES (OLD.document_id); END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS one_rag_chunks_log_update AFTER UPDATE OF embedding, document_id \
             ON one_rag_chunks \
             BEGIN INSERT INTO {CHANGE_LOG} (document_id) VALUES (OLD.document_id); \
                   INSERT INTO {CHANGE_LOG} (document_id) VALUES (NEW.document_id); END"
        ),
    ] {
        pool.execute(&sql, &[]).await?;
    }
    Ok(())
}

impl VectorIndex {
    /// Whether this deployment can use the index at all.
    pub(crate) fn supported(pool: &DbPool) -> bool {
        pool.backend() == DbBackend::Sqlite
    }

    /// Bring the index in step with `one_rag_chunks`.
    pub(crate) async fn refresh(&self, pool: &DbPool) -> Result<(), DevopsError> {
        if !Self::supported(pool) {
            return Ok(());
        }
        // Cheap read first: the common case is "nothing changed".
        {
            let state = self.state.read().await;
            if state.loaded && state.log_ready {
                let (newest, _) = log_bounds(pool).await?;
                if newest == state.cursor {
                    return Ok(());
                }
            }
        }
        let mut state = self.state.write().await;
        if !state.log_ready {
            ensure_change_log(pool).await?;
            state.log_ready = true;
        }
        let (newest, oldest) = log_bounds(pool).await?;
        let replayable = state.loaded
            && newest >= state.cursor
            // The entry right after our cursor must still be in the log, or we missed something.
            && (newest == state.cursor || oldest <= state.cursor + 1);
        if !replayable {
            // Read the log position BEFORE the rows: a write landing in between is replayed next time.
            let started = std::time::Instant::now();
            let rows: Vec<(String, String, Vec<u8>)> = pool
                .fetch_all_as(
                    "SELECT id, document_id, embedding FROM one_rag_chunks WHERE embedding IS NOT NULL",
                    &[],
                )
                .await?;
            let mut grouped: HashMap<String, Vec<(String, Vec<u8>)>> = HashMap::new();
            let chunks = rows.len();
            for (chunk_id, document_id, blob) in rows {
                grouped.entry(document_id).or_default().push((chunk_id, blob));
            }
            state.docs = grouped
                .into_iter()
                .filter_map(|(document_id, rows)| DocBlock::from_rows(rows).map(|block| (document_id, block)))
                .collect();
            state.cursor = newest;
            state.loaded = true;
            tracing::info!(
                chunks,
                documents = state.docs.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "knowledge vector index loaded"
            );
        } else if newest > state.cursor {
            let changed: Vec<String> = pool
                .fetch_all_scalar(
                    &format!("SELECT DISTINCT document_id FROM {CHANGE_LOG} WHERE seq > ? AND seq <= ?"),
                    &dream_core_db::db_params![state.cursor, newest],
                )
                .await?;
            for batch in changed.chunks(RELOAD_BATCH) {
                let placeholders = vec!["?"; batch.len()].join(",");
                let params: Vec<dream_core_db::DbValue> = batch.iter().map(|id| id.as_str().into()).collect();
                let rows: Vec<(String, String, Vec<u8>)> = pool
                    .fetch_all_as(
                        &format!(
                            "SELECT id, document_id, embedding FROM one_rag_chunks \
                             WHERE embedding IS NOT NULL AND document_id IN ({placeholders})"
                        ),
                        &params,
                    )
                    .await?;
                let mut grouped: HashMap<String, Vec<(String, Vec<u8>)>> = HashMap::new();
                for (chunk_id, document_id, blob) in rows {
                    grouped.entry(document_id).or_default().push((chunk_id, blob));
                }
                for document_id in batch {
                    match grouped.remove(document_id).and_then(DocBlock::from_rows) {
                        Some(block) => {
                            state.docs.insert(document_id.clone(), block);
                        }
                        None => {
                            state.docs.remove(document_id);
                        }
                    }
                }
            }
            state.cursor = newest;
        }
        if newest - oldest > CHANGE_LOG_KEEP * 2 {
            pool.execute(
                &format!("DELETE FROM {CHANGE_LOG} WHERE seq <= ?"),
                &dream_core_db::db_params![newest - CHANGE_LOG_KEEP],
            )
            .await?;
        }
        Ok(())
    }

    /// Approximate top-`limit` chunk ids among `visible_documents`, best first.
    pub(crate) async fn candidates(
        &self,
        pool: &DbPool,
        query: &[f32],
        visible_documents: &HashSet<String>,
        limit: usize,
    ) -> Result<Vec<String>, DevopsError> {
        self.refresh(pool).await?;
        let norm = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        if query.is_empty() || norm <= f32::EPSILON {
            return Ok(Vec::new());
        }
        let query: Vec<f32> = query.iter().map(|x| x / norm).collect();
        let state = self.state.read().await;
        let mut scored: Vec<(f32, &str)> = Vec::new();
        for document_id in visible_documents {
            let Some(block) = state.docs.get(document_id) else {
                continue;
            };
            if block.dims != query.len() {
                continue;
            }
            for (row, chunk_id) in block.chunk_ids.iter().enumerate() {
                let codes = &block.codes[row * block.dims..(row + 1) * block.dims];
                let dot: f32 = codes.iter().zip(&query).map(|(code, q)| f32::from(*code) * q).sum();
                scored.push((dot * block.scales[row], chunk_id.as_str()));
            }
        }
        let limit = limit.min(scored.len());
        if limit == 0 {
            return Ok(Vec::new());
        }
        let by_score = |a: &(f32, &str), b: &(f32, &str)| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal);
        if limit < scored.len() {
            scored.select_nth_unstable_by(limit - 1, by_score);
            scored.truncate(limit);
        }
        scored.sort_by(by_score);
        Ok(scored.into_iter().map(|(_, id)| id.to_owned()).collect())
    }

    /// How many chunks the index holds (diagnostics and tests).
    #[cfg(test)]
    pub(crate) async fn chunk_count(&self) -> usize {
        self.state
            .read()
            .await
            .docs
            .values()
            .map(|block| block.chunk_ids.len())
            .sum()
    }
}

/// `(newest seq, oldest seq)` in the change log; `(0, 0)` when empty.
async fn log_bounds(pool: &DbPool) -> Result<(i64, i64), DevopsError> {
    let row = sqlx::query(&format!(
        "SELECT COALESCE(MAX(seq), 0) AS newest, COALESCE(MIN(seq), 0) AS oldest FROM {CHANGE_LOG}"
    ))
    .fetch_one(pool.sqlite())
    .await
    .map_err(|e| DevopsError::Internal(format!("vector index log read failed: {e}")))?;
    Ok((row.try_get("newest").unwrap_or(0), row.try_get("oldest").unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_dot_products_rank_like_exact_cosines() {
        let a = vec![0.9, 0.1, 0.0, -0.3];
        let b = vec![-0.2, 0.8, 0.4, 0.1];
        let query = vec![1.0, 0.2, 0.0, -0.2];
        let exact = |v: &[f32]| crate::embedding::cosine_similarity(&query, v);
        let approx = |v: &[f32]| {
            let (codes, scale) = quantize(v).unwrap();
            let norm = query.iter().map(|x| x * x).sum::<f32>().sqrt();
            codes
                .iter()
                .zip(&query)
                .map(|(c, q)| f32::from(*c) * q / norm)
                .sum::<f32>()
                * scale
        };
        assert!((approx(&a) - exact(&a)).abs() < 0.02);
        assert!((approx(&b) - exact(&b)).abs() < 0.02);
        assert!(approx(&a) > approx(&b));
    }

    #[test]
    fn empty_and_zero_vectors_are_not_indexed() {
        assert!(quantize(&[]).is_none());
        assert!(quantize(&[0.0, 0.0]).is_none());
    }
}
