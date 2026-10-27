//! Safety boundaries shared by persistence, selection and agent startup.

pub const MAX_USER_MCP_SERVERS: usize = 20;

/// These built-ins are owned by the application, never user quota entries.
pub fn is_system_mcp(name: &str) -> bool {
    matches!(
        name,
        "one-image-generation"
            | "aionui-image-generation" // legacy persisted media name
            | "AionUi Image Generation" // legacy persisted media name
            | "builtin-image-gen"
            | "one-web-search"
            | "builtin-web-search"
            | "one-browser"
            | "aionui-browser" // legacy persisted browser name
            | "one-page-reader"
            | "one-html-render"
            | "dream-team"
    )
}

/// Prefix `employee_runtime` gives the connectors a digital employee binds.
pub const EMPLOYEE_BOUND_MCP_PREFIX: &str = "enterprise_";

/// Connectors injected for a digital employee run; counted in their own bucket,
/// not against the member's 20 selected connectors.
pub fn is_employee_bound_mcp(name: &str) -> bool {
    name.starts_with(EMPLOYEE_BOUND_MCP_PREFIX)
}

/// A browser-authorizing proxy must never be spawned by unattended discovery.
/// Match package/executable tokens, including pinned npm versions and Windows paths.
pub fn is_browser_auth_proxy(command: &str, args: &[String]) -> bool {
    std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .any(|token| {
            let token = token.trim_matches(['\'', '"']).replace('\\', "/");
            token.split(['/', ' ', '\t']).any(|part| {
                part == "mcp-remote"
                    || part.starts_with("mcp-remote@")
                    || matches!(part, "mcp-remote.cmd" | "mcp-remote.exe")
            })
        })
}

/// Legacy implicit full-catalog snapshots are unsafe. Clear their user selection
/// as a whole, retaining messages and system tools; never choose an arbitrary 20.
pub fn repair_oversized_mcp_snapshot(extra: &mut serde_json::Value) -> bool {
    let Some(obj) = extra.as_object_mut() else { return false };
    let ids = obj.get("mcp_server_ids").and_then(|v| v.as_array());
    let names = obj.get("mcp_servers").and_then(|v| v.as_array());
    let mut unique = std::collections::HashSet::new();
    for id in ids.into_iter().flatten().filter_map(|v| v.as_str()) {
        unique.insert(id);
    }
    // Inline entries are separate processes whatever id they claim (missing or
    // repeated ids must not merge them), so count each one.
    let inline_count = obj
        .get("session_mcp_servers")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|item| {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
            !is_system_mcp(name) && !is_employee_bound_mcp(name)
        })
        .count();
    let names_count = names.map_or(0, |items| {
        items
            .iter()
            .filter(|v| !is_system_mcp(v.as_str().unwrap_or("")))
            .count()
    });
    let count = (unique.len().max(ids.map_or(0, Vec::len)) + inline_count).max(names_count);
    if count <= MAX_USER_MCP_SERVERS {
        return false;
    }
    for key in [
        "mcp_server_ids",
        "mcp_servers",
        "mcp_statuses",
        "selected_mcp_server_ids",
    ] {
        obj.insert(key.into(), serde_json::json!([]));
    }
    for key in ["session_mcp_servers", "selected_session_mcp_servers"] {
        if let Some(items) = obj.get_mut(key).and_then(|v| v.as_array_mut()) {
            items.retain(|v| {
                let name = v.get("name").and_then(|v| v.as_str()).unwrap_or("");
                is_system_mcp(name) || is_employee_bound_mcp(name)
            });
        }
    }
    obj.insert("mcp_selection_required".into(), serde_json::json!(true));
    obj.insert("mcp_selection_limit".into(), serde_json::json!(MAX_USER_MCP_SERVERS));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_legacy_catalog_without_losing_history_or_selecting_random_tools() {
        let mut value = serde_json::json!({"mcp_server_ids": (0..2692).map(|i| i.to_string()).collect::<Vec<_>>(), "workspace":"keep", "skills":["keep"], "session_mcp_servers":[{"name":"one-browser"},{"name":"arbitrary"}]});
        assert!(repair_oversized_mcp_snapshot(&mut value));
        assert_eq!(value["mcp_server_ids"], serde_json::json!([]));
        assert_eq!(value["workspace"], "keep");
        assert_eq!(
            value["session_mcp_servers"],
            serde_json::json!([{"name":"one-browser"}])
        );
        assert!(!repair_oversized_mcp_snapshot(&mut value));
    }

    #[test]
    fn repairs_inline_snapshots_whose_servers_share_or_lack_ids() {
        let inline: Vec<_> = (0..21)
            .map(|i| serde_json::json!({"name": format!("srv-{i}"), "id": ""}))
            .collect();
        let mut value = serde_json::json!({"session_mcp_servers": inline});
        assert!(repair_oversized_mcp_snapshot(&mut value));
        assert_eq!(value["session_mcp_servers"], serde_json::json!([]));
        assert_eq!(value["mcp_selection_required"], true);

        let no_id: Vec<_> = (0..21).map(|i| serde_json::json!({"name": format!("s{i}")})).collect();
        let mut value = serde_json::json!({"session_mcp_servers": no_id});
        assert!(repair_oversized_mcp_snapshot(&mut value));
    }

    #[test]
    fn boundary_twenty_is_preserved() {
        let mut value = serde_json::json!({"mcp_server_ids": (0..20).collect::<Vec<_>>()});
        assert!(!repair_oversized_mcp_snapshot(&mut value));
    }

    #[test]
    fn detects_pinned_and_resolved_proxies_without_blocking_other_packages() {
        assert!(is_browser_auth_proxy("npx", &["-y".into(), "mcp-remote@0.1.0".into()]));
        assert!(is_browser_auth_proxy(r"C:\npm\mcp-remote.cmd", &[]));
        assert!(!is_browser_auth_proxy("npx", &["my-mcp-remote-tools".into()]));
    }
}
