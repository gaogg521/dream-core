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

use dream_core_common::recall_terms::QueryTerms;
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

        let terms = QueryTerms::from_query(query);
        if terms.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(usize, &TeamMemoryItem)> = snapshot
            .items
            .iter()
            .filter_map(|item| {
                let score = terms.score(&item.content.to_lowercase());
                (score >= terms.required_score()).then_some((score, item))
            })
            .collect();
        // Strongest match first; ties keep the order the server sent, which is
        // its own relevance order (`sort_by_key` is stable).
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

        scored
            .into_iter()
            .take(MAX_RECALLED)
            .map(|(_, item)| truncate(&item.content))
            .collect()
    }
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

    fn zh_loaded() -> TeamMemoryService {
        let svc = TeamMemoryService::new();
        svc.set_snapshot(TeamMemorySnapshot {
            recall_enabled: true,
            items: vec![
                item("z1", "项目北辰由周明远负责，每周三上午同步进度。"),
                item("z2", "报销单需在每月25日前提交给财务部，逾期顺延到下月。"),
                item("z3", "测试环境的部署密钥每周一轮换。"),
            ],
        });
        svc
    }

    /// The reason this exists: a whole Chinese sentence is one "word" to a
    /// splitter that only breaks on punctuation, so nothing could ever match it.
    #[test]
    fn a_chinese_question_finds_the_item_it_is_about() {
        let hits = zh_loaded().recall("项目北辰是谁负责的？");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].contains("周明远"));

        let hits = zh_loaded().recall("报销要在什么时候之前提交");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].contains("财务部"));
    }

    #[test]
    fn filler_in_a_chinese_question_is_not_a_match() {
        assert!(zh_loaded().recall("今天天气怎么样").is_empty());
        assert!(zh_loaded().recall("你可以帮我一下吗").is_empty());
    }

    /// With enough pairs to afford it, one coincidental pair is not a match.
    #[test]
    fn one_stray_pair_in_a_long_chinese_question_is_not_enough() {
        // "项目" sits in z1, but nothing else in this long question does.
        assert!(
            zh_loaded()
                .recall("项目演示文稿的封面应该选什么颜色比较合适")
                .is_empty()
        );
    }

    #[test]
    fn a_latin_word_still_counts_inside_a_chinese_question() {
        let svc = TeamMemoryService::new();
        svc.set_snapshot(TeamMemorySnapshot {
            recall_enabled: true,
            items: vec![item("m", "Project Zephyr 的负责人是周明远。")],
        });
        let hits = svc.recall("请问 Zephyr 项目负责人电话号码是多少呢");
        assert_eq!(hits.len(), 1, "{hits:?}");
    }
}
