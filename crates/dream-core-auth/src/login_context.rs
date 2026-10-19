//! Bounded, non-sensitive request metadata for successful authentication.
use axum::http::HeaderMap;

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

pub fn login_request_metadata(headers: &HeaderMap) -> (Option<String>, Option<String>) {
    let ip = crate::extract_client_ip_from_headers(headers)
        .parse::<std::net::IpAddr>()
        .ok()
        .map(|ip| ip.to_string());
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
    #[test]
    fn validates_ip_and_bounds_user_agent_without_recording_credentials() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "192.0.2.9, 10.0.0.1".parse().unwrap());
        headers.insert("user-agent", "x".repeat(900).parse().unwrap());
        headers.insert("authorization", "Bearer secret".parse().unwrap());
        let (ip, agent) = login_request_metadata(&headers);
        assert_eq!(ip.as_deref(), Some("192.0.2.9"));
        assert_eq!(agent.unwrap().len(), 512);
        headers.insert("x-forwarded-for", "invalid".parse().unwrap());
        assert_eq!(login_request_metadata(&headers).0, None);
    }
}
