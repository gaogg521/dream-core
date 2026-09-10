use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;

use super::{GovernanceCaller, MACHINE_ID_HEADER, classify_governance_caller};

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    h
}

#[test]
fn a_client_that_names_its_machine_is_judged_on_that_name() {
    let h = headers(&[("authorization", "Bearer t"), (MACHINE_ID_HEADER, "laptop-7")]);
    assert_eq!(classify_governance_caller(&h), GovernanceCaller::Identified("laptop-7"));
}

/// The bypass this classification exists to close: a bearer-token caller that
/// simply does not send the header. Treating it as a browser is what made
/// blocking a runtime node advisory.
#[test]
fn a_bearer_caller_without_a_machine_id_is_an_unidentified_client() {
    let h = headers(&[("authorization", "Bearer t")]);
    assert_eq!(
        classify_governance_caller(&h),
        GovernanceCaller::UnidentifiedRemoteClient
    );
}

/// The console, whose auth is the session cookie alone. It must stay outside
/// machine scoping entirely, or blocking a node could lock an administrator
/// out of the page that unblocks it.
#[test]
fn a_session_cookie_caller_is_a_browser() {
    let h = headers(&[("cookie", "dream-session=abc")]);
    assert_eq!(classify_governance_caller(&h), GovernanceCaller::BrowserSession);
    assert_eq!(
        classify_governance_caller(&HeaderMap::new()),
        GovernanceCaller::BrowserSession
    );
}

/// A header present but empty is no identification at all. Read as
/// `Identified("")` it would match no roster row and read as "not blocked",
/// which is the bypass again wearing a header.
#[test]
fn an_empty_or_blank_machine_id_does_not_count_as_identification() {
    for blank in ["", "   "] {
        let h = headers(&[("authorization", "Bearer t"), (MACHINE_ID_HEADER, blank)]);
        assert_eq!(
            classify_governance_caller(&h),
            GovernanceCaller::UnidentifiedRemoteClient,
            "blank machine id {blank:?} must not read as identified"
        );
    }
}

/// Without a bearer, a blank machine id is still just the console.
#[test]
fn a_browser_is_not_promoted_to_a_client_by_a_blank_header() {
    let h = headers(&[(MACHINE_ID_HEADER, "")]);
    assert_eq!(classify_governance_caller(&h), GovernanceCaller::BrowserSession);
}
