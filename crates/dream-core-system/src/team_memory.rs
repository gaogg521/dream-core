//! Local recall over the company memory a member is allowed to read.
//!
//! # Why the items come down instead of the query going up
//!
//! Enterprise memory recall lives in the turn orchestrator, and the turn
//! orchestrator that runs a desktop member's conversation is the co-located
//! personal build, where the enterprise provider is not compiled in. So the
//! client showed a "recall my company memory in conversations" switch, the
//! member turned it on, and nothing was ever recalled — the page said
//! something that was not true of the product they were using.
//!
//! There were two ways to close that. Sending each prompt up to the company's
//! `/api/one/memory/search` would be less code, and it would also mean every
//! prompt a member types leaves their machine — on a client whose whole
//! premise is that conversation content stays local unless they explicitly
//! share it. That is a privacy posture change wearing a bugfix's clothes.
//!
//! So the items travel instead: the renderer syncs the member's readable
//! memory into this process on the same timer as the content rules, and the
//! matching happens here. Nothing about the conversation leaves the machine,
//! and the mechanism is the one the codebase already uses twice.
//!
//! # Matching
//!
//! Word-overlap, not substring. Feeding a whole sentence to a `LIKE` never
//! matches anything — a mistake the server-side implementation made and had to
//! correct — so a query is split into words and an item scores by how many of
//! them it contains. Ranked by hits, capped, and empty when nothing matches:
//! injecting unrelated memory into a prompt is worse than injecting none.

use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// Most items to inject into one turn.
///
/// The recalled text is prepended to the prompt, so this is a budget against
/// crowding out the member's own words, not a performance limit.
const MAX_RECALLED: usize = 5;

/// Longest single item to inject, in characters. Mirrors the server-side
/// truncation so a member sees the same shape either way.
const MAX_ITEM_CHARS: usize = 400;

/// One memory entry as the member's client synced it down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemoryItem {
    pub id: String,
    pub content: String,
}

/// What the renderer pushes into `POST /api/one/team-memory/items`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemorySnapshot {
    /// The member's own `recallEnabled` preference, carried with the items.
    ///
    /// Carried rather than inferred: the switch is the member's, it lives on
    /// the server, and the only honest way for this process to respect it is
    /// to be told. Off means recall returns nothing even though the items are
    /// present — turning the switch off must take effect without waiting for a
    /// purge to arrive.
    #[serde(default)]
    pub recall_enabled: bool,
    #[serde(default)]
    pub items: Vec<TeamMemoryItem>,
}

#[derive(Debug, Default)]
pub struct TeamMemoryService {
    snapshot: RwLock<TeamMemorySnapshot>,
}

impl TeamMemoryService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the synced set wholesale.
    ///
    /// Whole-set replacement is what makes deletion work: an item an
    /// administrator removed upstream has to disappear here, and a merge could
    /// never express that.
    pub fn set_snapshot(&self, snapshot: TeamMemorySnapshot) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = snapshot;
        }
    }

    pub fn item_count(&self) -> usize {
        self.snapshot.read().map(|s| s.items.len()).unwrap_or(0)
    }

    /// Items worth putting in front of this prompt, best first.
    pub fn recall(&self, query: &str) -> Vec<String> {
        let Ok(snapshot) = self.snapshot.read() else {
            return Vec::new();
        };
        if !snapshot.recall_enabled || snapshot.items.is_empty() {
            return Vec::new();
        }

        let words: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| is_selective(word))
            .map(str::to_lowercase)
            .collect();
        if words.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(usize, &TeamMemoryItem)> = snapshot
            .items
            .iter()
            .filter_map(|item| {
                let haystack = item.content.to_lowercase();
                let hits = words.iter().filter(|word| haystack.contains(*word)).count();
                (hits > 0).then_some((hits, item))
            })
            .collect();
        // Strongest match first; ties keep the order the server sent, which is
        // its own relevance order (`sort_by_key` is stable).
        scored.sort_by_key(|(hits, _)| std::cmp::Reverse(*hits));

        scored
            .into_iter()
            .take(MAX_RECALLED)
            .map(|(_, item)| truncate(&item.content))
            .collect()
    }
}

