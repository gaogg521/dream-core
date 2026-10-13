//! Pushing a finished scheduled run to the owner's chat bot.
//!
//! The cron crate does not know about channels: it only knows the shape of the
//! hand-off. The composition root implements [`CronResultPusher`] on top of the
//! channel manager, so a job that names a bot as its `push_target` gets its
//! result delivered there without this crate depending on any IM platform.

use async_trait::async_trait;
use dream_core_api_types::CronPushTarget;

/// Longest message handed to a bot. Platforms cap a single message somewhere
/// between 2 000 and 4 096 characters; staying under the smallest keeps one code
/// path working everywhere instead of splitting per platform.
const MAX_PUSH_CHARS: usize = 1800;

const TRUNCATED_NOTE: &str = "\n\n…（内容较长，完整结果请在 One Work 里查看）";

#[async_trait]
pub trait CronResultPusher: Send + Sync {
    /// Deliver `text` to `target` on behalf of `owner_user_id`. Errors are
    /// reported as text: the executor only logs them, a failed push never fails
    /// the run that produced the result.
    async fn push(&self, owner_user_id: &str, target: &CronPushTarget, text: &str) -> Result<(), String>;
}

/// The message a bot receives: the task name as a heading, then the result.
pub fn compose_push_text(job_name: &str, reply: &str) -> String {
    let title = job_name.trim();
    let body = reply.trim();
    let text = if title.is_empty() {
        body.to_owned()
    } else {
        format!("【{title}】\n{body}")
    };
    if text.chars().count() <= MAX_PUSH_CHARS {
        return text;
    }
    let keep = MAX_PUSH_CHARS - TRUNCATED_NOTE.chars().count();
    let mut clipped: String = text.chars().take(keep).collect();
    clipped.push_str(TRUNCATED_NOTE);
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_reply_gets_the_task_name_as_heading() {
        assert_eq!(
            compose_push_text("每日简报", "  今天没有大事  "),
            "【每日简报】\n今天没有大事"
        );
    }

    #[test]
    fn blank_task_name_sends_the_reply_alone() {
        assert_eq!(compose_push_text("  ", "hello"), "hello");
    }

    #[test]
    fn long_reply_is_clipped_to_the_limit_with_a_note() {
        let reply = "字".repeat(5000);
        let text = compose_push_text("t", &reply);
        assert_eq!(text.chars().count(), MAX_PUSH_CHARS);
        assert!(text.ends_with("完整结果请在 One Work 里查看）"));
    }

    #[test]
    fn reply_exactly_at_the_limit_is_untouched() {
        let reply = "a".repeat(MAX_PUSH_CHARS - "【t】\n".chars().count());
        let text = compose_push_text("t", &reply);
        assert_eq!(text.chars().count(), MAX_PUSH_CHARS);
        assert!(!text.contains('…'));
    }
}
