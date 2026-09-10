//! Who is asking, for the purposes of runtime-node blocking (C1-2).
//!
//! Blocking a runtime node has to hold against the machine it names, and it
//! has to not lock an administrator out of the console they would undo it
//! from. Those two callers reach the same governance endpoints and are told
//! apart here, once, rather than by five domain crates each re-deriving it.

use axum::http::HeaderMap;

/// The header a desktop client self-reports its machine id in, sent alongside
/// the Bearer token on every remote governance request.
pub const MACHINE_ID_HEADER: &str = "x-dream-machine-id";

/// What a governance request can say about the machine it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernanceCaller<'a> {
    /// A desktop client that named its machine. The roster decides.
    Identified(&'a str),
    /// A desktop client that did not.
    ///
    /// This used to be treated the same as the console below — waved through —
    /// which made the block advisory rather than enforcing: it held only for
    /// as long as the client chose to identify itself. The real client omits
    /// the header whenever its own machine-id lookup times out, and anyone
    /// reusing the same bearer token by hand never sent it at all.
    UnidentifiedRemoteClient,
    /// The admin console, or the WebUI: a same-origin session cookie and no
    /// bearer token.
    ///
    /// It never sends a machine id and never should be judged on one. An
    /// administrator whose own laptop is blocked must still be able to open
    /// the console and unblock it — a block that can lock you out of the page
    /// that undoes it is a trap, not a control.
    BrowserSession,
}

/// Classify a governance request.
///
/// The discriminator is the bearer token: the desktop client authenticates
/// with `Authorization: Bearer` on every remote governance call, while the
/// console's auth "is the same-origin session cookie, the whole story". So a
/// bearer with no machine id is a client that should have identified itself
/// and did not, and no bearer at all is a browser.
pub fn classify_governance_caller(headers: &HeaderMap) -> GovernanceCaller<'_> {
    if let Some(machine_id) = headers
        .get(MACHINE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return GovernanceCaller::Identified(machine_id);
    }
    if headers.contains_key(axum::http::header::AUTHORIZATION) {
        return GovernanceCaller::UnidentifiedRemoteClient;
    }
    GovernanceCaller::BrowserSession
}

#[cfg(test)]
#[path = "governance_caller_test.rs"]
mod governance_caller_test;
