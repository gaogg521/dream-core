//! Where a viewer's extra reachability comes from.
//!
//! The four registries in this crate gate reads on their own `scope` and
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
//! # Grants only ever add
//!
//! A grant cannot take reachability away. That is what makes wiring this in
//! safe for every existing install: with no matrix configured the resolver
//! contributes nothing, the registries' own predicates decide alone, and
//! behaviour is bit-for-bit what it was. It also means the personal edition —
//! where there is no source at all — needs no special case.
//!
//! If restrictive semantics are ever wanted ("granted means *only* the granted
//! ones"), that is a different feature with a migration story: every existing
//! deployment would go from "members see the team's skills" to "members see
//! nothing" the moment it switched on. It is deliberately not what this is.

use async_trait::async_trait;

/// Resource kinds the matrix can grant. These strings are the wire values
/// shared with `dream-domain-platform`'s `GRANT_RESOURCE_TYPES`; they are
/// persisted in `one_resource_grants.resource_type`, so they are not free to
/// rename.
pub mod resource_type {
    pub const SKILL: &str = "skill";
    pub const MCP: &str = "mcp";
    pub const KNOWLEDGE: &str = "knowledge";
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
}

impl ExtraGrants {
    /// True when this contributes nothing, so callers can skip the extra SQL
    /// entirely rather than building a predicate that can never match.
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