/// Whether a query word carries enough signal to match on.
///
/// Without this, "the" matches almost every sentence ever written and recall
/// fills the prompt with whatever happened to be stored — the first version
/// here surfaced an unrelated item for "what is the weather tomorrow" because
/// one memory began with "The". Injecting the wrong memory is worse than
/// injecting none: the member cannot see why the model suddenly knows about
/// deploy keys.
///
/// A length floor rather than a stopword list, because a stopword list is one
/// per language and this text is routinely mixed. Latin words need four
/// characters, which clears the/is/who/and while keeping anything anyone would
/// actually search for; scripts that do not separate words — CJK above all —
/// need two, where two characters is usually already a whole word.
fn is_selective(word: &str) -> bool {
    let len = word.chars().count();
    if word.is_ascii() { len >= 4 } else { len >= 2 }
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_ITEM_CHARS {
        return text.to_owned();
    }
    // By characters, not bytes: this text is routinely CJK, and slicing a
    // multi-byte character in half panics.
    text.chars().take(MAX_ITEM_CHARS).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, content: &str) -> TeamMemoryItem {
        TeamMemoryItem {
            id: id.into(),
            content: content.into(),
        }
    }

    fn loaded() -> TeamMemoryService {
        let svc = TeamMemoryService::new();
        svc.set_snapshot(TeamMemorySnapshot {
            recall_enabled: true,
            items: vec![
                item("m1", "The staging deploy key rotates every Monday."),
                item("m2", "Project Zephyr is led by Zhou Mingyuan."),
                item("m3", "Invoices go to finance@example.com."),
            ],
        });
        svc
    }

    #[test]
    fn an_unsynced_machine_recalls_nothing() {
        assert!(TeamMemoryService::new().recall("zephyr").is_empty());
    }

    #[test]
    fn the_members_switch_is_respected_even_with_items_present() {
        let svc = loaded();
        assert!(!svc.recall("zephyr").is_empty());

        svc.set_snapshot(TeamMemorySnapshot {
            recall_enabled: false,
            items: vec![item("m2", "Project Zephyr is led by Zhou Mingyuan.")],
        });
        assert!(svc.recall("zephyr").is_empty());
    }

    #[test]
    fn matching_is_by_word_not_by_whole_sentence() {
        // The server-side implementation originally fed the whole sentence to
        // a LIKE and therefore never matched anything. Same query here.
        let hits = loaded().recall("who leads project zephyr?");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].contains("Zhou Mingyuan"));
    }

    #[test]
    fn the_best_match_comes_first() {
        // "rotates" and "deploy" both hit m1; only "invoices" hits m3.
        let hits = loaded().recall("deploy rotates invoices");
        assert_eq!(hits.len(), 2);
        assert!(
            hits[0].contains("deploy key"),
            "expected the two-word match first: {hits:?}"
        );
    }

    /// Short common words must not be what a match is built on.
    ///
    /// "The" opens one of the fixtures, so before the selectivity floor this
    /// query recalled a note about deploy keys — the model would have been
    /// handed company memory that had nothing to do with the question.
    #[test]
    fn a_stopword_alone_is_not_a_match() {
        assert!(loaded().recall("what is the weather tomorrow").is_empty());
        assert!(loaded().recall("is it the one").is_empty());
    }

    #[test]
    fn a_long_item_is_cut_on_a_character_boundary() {
        let svc = TeamMemoryService::new();
        svc.set_snapshot(TeamMemorySnapshot {
            recall_enabled: true,
            items: vec![item("m1", &"记忆".repeat(500))],
        });
        let hits = svc.recall("记忆");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].ends_with('…'));
        assert_eq!(hits[0].chars().count(), MAX_ITEM_CHARS + 1);
    }
}
