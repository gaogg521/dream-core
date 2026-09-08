//! Local enforcement of the company's tool-call security policy.
//!
//! # Why this exists in the personal-edition crate
//!
//! An enterprise member on the desktop client runs their conversations on the
//! co-located backend, which is a personal build: every `#[cfg(feature =
//! "enterprise")]` gate — including the platform one that reads
//! `one_security_policy` — is not merely unused there, it is not compiled in.
//! So an administrator could set the strict tier, watch the member's client
//! receive all nine policy fields, and still have the agent run `rm -rf` and
//! reach the open internet, because nothing on that machine had ever heard of
//! the policy.
//!
//! Content inspection already solved the same problem the same way (see
//! `content_inspection.rs`): the decision has to be made where the tool call
//! is made, so the policy comes *down* and is evaluated locally. This is that
//! pattern applied to the other half of the policy.
//!
//! # What this does and does not cover
//!
//! Two of the three tool-call dimensions are pure functions of the policy and
//! are enforced here: the destructive-command block and the default-deny on
//! external network access.
//!
//! Terminal-tool **approval** is the third. It cannot be decided from a local
//! copy alone — it means "block until a human on the admin console decides",
//! which needs a round trip to the company server. What makes it enforceable
//! on a client is the enterprise upstream channel (`dream-core-app`'s
//! `enterprise_upstream`): the renderer pushes the server address and the
//! member's token here, and the gate holds the tool call while it drives the
//! company's own `/api/workflow/tasks` over that channel. The flag travels in
//! this policy so the gate knows the dimension is demanded; the *mechanism*
//! lives where the HTTP client lives. When the channel has not been synced
//! yet, the gate fails closed — the company asked for approval, so a silent
//! allow would be weaker than what the administrator configured.
//!
//! # What this is not
//!
//! Not a defence against a member who controls their own machine — the same
//! honest limit `content_inspection` states. It enforces the policy for
//! someone using the product as shipped, and that is the population the
//! policy is for.

use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// The subset of `one_security_policy` a client can act on by itself.
///
/// Deliberately not the whole DTO: fields the local backend cannot honour
/// (approval, the send-rate budget, message scanning — that last one is
/// content inspection's job and already delivered) are left out rather than
/// accepted and ignored, so a reader of this struct cannot mistake "stored
/// here" for "enforced here".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSecurityPolicy {
    /// Refuse commands containing any `blocked_command_patterns` entry.
    #[serde(default)]
    pub destructive_commands_blocked: bool,
    /// Case-insensitive substrings. Empty entries are ignored — an empty
    /// pattern matches everything, which is never what an administrator typing
    /// into a list meant.
    #[serde(default)]
    pub blocked_command_patterns: Vec<String>,
    /// Refuse tool calls that reach the network.
    #[serde(default)]
    pub external_network_denied_by_default: bool,
    /// Terminal tool calls must be approved by an administrator before they
    /// run (C0-1). Enforced by the gate through the enterprise upstream
    /// channel — a company server round trip, not a local decision.
    #[serde(default)]
    pub terminal_tools_require_approval: bool,
}

impl ToolSecurityPolicy {
    /// Whether this policy would demand anything at all.
    ///
    /// The default — every flag off — is the personal-edition posture, and the
    /// gate short-circuits on it so a standalone user pays nothing.
    pub fn is_permissive(&self) -> bool {
        !self.destructive_commands_blocked
            && !self.external_network_denied_by_default
            && !self.terminal_tools_require_approval
    }
}

/// Holds the distributed policy for this machine.
///
/// One instance per process, shared with the agent factory's gate. Starts
/// empty: a client that has never synced, or a standalone install that never
/// will, allows everything — the pre-existing behaviour.
#[derive(Debug, Default)]
pub struct ToolSecurityService {
    policy: RwLock<ToolSecurityPolicy>,
}

impl ToolSecurityService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the policy wholesale.
    ///
    /// Replace rather than merge, and an all-default policy is a legitimate
    /// instruction: an administrator relaxing the tier has to be able to
    /// actually turn enforcement off, and a merge would make relaxation
    /// impossible to express.
    pub fn set_policy(&self, policy: ToolSecurityPolicy) {
        if let Ok(mut guard) = self.policy.write() {
            *guard = policy;
        }
    }

    pub fn policy(&self) -> ToolSecurityPolicy {
        self.policy.read().map(|guard| guard.clone()).unwrap_or_default()
    }

    /// `None` to allow, `Some(reason)` to refuse.
    ///
    /// The reason is shown to the member in the transcript, so it names the
    /// pattern that matched: "blocked by company policy" with nothing else
    /// leaves someone re-running the same command hoping for a different
    /// answer.
    pub fn check(&self, command_text: &str, is_network_fetch: bool) -> Option<String> {
        let policy = self.policy();
        if policy.is_permissive() {
            return None;
        }

        if is_network_fetch && policy.external_network_denied_by_default {
            return Some("blocked by company security policy (external network access denied by default)".to_owned());
        }

        if policy.destructive_commands_blocked {
            let haystack = command_text.to_lowercase();
            if let Some(pattern) = policy
                .blocked_command_patterns
                .iter()
                .find(|pattern| !pattern.is_empty() && haystack.contains(&pattern.to_lowercase()))
            {
                return Some(format!("blocked by company security policy (matches '{pattern}')"));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict() -> ToolSecurityPolicy {
        ToolSecurityPolicy {
            destructive_commands_blocked: true,
            terminal_tools_require_approval: false,
            blocked_command_patterns: vec!["rm -rf".into(), "shutdown".into()],
            external_network_denied_by_default: true,
        }
    }

    #[test]
    fn an_unsynced_machine_allows_everything() {
        let svc = ToolSecurityService::new();
        assert_eq!(svc.check("rm -rf /", false), None);
        assert_eq!(svc.check("curl example.com", true), None);
    }

    #[test]
    fn a_blocked_pattern_is_refused_and_named() {
        let svc = ToolSecurityService::new();
        svc.set_policy(strict());
        let refusal = svc.check("sudo RM -RF /var", false).expect("must refuse");
        assert!(
            refusal.contains("rm -rf"),
            "the reason must name the pattern: {refusal}"
        );
    }

    #[test]
    fn matching_is_case_insensitive_and_ignores_empty_patterns() {
        let svc = ToolSecurityService::new();
        svc.set_policy(ToolSecurityPolicy {
            destructive_commands_blocked: true,
            terminal_tools_require_approval: false,
            // An empty entry would otherwise match every command ever run.
            blocked_command_patterns: vec![String::new(), "MKFS".into()],
            external_network_denied_by_default: false,
        });
        assert!(svc.check("mkfs.ext4 /dev/sda", false).is_some());
        assert_eq!(svc.check("echo hello", false), None);
    }

    #[test]
    fn network_calls_are_refused_only_when_the_policy_says_so() {
        let svc = ToolSecurityService::new();
        svc.set_policy(strict());
        assert!(svc.check("fetch", true).is_some());

        svc.set_policy(ToolSecurityPolicy {
            external_network_denied_by_default: false,
            ..strict()
        });
        assert_eq!(svc.check("fetch", true), None);
    }

    /// Relaxing the tier has to actually relax it. A merge-style update would
    /// leave the old flags standing and make "turn this off" unexpressible.
    #[test]
    fn relaxing_the_policy_turns_enforcement_off() {
        let svc = ToolSecurityService::new();
        svc.set_policy(strict());
        assert!(svc.check("rm -rf /", false).is_some());

        svc.set_policy(ToolSecurityPolicy::default());
        assert_eq!(svc.check("rm -rf /", false), None);
    }
}
