//! Config-vault consumption seam (P1-5, the "真生效" half).
//!
//! Skill-registry `content` may embed `{{config.<set-alias>.<key>}}` tokens.
//! The vault that holds the values lives in `dream-domain-platform`; this
//! trait is the seam that expands them **when a member fetches skills for
//! team-sync**, so the `SKILL.md` materialised on their disk carries real
//! values instead of the placeholder syntax.
//!
//! Admin-facing registry reads deliberately keep the raw tokens: an admin
//! manages the vault and audits references against the literal
//! `{{config.…}}` string (that is exactly what the reference-count query
//! matches), and a list endpoint must never emit a decrypted sensitive
//! value.
//!
//! `None` (personal edition, tests) => content passes through untouched,
//! bit-for-bit the behaviour that shipped before the vault existed.

use async_trait::async_trait;

#[async_trait]
pub trait ConfigResolver: Send + Sync {
    /// Expand every `{{config.<set>.<key>}}` token in `content` using the
    /// config vault of the tenant `viewer_user_id` belongs to. A token whose
    /// set or key does not resolve is left verbatim — a skill author's typo
    /// must not silently blank a value.
    async fn resolve(&self, viewer_user_id: &str, content: &str) -> String;
}
