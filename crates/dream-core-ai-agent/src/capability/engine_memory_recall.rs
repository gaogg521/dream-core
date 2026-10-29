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

/// Characters of a file's text searched for the request's terms. Merged files are long, so this is
/// generous; an ordinary memory is far below it.
const MAX_SCAN_CHARS: usize = 20_000;

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

/// Memory files in `dir` whose name, description, index line or text match `query`, best first.
///
/// `already_sent` maps a file to the modification time it had when it was last brought into this
/// conversation. A file that has not changed since is left out: its text is still in the
/// conversation, and repeating it each turn would cost tokens for nothing.
///
/// A file qualifies when the request's terms are found anywhere in it, header or text. It ranks
/// by how much of the request its *header* matches first — a memory whose name or description
/// says the thing outranks one that merely mentions it — and by the whole file second.
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

    let mut scored: Vec<Candidate> = headers
        .iter()
        .filter_map(|header| {
            let modified: SystemTime = header.mtime.into();
            if already_sent.get(&header.filename) == Some(&modified) {
                return None;
            }
            let entry = read_memory(&header.file_path).ok()?;
            let index_line = index
                .lines()
                .find(|line| line.contains(&format!("({})", header.filename)))
                .unwrap_or_default();
            let header_text = format!(
                "{} {} {}",
                header.filename.trim_end_matches(".md"),
                header.description.as_deref().unwrap_or_default(),
                index_line
            )
            .to_lowercase();
            let body_text: String = entry
                .content
                .chars()
                .take(MAX_SCAN_CHARS)
                .collect::<String>()
                .to_lowercase();
            let total = terms.score(&format!("{header_text} {body_text}"));
            (total >= required).then(|| Candidate {
                header_score: terms.score(&header_text),
                total,
                modified,
                filename: header.filename.clone(),
                name: entry.frontmatter.name.clone(),
                content: entry.content,
            })
        })
        .collect();
    // Best match first; equal scores keep the scan order, which is newest first.
    scored.sort_by_key(|c| std::cmp::Reverse((c.header_score, c.total)));

    scored
        .into_iter()
        .take(MAX_FILES)
        .filter_map(|candidate| {
            let body = select_body(candidate.content.trim(), &terms);
            if body.is_empty() {
                return None;
            }
            Some(RecalledMemory {
                name: candidate
                    .name
                    .unwrap_or_else(|| candidate.filename.trim_end_matches(".md").to_owned()),
                filename: candidate.filename,
                body,
                modified: candidate.modified,
            })
        })
        .collect()
}

/// A memory file that matched, with what is needed to rank and present it.
struct Candidate {
    header_score: usize,
    total: usize,
    modified: SystemTime,
    filename: String,
    name: Option<String>,
    content: String,
}

/// Marker the app writes as the first line of a file it made by merging several memories.
/// Such a file is a list of `## ` sections, one per original memory, and only the sections that
/// match the request are brought in — a merged file is far too long to send whole.
pub const MERGED_MARKER: &str = "<!-- merged-memory -->";

fn select_body(content: &str, terms: &QueryTerms) -> String {
    if !content.starts_with(MERGED_MARKER) {
        return truncate(content);
    }
    let mut sections: Vec<(usize, &str)> = content
        .trim_start_matches(MERGED_MARKER)
        .split("\n## ")
        .map(str::trim)
        .filter(|section| !section.is_empty())
        .map(|section| (terms.score(&section.to_lowercase()), section))
        .filter(|(score, _)| *score > 0)
        .collect();
    sections.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

    let mut out = String::new();
    for (_, section) in sections {
        let piece = format!("## {}", section.trim_start_matches("## "));
        let used = out.chars().count();
        if used > 0 && used + piece.chars().count() > MAX_BODY_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&truncate(&piece));
    }
    out
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
