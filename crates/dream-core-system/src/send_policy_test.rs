use super::*;

#[test]
fn an_unsynced_machine_limits_nothing() {
    let svc = SendPolicyService::new();
    for _ in 0..50 {
        assert_eq!(svc.check_send("u1"), None);
    }
    assert_eq!(svc.check_model("anything-at-all"), None);
}

/// The case the audit reproduced: limit of one, two sends ten seconds apart.
#[test]
fn the_second_send_in_a_window_is_refused_when_the_limit_is_one() {
    let svc = SendPolicyService::new();
    svc.set_policy(SendPolicy {
        send_rate_limit_per_minute: Some(1),
        allowed_models: Vec::new(),
    });

    assert_eq!(svc.check_send("u1"), None);
    let refusal = svc.check_send("u1").expect("the second send must be refused");
    assert!(refusal.contains("1"), "the reason must name the limit: {refusal}");
}

#[test]
fn the_window_expiring_lets_sending_resume() {
    let svc = SendPolicyService::with_window_ms(30);
    svc.set_policy(SendPolicy {
        send_rate_limit_per_minute: Some(1),
        allowed_models: Vec::new(),
    });

    assert_eq!(svc.check_send("u1"), None);
    assert!(svc.check_send("u1").is_some());
    std::thread::sleep(std::time::Duration::from_millis(60));
    assert_eq!(svc.check_send("u1"), None, "a new window must start clean");
}

/// One member burning their allowance must not silence anyone else.
#[test]
fn the_limit_is_counted_per_member() {
    let svc = SendPolicyService::new();
    svc.set_policy(SendPolicy {
        send_rate_limit_per_minute: Some(1),
        allowed_models: Vec::new(),
    });

    assert_eq!(svc.check_send("u1"), None);
    assert!(svc.check_send("u1").is_some());
    assert_eq!(svc.check_send("u2"), None, "u2 has their own window");
}

#[test]
fn an_empty_allowlist_permits_every_model() {
    let svc = SendPolicyService::new();
    svc.set_policy(SendPolicy {
        send_rate_limit_per_minute: None,
        allowed_models: Vec::new(),
    });
    assert_eq!(svc.check_model("gpt-4"), None);
}

#[test]
fn a_model_outside_the_list_is_refused_and_named() {
    let svc = SendPolicyService::new();
    svc.set_policy(SendPolicy {
        send_rate_limit_per_minute: None,
        allowed_models: vec!["glm-flash-latest".into()],
    });

    assert_eq!(svc.check_model("glm-flash-latest"), None);
    assert_eq!(svc.check_model("  glm-flash-latest  "), None, "surrounding space");
    let refusal = svc.check_model("gpt-4").expect("must refuse");
    assert!(refusal.contains("gpt-4"), "{refusal}");
}

/// "No model named" means the conversation keeps whatever it had — not a
/// choice to police. The server's own check skips it, and diverging here would
/// break conversations that never name a model.
#[test]
fn an_unnamed_model_is_not_a_choice_to_refuse() {
    let svc = SendPolicyService::new();
    svc.set_policy(SendPolicy {
        send_rate_limit_per_minute: None,
        allowed_models: vec!["glm-flash-latest".into()],
    });
    assert_eq!(svc.check_model(""), None);
}
