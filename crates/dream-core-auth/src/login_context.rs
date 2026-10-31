//! Bounded, non-sensitive request metadata for successful authentication.
use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;

use crate::middleware::{CLIENT_IP_HEADER, is_webui_proxied};

pub fn login_audit_event(row: dream_core_db::UserLoginEvent) -> dream_core_api_types::LoginAuditEvent {
    dream_core_api_types::LoginAuditEvent {
        id: row.id,
        user_id: row.user_id,
        method: row.method,
        client: row.client,
        ip: row.ip,
        user_agent: row.user_agent,
        created_at: row.created_at,
    }
}

/// Parse a single header value as an IP address (trimmed, format-checked).
fn parse_ip_header(headers: &HeaderMap, name: &str) -> Option<IpAddr> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<IpAddr>().ok())
}

/// Whether the address identifies a proxy sitting on the same machine or in
/// the on-prem network (loopback, RFC 1918, IPv4 link-local, IPv6 unique
/// local / link local). Such a peer means "the TCP connection came from a
/// reverse proxy", not from the end client.
fn peer_is_proxy(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local (fc00::/7)
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link local (fe80::/10)
        }
    }
}

/// Rightmost parseable entry of the `X-Forwarded-For` chain.
///
/// Every proxy hop appends the peer it saw to the end of the chain, so the
/// rightmost entry is the origin as judged by the proxy closest to this
/// service. Taking the rightmost (rather than the leftmost) entry is what
/// keeps a client-forged first entry (`X-Forwarded-For: 9.9.9.9` sent
/// verbatim) from masking the value the real reverse proxy appended.
fn last_forwarded_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|chain| {
            chain
                .split(',')
                .rev()
                .map(str::trim)
                .find_map(|entry| entry.parse::<IpAddr>().ok())
        })
}

/// Best-effort client-source resolution for audit-grade consumers (login
/// audit, rate-limit bucketing, device last-seen IP).
///
/// Trust model — same building blocks as the middleware's
/// `resolve_caller_ip`, with audit (not admission) semantics:
///
/// 1. Desktop WebUI reverse proxy: the connection is spliced over loopback
///    and the proxy overwrites the dedicated header unconditionally, so the
///    header is the truth.
/// 2. Socket peer is loopback/private: the request came through a local or
///    on-prem reverse proxy, and the peer only identifies that proxy. The
///    real origin can only come from headers — `X-Real-IP` (stamped by the
///    edge proxy) first, then the rightmost parseable `X-Forwarded-For`
///    entry; when neither resolves, record the peer as-is (the proxy's own
///    address) rather than nothing.
/// 3. Socket peer is public: a direct client, which may forge any header —
///    the peer wins outright and headers are ignored.
/// 4. No peer info (handlers invoked directly in tests, exotic runtimes):
///    fall back to header parsing alone.
pub fn audit_client_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
    if is_webui_proxied(headers) {
        return parse_ip_header(headers, CLIENT_IP_HEADER);
    }
    match peer {
        Some(peer) if peer_is_proxy(peer) => {
            Some(parse_ip_header(headers, "x-real-ip").or_else(|| last_forwarded_ip(headers)).unwrap_or(peer))
        }
        Some(peer) => Some(peer),
        None => parse_ip_header(headers, "x-real-ip").or_else(|| last_forwarded_ip(headers)),
    }
}

pub fn login_request_metadata(headers: &HeaderMap, peer: Option<SocketAddr>) -> (Option<String>, Option<String>) {
    let ip = audit_client_ip(headers, peer.map(|addr| addr.ip())).map(|ip| ip.to_string());
    let user_agent = headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().filter(|c| !c.is_control()).take(512).collect::<String>())
        .filter(|value| !value.is_empty());
    (ip, user_agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for &(name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn ip(text: &str) -> Option<IpAddr> {
        Some(text.parse().unwrap())
    }

    #[test]
    fn webui_proxy_header_wins_over_everything() {
        let headers = headers_with(&[
            ("x-dream-forwarded-origin", "webui"),
            ("x-dream-client-ip", "203.0.113.4"),
            ("x-real-ip", "203.0.113.5"),
            ("x-forwarded-for", "9.9.9.9"),
        ]);
        assert_eq!(audit_client_ip(&headers, ip("127.0.0.1")), ip("203.0.113.4"));
    }

    #[test]
    fn loopback_peer_without_headers_records_peer() {
        // 本地栈直连（无任何代理头）此前记录 NULL——现在如实记录对端。
        let headers = HeaderMap::new();
        assert_eq!(audit_client_ip(&headers, ip("127.0.0.1")), ip("127.0.0.1"));
    }

    #[test]
    fn proxied_peer_prefers_x_real_ip_then_rightmost_forwarded() {
        let headers = headers_with(&[("x-real-ip", "203.0.113.5")]);
        assert_eq!(audit_client_ip(&headers, ip("172.18.0.2")), ip("203.0.113.5"));
        let headers = headers_with(&[("x-forwarded-for", "9.9.9.9, 203.0.113.8")]);
        assert_eq!(audit_client_ip(&headers, ip("172.18.0.2")), ip("203.0.113.8"));
        let headers = headers_with(&[("x-forwarded-for", "203.0.113.8")]);
        assert_eq!(audit_client_ip(&headers, ip("172.18.0.2")), ip("203.0.113.8"));
        // 头都在但全是垃圾：回退对端，不崩溃也不记 NULL。
        let headers = headers_with(&[("x-real-ip", "garbage"), ("x-forwarded-for", "not-an-ip")]);
        assert_eq!(audit_client_ip(&headers, ip("172.18.0.2")), ip("172.18.0.2"));
    }

    #[test]
    fn public_peer_overrides_forgeable_headers() {
        // 公网直连客户端可伪造任意头：一律以 socket 对端为准。
        let headers = headers_with(&[
            ("x-real-ip", "9.9.9.9"),
            ("x-forwarded-for", "8.8.8.8, 9.9.9.9"),
        ]);
        assert_eq!(audit_client_ip(&headers, ip("203.0.113.99")), ip("203.0.113.99"));
    }

    #[test]
    fn private_lan_peer_without_headers_records_peer() {
        let headers = HeaderMap::new();
        assert_eq!(audit_client_ip(&headers, ip("192.168.1.20")), ip("192.168.1.20"));
    }

    #[test]
    fn without_peer_falls_back_to_headers_only() {
        let headers = headers_with(&[("x-real-ip", "198.51.100.5")]);
        assert_eq!(audit_client_ip(&headers, None), ip("198.51.100.5"));
        let headers = headers_with(&[("x-forwarded-for", "invalid")]);
        assert_eq!(audit_client_ip(&headers, None), None);
        assert_eq!(audit_client_ip(&HeaderMap::new(), None), None);
    }

    #[test]
    fn validates_ip_and_bounds_user_agent_without_recording_credentials() {
        let mut headers = headers_with(&[("x-forwarded-for", "192.0.2.9, 10.0.0.1")]);
        headers.insert("user-agent", "x".repeat(900).parse().unwrap());
        headers.insert("authorization", "Bearer secret".parse().unwrap());
        let (ip, agent) = login_request_metadata(&headers, None);
        // 最右可解析条目：最近一跳代理追加的 10.0.0.1，而非客户端可伪造的首条。
        assert_eq!(ip.as_deref(), Some("10.0.0.1"));
        assert_eq!(agent.unwrap().len(), 512);
        headers.insert("x-forwarded-for", "invalid".parse().unwrap());
        assert_eq!(login_request_metadata(&headers, None).0, None);
    }
}
