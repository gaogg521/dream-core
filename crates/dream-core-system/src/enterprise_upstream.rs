//! The client's channel to the company server.
//!
//! # Why this exists (C0-1, plan A)
//!
//! The governance routers a desktop member's calls need live on the company
//! server, but the local backend process has neither the server's address nor
//! the member's token — both live in the renderer, which routes by path prefix
//! and holds the session. Terminal-tool **approval** is the first capability
//! that cannot work that way: the block happens inside the tool call, on this
//! machine, and must last until an administrator decides — far longer than any
//! HTTP request the renderer could proxy.
//!
//! So the renderer pushes the server address and the member's token here (see
//! the `/api/enterprise/upstream` route), and the tool-call gate drives the
//! company's own `/api/workflow/tasks` over it. The token is a live company
//! credential: it is held in memory only, never written to disk or logs, and
//! the renderer clears it on revocation (`clearTeamResources`) the same way it
//! clears the cached channel tokens.
//!
//! # What this is not
//!
//! Not a general proxy. Only code that can explain why it needs the company
//! server gets to read the channel, and today that is the terminal-approval
//! gate alone.

use std::sync::RwLock;

/// The company server address and the member credential for reaching it.
///
/// Wire shape of the renderer's push (`POST /api/enterprise/upstream`) in
/// camelCase: `baseUrl` + `token`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnterpriseUpstream {
    /// Origin of the company server, e.g. `https://one.example.com`. No
    /// trailing slash.
    pub base_url: String,
    /// The member's JWT. Held in memory for as long as the session lasts.
    pub token: String,
}

/// Holds the channel for this process. One instance, shared by the route that
/// writes it and the gate that reads it. Starts empty: a client that has never
/// synced has no channel, and the gate reports that rather than guessing.
#[derive(Debug, Default)]
pub struct EnterpriseUpstreamService {
    inner: RwLock<Option<EnterpriseUpstream>>,
}

impl EnterpriseUpstreamService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the channel wholesale. Idempotent — the renderer pushes it on
    /// every sync cycle, so a repeated identical push must be a no-op.
    pub fn set(&self, upstream: EnterpriseUpstream) {
        if let Ok(mut guard) = self.inner.write() {
            *guard = Some(upstream);
        }
    }

    /// Forget the channel — revocation or a manual disconnect. After this the
    /// gate fails closed instead of talking to a server the member no longer
    /// belongs to.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.write() {
            *guard = None;
        }
    }

    pub fn get(&self) -> Option<EnterpriseUpstream> {
        self.inner.read().ok().and_then(|guard| guard.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(base: &str) -> EnterpriseUpstream {
        EnterpriseUpstream {
            base_url: base.to_owned(),
            token: "tok".to_owned(),
        }
    }

    #[test]
    fn starts_empty_and_roundtrips() {
        let svc = EnterpriseUpstreamService::new();
        assert!(svc.get().is_none(), "a client that never synced has no channel");

        svc.set(upstream("https://one.example.com"));
        assert_eq!(
            svc.get().as_ref().map(|u| u.base_url.as_str()),
            Some("https://one.example.com")
        );

        // A repeated push replaces, not appends.
        svc.set(upstream("https://other.example.com"));
        assert_eq!(
            svc.get().as_ref().map(|u| u.base_url.as_str()),
            Some("https://other.example.com")
        );
    }

    #[test]
    fn clear_forgets_and_allows_repush() {
        let svc = EnterpriseUpstreamService::new();
        svc.set(upstream("https://one.example.com"));
        svc.clear();
        assert!(svc.get().is_none(), "revocation must leave no channel behind");

        // A later login re-establishes it.
        svc.set(upstream("https://one.example.com"));
        assert!(svc.get().is_some());
    }
}
