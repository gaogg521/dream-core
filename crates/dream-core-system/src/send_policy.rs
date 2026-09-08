//! Local enforcement of the two company limits that govern *sending*.
//!
//! Sibling of [`crate::tool_security`] and for the same reason: the desktop
//! client's conversations run on a personal build, where `EnterpriseSendGate`
//! is not compiled in. So a company could set "one message a minute" and watch
//! a member send ten, and could restrict the fleet to two approved models and
//! watch a member pick any model they liked — both were configured on the
//! server, delivered to the client, and enforced by nobody.
//!
//! Two dimensions, both pure functions of state the member is already allowed
//! to read:
//!
//! - **Send rate.** `one_security_policy.send_rate_limit_per_minute`, counted
//!   in a fixed per-user window exactly as the server counts it — a member who
//!   moves between the desktop client and the WebUI must not find that the
//!   limit means two different things.
//! - **Model allowlist.** The license's `allowed_models`. Empty means
//!   unrestricted, which is what an administrator who never set one means.
//!
//! # What this deliberately does not carry
//!
//! The **spend cap** stays on the server. A budget is a running total across
//! everyone on the licence, and a per-machine copy of it would either be stale
//! (each client seeing only its own spend) or would need a live figure pushed
//! on every turn from every member's machine. The company channel already
//! meters what it proxies, which is where the cap is actually enforceable.
//! Better an honest server-side cap than a local one that quietly permits
//! `n` machines to spend the budget `n` times.

use std::sync::RwLock;

use dashmap::DashMap;
use dream_core_common::now_ms;
use serde::{Deserialize, Serialize};

const DEFAULT_WINDOW_MS: i64 = 60_000;

/// The send-time half of the company policy, as synced onto this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendPolicy {
    /// `None` — the default, and every tier that does not set one — is
    /// unlimited.
    #[serde(default)]
    pub send_rate_limit_per_minute: Option<u32>,
    /// Empty is unrestricted. A non-empty list is exhaustive.
    #[serde(default)]
    pub allowed_models: Vec<String>,
}

impl SendPolicy {
    pub fn is_permissive(&self) -> bool {
        self.send_rate_limit_per_minute.is_none() && self.allowed_models.is_empty()
    }
}

#[derive(Debug)]
pub struct SendPolicyService {
    policy: RwLock<SendPolicy>,
    /// `user_id -> (sends so far, when the window ends)`.
    counts: DashMap<String, (u32, i64)>,
    window_ms: i64,
}

impl Default for SendPolicyService {
    fn default() -> Self {
        Self::with_window_ms(DEFAULT_WINDOW_MS)
    }
}

impl SendPolicyService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test-only seam: a 60-second window cannot be exercised end to end, so
    /// tests build a millisecond-scale one. Same technique the server-side
    /// limiter uses.
    pub fn with_window_ms(window_ms: i64) -> Self {
        Self {
            policy: RwLock::new(SendPolicy::default()),
            counts: DashMap::new(),
            window_ms,
        }
    }

    /// Replace wholesale — an all-default policy is how an administrator turns
    /// enforcement off, and a merge could not express that.
    pub fn set_policy(&self, policy: SendPolicy) {
        if let Ok(mut guard) = self.policy.write() {
            *guard = policy;
        }
    }

    pub fn policy(&self) -> SendPolicy {
        self.policy.read().map(|guard| guard.clone()).unwrap_or_default()
    }

    /// Records a send and returns `Some(reason)` if it exceeded the limit.
    ///
    /// Counting happens only when a limit exists, so an unsynced or
    /// unrestricted install never grows the map.
    pub fn check_send(&self, user_id: &str) -> Option<String> {
        let limit = self.policy().send_rate_limit_per_minute?;
        // A limit of zero is "nobody may send", and honouring it literally is
        // more likely to be a misconfiguration than an intent — but refusing to
        // honour it would be this process overriding an administrator. Honour
        // it; the message says what happened.
        let now = now_ms();
        let mut entry = self
            .counts
            .entry(user_id.to_owned())
            .or_insert((0, now + self.window_ms));
        if now >= entry.1 {
            entry.0 = 0;
            entry.1 = now + self.window_ms;
        }
        if entry.0 >= limit {
            return Some(format!(
                "blocked by company security policy (send rate limit: {limit} per minute)"
            ));
        }
        entry.0 += 1;
        None
    }

    /// `Some(reason)` if this model is outside the company's list.
    pub fn check_model(&self, model: &str) -> Option<String> {
        let policy = self.policy();
        if policy.allowed_models.is_empty() {
            return None;
        }
        let model = model.trim();
        // An empty model name is "whatever the conversation already had", not a
        // choice the member made — the server's own check skips it too.
        if model.is_empty() || policy.allowed_models.iter().any(|m| m == model) {
            return None;
        }
        Some(format!(
            "blocked by company policy: '{model}' is not one of the models your administrator approved"
        ))
    }
}

#[cfg(test)]
#[path = "send_policy_test.rs"]
mod send_policy_test;
