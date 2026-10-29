//! Bring the assistant's own saved memories that a request mentions into that turn.
//!
//! The engine puts only a short index of recent memories in the system prompt (see
//! `dream_engine_memory::index::index_for_prompt`), because every conversation pays for what is
//! there. A memory that has dropped out of that short list, or that the model would not think to
//! open, would otherwise stay unused until the model happened to list the directory. So the
//! request itself is matched against each memory's name, description and index line — the
//! cheap, keyword-like part — and only the files that match are read and put in front of the
//! prompt. The rest of the memory costs nothing.
//!
//! The same word and character-pair matching as company memory (`dream_core_common::recall_terms`)
//! is used, so Chinese questions find Chinese memories.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use dream_core_common::recall_terms::QueryTerms;
use dream_engine_memory::index::read_index;
use dream_engine_memory::paths::ENTRYPOINT_NAME;
use dream_engine_memory::store::{read_memory, scan_memory_files};

/// Most memory files to bring into one turn.
const MAX_FILES: usize = 3;

/// Longest body to bring in, in characters. A memory is meant to be short; this only bounds a
/// file that grew long, so a single hit cannot crowd out the person's own words.
const MAX_BODY_CHARS: usize = 800;

/// One memory file chosen for a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecalledMemory {
    pub filename: String,
    pub name: String,
    pub body: String,
    /// When the file was last written; used to bring a changed file in again.
    pub modified: SystemTime,
}

/// Memory files in `dir` whose name, description or index line match `query`, best first.
///
/// `already_sent` maps a file to the modification time it had when it was last brought into this
/// conversation. A file that has not changed since is left out: its text is still in the
/// conversation, and repeating it each turn would cost tokens for nothing.
pub fn recall_engine_memory(
    dir: &Path,
    query: &str,
    already_sent: &HashMap<String, SystemTime>,
) -> Vec<RecalledMemory> {
    let terms = QueryTerms::from_query(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let Ok(headers) = scan_memory_files(dir) else {
        return Vec::new();
    };
    if headers.is_empty() {
        return Vec::new();
    }
    let index = read_index(&dir.join(ENTRYPOINT_NAME));
    let required = terms.required_score();

    let mut scored: Vec<(usize, SystemTime, String, std::path::PathBuf)> = headers
        .iter()
        .filter_map(|header| {
            let modified: SystemTime = header.mtime.into();
            if already_sent.get(&header.filename) == Some(&modified) {
                return None;
            }
            let index_line = index
                .lines()
                .find(|line| line.contains(&format!("({})", header.filename)))
                .unwrap_or_default();
            let haystack = format!(
                "{} {} {}",
                header.filename.trim_end_matches(".md"),
                header.description.as_deref().unwrap_or_default(),
                index_line
            )
            .to_lowercase();
            let score = terms.score(&haystack);
            (score >= required).then(|| (score, modified, header.filename.clone(), header.file_path.clone()))
        })
        .collect();
    // Best match first; equal scores keep the scan order, which is newest first.
    scored.sort_by_key(|(score, ..)| std::cmp::Reverse(*score));

    scored
        .into_iter()
        .take(MAX_FILES)
        .filter_map(|(_, modified, filename, path)| {
            let entry = read_memory(&path).ok()?;
            let body = truncate(entry.content.trim());
            if body.is_empty() {
                return None;
            }
            Some(RecalledMemory {
                name: entry
                    .frontmatter
                    .name
                    .unwrap_or_else(|| filename.trim_end_matches(".md").to_owned()),
                filename,
                body,
                modified,
            })
        })
        .collect()
}

/// The block that goes in front of the person's prompt for these memories.
pub fn format_engine_memory_prefix(memories: &[RecalledMemory]) -> String {
    let body = memories
        .iter()
        .map(|memory| format!("## {}\n{}", memory.name, memory.body))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("[Saved Memory]\n{body}\n[/Saved Memory]\n\n")
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_BODY_CHARS {
        return text.to_owned();
    }
    // By characters: this text is routinely CJK, and cutting a multi-byte character panics.
    text.chars().take(MAX_BODY_CHARS).collect::<String>() + "…"
}

#[cfg(test)]
#[path = "engine_memory_recall_test.rs"]
mod engine_memory_recall_test;
