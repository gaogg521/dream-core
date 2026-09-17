use std::time::Instant;

use dashmap::mapref::entry::Entry;

use super::{FINALIZE_DEDUP_WINDOW, TeammateManager};

impl TeammateManager {
    /// Claim the right to finalize this conversation's turn, or report that
    /// someone else already has.
    ///
    /// The check and the claim are one `entry` operation on purpose. Reading
    /// with `get` and then `insert`ing left a gap between them, and duplicate
    /// finish events are precisely the ones that arrive together — two tasks
    /// could both find no recent entry and both proceed, which is the one thing
    /// this exists to prevent.
    pub fn begin_finalize(&self, conversation_id: &str) -> bool {
        let now = Instant::now();
        let should_proceed = match self.finalized_turns.entry(conversation_id.to_owned()) {
            Entry::Occupied(mut entry) => {
                if now.duration_since(*entry.get()) < FINALIZE_DEDUP_WINDOW {
                    false
                } else {
                    entry.insert(now);
                    true
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(now);
                true
            }
        };
        if should_proceed {
            let map = self.finalized_turns.clone();
            let key = conversation_id.to_owned();
            tokio::spawn(async move {
                tokio::time::sleep(FINALIZE_DEDUP_WINDOW).await;
                // Only if this is still our own claim: a later one deserves
                // its own full window, and an unconditional remove would cut it
                // short. Compared by equality rather than elapsed time —
                // `Instant::duration_since` saturates to zero when the argument
                // is the later of the two, so a newer claim would look
                // identical to ours and be removed.
                map.remove_if(&key, |_, claimed_at| *claimed_at == now);
            });
        }
        should_proceed
    }

    pub fn clear_finalized_turn(&self, conversation_id: &str) {
        self.finalized_turns.remove(conversation_id);
    }
}
