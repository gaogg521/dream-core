use std::time::Duration;

use super::*;

struct Fixed(Vec<String>);

#[async_trait::async_trait]
impl TurnMemoryRecall for Fixed {
    async fn recall(&self, _user_id: &str, _query: &str) -> Vec<String> {
        self.0.clone()
    }
}

struct Slow;

#[async_trait::async_trait]
impl TurnMemoryRecall for Slow {
    async fn recall(&self, _user_id: &str, _query: &str) -> Vec<String> {
        tokio::time::sleep(RECALL_TIMEOUT * 4).await;
        vec!["too late".into()]
    }
}

#[tokio::test]
async fn hits_become_a_block_that_ends_with_a_blank_line() {
    let recall = Fixed(vec!["Zephyr is led by Zhou Mingyuan.".into(), "b".into()]);
    let MemoryPrefix::Text(prefix) = recall_prefix(&recall, "u1", "who leads zephyr").await else {
        panic!("expected a memory block");
    };
    assert!(prefix.starts_with("[Relevant Memory]\n"));
    assert!(prefix.contains("Zhou Mingyuan"));
    // The prompt is concatenated straight onto this, so the separation has to
    // live here or the memory runs into the member's first word.
    assert!(prefix.ends_with("[/Relevant Memory]\n\n"));
}

#[tokio::test]
async fn no_hits_means_no_block() {
    assert!(matches!(
        recall_prefix(&Fixed(Vec::new()), "u1", "anything").await,
        MemoryPrefix::None
    ));
}

/// A slow lookup must cost the turn its patience, not the turn.
#[tokio::test]
async fn a_slow_lookup_times_out_instead_of_delaying_the_turn() {
    let started = std::time::Instant::now();
    assert!(matches!(
        recall_prefix(&Slow, "u1", "anything").await,
        MemoryPrefix::TimedOut
    ));
    assert!(
        started.elapsed() < RECALL_TIMEOUT + Duration::from_millis(400),
        "recall_prefix waited {:?}",
        started.elapsed()
    );
}
