//! Shared HTTP reachability probe for platform adapters (container, collab, SIEM).

use std::time::Duration;

/// Probe an admin-configured endpoint. Empty/missing → `not_configured`.
/// Unix sockets cannot be HTTP-probed. HTTP 2xx/401/403/404 counts as reachable.
pub async fn probe_http_endpoint(endpoint: Option<&str>, secret: Option<&str>) -> (String, String) {
    let Some(raw) = endpoint.map(str::trim).filter(|s| !s.is_empty()) else {
        return (
            "not_configured".to_owned(),
            "No endpoint configured. Save an HTTP URL and probe again.".to_owned(),
        );
    };
    if raw.starts_with("unix:") || raw.contains(".sock") || raw.starts_with("npipe:") {
        return (
            "error".to_owned(),
            "Unix/named-pipe sockets cannot be HTTP-probed. Use a TCP Docker API such as http://127.0.0.1:2375."
                .to_owned(),
        );
    }
    let url = normalize_probe_url(raw);
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(5)).build() {
        Ok(c) => c,
        Err(e) => return ("error".to_owned(), e.to_string()),
    };
    let mut req = client.get(&url);
    if let Some(token) = secret.map(str::trim).filter(|s| !s.is_empty()) {
        req = req.bearer_auth(token);
    }
    match req.send().await {
        Ok(resp) => {
            let code = resp.status().as_u16();
            if resp.status().is_success() || matches!(code, 401 | 403 | 404) {
                ("ok".to_owned(), format!("Reached {url} (HTTP {code})"))
            } else {
                ("error".to_owned(), format!("{url} returned HTTP {code}"))
            }
        }
        Err(e) => ("error".to_owned(), format!("Could not reach {url}: {e}")),
    }
}

fn normalize_probe_url(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = raw.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    }
}

/// TCP connect to `host[:port]` (default port supplied by the caller).
pub fn probe_tcp_host(endpoint: Option<&str>, default_port: u16) -> (String, String) {
    let Some(raw) = endpoint.map(str::trim).filter(|s| !s.is_empty()) else {
        return (
            "not_configured".to_owned(),
            "No endpoint configured.".to_owned(),
        );
    };
    let stripped = raw
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(raw)
        .trim_end_matches('/');
    let (host, port) = match stripped.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, p.parse().unwrap_or(default_port)),
        _ => (stripped, default_port),
    };
    use std::net::ToSocketAddrs;
    let addrs = match (host, port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(e) => return ("error".to_owned(), format!("DNS failed for {host}:{port}: {e}")),
    };
    let Some(addr) = addrs.first().copied() else {
        return ("error".to_owned(), format!("No address for {host}:{port}"));
    };
    match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
        Ok(_) => ("ok".to_owned(), format!("TCP reachable at {host}:{port}")),
        Err(e) => ("error".to_owned(), format!("TCP {host}:{port}: {e}")),
    }
}
