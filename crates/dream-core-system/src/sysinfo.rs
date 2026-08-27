use dream_core_api_types::SystemInfoResponse;

/// Map Rust `std::env::consts::OS` to the Node.js-compatible platform name
/// used by the API contract.
fn map_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other, // "linux" stays "linux"
    }
}

/// Map Rust `std::env::consts::ARCH` to the API contract arch name.
fn map_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Brand-named subdirectory of `parent`, preferring the current name but
/// staying on the pre-rebrand one while that is what exists on disk.
fn brand_dir(parent: &std::path::Path, current: &str) -> std::path::PathBuf {
    dream_core_common::resolve_with_legacy(parent, current, "aionui")
}

/// Resolve the cache directory.
///
/// Priority: `ONE_CACHE_DIR` env → `dirs::cache_dir()/one`, falling back to the
/// pre-rebrand directory when that is the one already on disk. This doc comment
/// used to claim `/dream` while the code joined `aionui`: the rebrand updated
/// the comment and left the literal, which is why a running backend still
/// still reported a cache path under the pre-rebrand name long afterwards.
fn resolve_cache_dir() -> String {
    if let Ok(v) = std::env::var("ONE_CACHE_DIR")
        && !v.is_empty()
    {
        return v;
    }
    dirs::cache_dir()
        .map(|p| brand_dir(&p, "one").to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Resolve the work (data) directory for DreamUI.
///
/// Priority: `ONE_WORK_DIR` env → `dirs::data_dir()/one`, with the same
/// pre-rebrand fallback as the cache directory.
fn resolve_work_dir() -> String {
    if let Ok(v) = std::env::var("ONE_WORK_DIR")
        && !v.is_empty()
    {
        return v;
    }
    dirs::data_dir()
        .map(|p| brand_dir(&p, "one").to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Resolve the log directory for DreamUI.
///
/// Priority: `ONE_LOG_DIR` env →
///   macOS: `~/Library/Logs/one`
///   Linux: `dirs::state_dir()/one/logs` (XDG_STATE_HOME)
///   Windows: `dirs::data_dir()/one/logs`
///
/// Each keeps its pre-rebrand sibling when that directory is the one that
/// exists, so an install's log history stays in one place — the diagnose skill
/// tails whatever this returns.
fn resolve_log_dir() -> String {
    if let Ok(v) = std::env::var("ONE_LOG_DIR")
        && !v.is_empty()
    {
        return v;
    }
    // macOS: ~/Library/Logs is the conventional log location
    if cfg!(target_os = "macos")
        && let Some(home) = dirs::home_dir()
    {
        return brand_dir(&home.join("Library/Logs"), "one")
            .to_string_lossy()
            .into_owned();
    }
    // Linux: XDG state dir
    if let Some(state) = dirs::state_dir() {
        return brand_dir(&state, "one").join("logs").to_string_lossy().into_owned();
    }
    // Fallback: data_dir/dream/logs
    dirs::data_dir()
        .map(|p| brand_dir(&p, "one").join("logs").to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Build the system info response from the current runtime environment.
pub fn get_system_info() -> SystemInfoResponse {
    SystemInfoResponse {
        cache_dir: resolve_cache_dir(),
        work_dir: resolve_work_dir(),
        log_dir: resolve_log_dir(),
        platform: map_platform().to_owned(),
        arch: map_arch().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_platform_known() {
        let p = map_platform();
        // On CI this will be one of the known values
        assert!(["darwin", "win32", "linux"].contains(&p), "unexpected platform: {p}");
    }

    #[test]
    fn test_map_arch_known() {
        let a = map_arch();
        assert!(["x64", "arm64"].contains(&a), "unexpected arch: {a}");
    }

    #[test]
    fn test_get_system_info_fields_non_empty() {
        let info = get_system_info();
        assert!(!info.cache_dir.is_empty(), "cache_dir should not be empty");
        assert!(!info.work_dir.is_empty(), "work_dir should not be empty");
        assert!(!info.log_dir.is_empty(), "log_dir should not be empty");
        assert!(!info.platform.is_empty());
        assert!(!info.arch.is_empty());
    }

    /// Asserting the literal `aionui` here is what let the hardcoded path
    /// survive the rebrand — the test agreed with the bug. What actually
    /// matters is that a directory is resolved under one of the two brands.
    #[test]
    fn defaults_resolve_to_a_brand_directory() {
        for dir in [resolve_cache_dir(), resolve_work_dir(), resolve_log_dir()] {
            let lower = dir.to_ascii_lowercase();
            assert!(
                lower.contains("one") || lower.contains(concat!("aion", "ui")),
                "unexpected directory: {dir}"
            );
        }
    }

    /// A fresh machine has neither directory, so a new install gets the current
    /// name rather than being handed the pre-rebrand one.
    #[test]
    fn a_missing_legacy_directory_is_not_chosen() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(brand_dir(empty.path(), "one"), empty.path().join("one"));
    }

    /// An install that already has the pre-rebrand directory keeps it, or its
    /// cache and log history are orphaned in place.
    #[test]
    fn an_existing_legacy_directory_wins() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = concat!("aion", "ui");
        std::fs::create_dir(dir.path().join(legacy)).unwrap();
        assert_eq!(brand_dir(dir.path(), "one"), dir.path().join(legacy));
    }
}
