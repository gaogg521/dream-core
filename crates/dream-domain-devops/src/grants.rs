//! Where a viewer's extra reachability comes from.
//!
//! The registries in this crate gate reads on their own `scope` and
//! `visibility` columns, which express reachability in coarse strokes: a
//! resource is reachable by the whole org, by one team, or — when
//! `visibility = 'admin'` — by nobody but administrators. There is no way to
//! say "this one restricted MCP server, for the DevOps department".
//!
//! The enterprise plane's resource-authorization matrix says exactly that, but
//! it lives in `dream-domain-platform`, which is compiled out of the personal
//! edition. This crate is not, and must not drag it in — hence a trait the app
//! layer implements, the same seam every other cross-domain dependency here
//! uses (`TenantResolver`, `CredentialRevoker`, …).
//!
//! # Grants add, unless the tenant opted into a whitelist
//!
//! By default a grant cannot take reachability away. That is what makes wiring
//! this in safe for every existing install: with no matrix configured the
//! resolver contributes nothing, the registries' own predicates decide alone,
//! and behaviour is bit-for-bit what it was. It also means the personal edition
//! — where there is no source at all — needs no special case.
//!
//! Additive is not what an administrator assumes, though. Someone who grants a
//! department exactly three skills reads that as a whitelist, and is not told
//! the department still reaches every `visibility = 'all'` skill in its scopes.
//! So a tenant may opt one resource type into [`ExtraGrants::restrictive`],
//! where granted means *only* the granted ones.
//!
//! That mode is deliberately hard to enter by accident, because its failure
//! mode is "the member sees nothing": it requires an explicit, readable
//! setting, and every unhappy path on the way to reading it — no tenant, an
//! unreadable matrix, an unreadable mode row, a value from a newer version —
//! resolves back to additive. Widening on a bad read is recoverable; blanking
//! a member's whole skill list is not.

use async_trait::async_trait;

/// Resource kinds the matrix can grant. These strings are the wire values
/// shared with `dream-domain-platform`'s `GRANT_RESOURCE_TYPES`; they are
/// persisted in `one_resource_grants.resource_type`, so they are not free to
/// rename.
pub mod resource_type {
    pub const SKILL: &str = "skill";
    pub const MCP: &str = "mcp";
    pub const KNOWLEDGE: &str = "knowledge";
    pub const MODEL_CHANNEL: &str = "model_channel";
}

/// What a viewer may additionally reach, beyond what `scope`/`visibility`
/// already allow.
#[derive(Debug, Clone, Default)]
pub struct ExtraGrants {
    /// A wildcard grant: every resource of this type, including ones created
    /// later. When true `ids` carries nothing — there is nothing to enumerate.
    pub all: bool,
    /// Explicitly granted resource ids.
    pub ids: Vec<String>,
    /// When true the tenant reads the matrix as a whitelist for this resource
    /// type: the grants below are the *only* thing reachable, instead of an
    /// addition to what `scope`/`visibility` already allow.
    ///
    /// `false` is the default and the value every failure path resolves to —
    /// see this module's header for why that asymmetry is deliberate.
    pub restrictive: bool,
}

impl ExtraGrants {
    /// True when this contributes nothing, so callers can skip the extra SQL
    /// entirely rather than building a predicate that can never match.
    ///
    /// Only meaningful in additive mode: under [`Self::restrictive`] an empty
    /// grant set is not "nothing to add", it is "nothing is reachable", which
    /// is a predicate the caller must still apply.
    pub fn is_empty(&self) -> bool {
        !self.all && self.ids.is_empty()
    }
}

/// Resolves a viewer's matrix grants. Wired by the app layer in the enterprise
/// edition; absent everywhere else.
#[async_trait]
pub trait ResourceGrantSource: Send + Sync {
    /// Never fails the caller: a matrix that cannot be read must not make
    /// registries disappear, so implementations log and return no grants
    /// rather than surfacing an error. The viewer still sees everything their
    /// `scope`/`visibility` already allowed.
    async fn extra_grants(&self, viewer_user_id: &str, resource_type: &str) -> ExtraGrants;
}
