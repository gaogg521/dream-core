//! Adopting the pre-rebrand `AIONUI_*` environment variables.
//!
//! Every environment variable this backend reads moved from `AIONUI_*` to
//! `ONE_*`. Most of them are operator-facing escape hatches — data and log
//! directories, host and port, timeouts, asset path overrides — which means
//! there are launch scripts, service units and shell profiles out there setting
//! the old names. A rename with no path back would not fail loudly for them: the
//! backend would simply fall back to its defaults and come up pointed at the
//! wrong directory.
//!
//! Rather than teaching ~50 individual read sites a fallback (and half of them
//! are `clap` `#[arg(env = ...)]` attributes, which take exactly one name), this
//! copies the old value into the new name once, before anything reads either.
//! Every read site can then use the current name and nothing else.
//!
//! The new name always wins: an operator who has set both is mid-migration, and
//! silently preferring the value they are moving away from would be the wrong
//! way to break the tie.

/// Suffixes shared by both spellings — `AIONUI_<S>` is adopted as `ONE_<S>`.
///
/// Deliberately explicit rather than derived: this list is what an operator's
/// old configuration is matched against, so a name missing here fails silently
/// (their setting is ignored) rather than loudly. Keep it in step with the
/// `ONE_*` names actually read across the workspace.
pub const ADOPTED_ENV_SUFFIXES: &[&str] = &[
    // Server / process
    "HOST",
    "PORT",
    "HTTPS",
    "ADMIN_PORT",
    "DATA_DIR",
    "WORK_DIR",
    "CACHE_DIR",
    "LOG_DIR",
    "LOG_LEVEL",
    "LOG_JSON",
    // Conversation runtime context handed to agent sessions
    "USER_ID",
    "CONVERSATION_ID",
    "HELPER_BIN",
    "BASE_URL",
    "RUNTIME_TOKEN",
    // Bundled asset overrides
    "BUILTIN_ASSISTANTS_PATH",
    "BUILTIN_SKILLS_PATH",
    "EXTENSIONS_PATH",
    "EXTENSION_STATES_FILE",
    "BUNDLED_MANAGED_RESOURCES",
    // Agent / protocol tuning
    "ACP_INIT_TIMEOUT_SECS",
    "HANDSHAKE_TIMEOUT_SECS",
    "IDLE_TIMEOUT_SECS",
    "IDLE_SCAN_INTERVAL_SECS",
    "TEAM_IDLE_TIMEOUT_SECS",
    "BYPASS_PROBE",
    "CLAUDE_WIRE_DUMP",
    // Integrations
    "FEISHU_BASE_URL",
    "GITHUB_REPO",
    // Built-in media generation
    "IMG_API_KEY",
    "IMG_API_URL",
    "IMG_MODEL",
    "IMG_QUALITY",
    "IMG_SIZE",
    "IMG_STYLE",
    // Antigravity hook
    "ANTIGRAVITY_HOOK_BASE_URL",
    "ANTIGRAVITY_HOOK_CONVERSATION_ID",
    "ANTIGRAVITY_HOOK_TOKEN",
];

/// Copy any `AIONUI_<S>` whose `ONE_<S>` is unset.
///
/// # Safety
///
/// Calls `std::env::set_var`, which is unsound once other threads are running.
/// This must be the first thing `main` does — before the async runtime starts,
/// before `clap` parses, before anything reads an environment variable.
pub unsafe fn adopt_legacy_env() {
    for suffix in ADOPTED_ENV_SUFFIXES {
        let current = format!("ONE_{suffix}");
        if std::env::var_os(&current).is_some() {
            continue;
        }
        if let Some(value) = std::env::var_os(format!("AIONUI_{suffix}")) {
            // SAFETY: caller contract — single-threaded, before any reader.
            unsafe { std::env::set_var(&current, value) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Built at runtime so a future sweep over quoted `AIONUI_*` literals cannot
    // rewrite these into the current name — which would leave both tests setting
    // and asserting the same variable, passing while proving nothing.
    const LEGACY_PREFIX: &str = "AIONUI";
    static LEGACY_DATA_DIR: &str = concat!("AIONUI", "_DATA_DIR");
    static LEGACY_LOG_LEVEL: &str = concat!("AIONUI", "_LOG_LEVEL");

    #[test]
    fn the_legacy_prefix_is_what_operators_actually_set() {
        assert_eq!(LEGACY_PREFIX, "AIONUI");
        assert_eq!(LEGACY_DATA_DIR, "AIONUI_DATA_DIR");
    }

    /// The list is what an operator's existing configuration is matched
    /// against, so an entry that does not round-trip would drop their setting
    /// without a word.
    #[test]
    fn every_suffix_maps_between_the_two_spellings() {
        for suffix in ADOPTED_ENV_SUFFIXES {
            assert!(!suffix.is_empty());
            assert!(
                !suffix.starts_with('_') && !suffix.starts_with("ONE_") && !suffix.starts_with("AIONUI_"),
                "{suffix} should be the bare suffix, not a full variable name"
            );
        }
    }

    #[test]
    fn the_list_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for suffix in ADOPTED_ENV_SUFFIXES {
            assert!(seen.insert(*suffix), "duplicate suffix {suffix}");
        }
    }

    /// Covers the operator who set the old name and nothing else — the whole
    /// reason this module exists.
    #[test]
    fn a_legacy_value_is_adopted_under_the_current_name() {
        unsafe {
            std::env::remove_var("ONE_DATA_DIR");
            std::env::set_var(LEGACY_DATA_DIR, "/legacy/data");
            adopt_legacy_env();
        }
        assert_eq!(std::env::var("ONE_DATA_DIR").unwrap(), "/legacy/data");
        unsafe {
            std::env::remove_var("ONE_DATA_DIR");
            std::env::remove_var(LEGACY_DATA_DIR);
        }
    }

    /// An operator part-way through the migration has both set; the one they
    /// are moving toward must win.
    #[test]
    fn the_current_name_is_never_overwritten() {
        unsafe {
            std::env::set_var("ONE_LOG_LEVEL", "debug");
            std::env::set_var(LEGACY_LOG_LEVEL, "trace");
            adopt_legacy_env();
        }
        assert_eq!(std::env::var("ONE_LOG_LEVEL").unwrap(), "debug");
        unsafe {
            std::env::remove_var("ONE_LOG_LEVEL");
            std::env::remove_var(LEGACY_LOG_LEVEL);
        }
    }
}
