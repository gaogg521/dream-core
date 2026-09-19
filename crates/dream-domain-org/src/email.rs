//! Invite-email sending (P2-4 onboarding).
//!
//! Default path uses the saved SMTP config via `lettre`. When SMTP is disabled
//! or has no host, `StubEmailSender` reports `not_configured`.

use std::time::Duration;

use async_trait::async_trait;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

/// Outcome of an invite-email send attempt, shaped like
/// `dream_domain_billing::CheckoutResultDto`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendEmailResult {
    /// `"not_configured"` | `"sent"` | `"error"`.
    pub status: String,
    pub message: String,
}

#[async_trait]
pub trait EmailSender: Send + Sync {
    /// Send an invite email. `to` is the recipient address; `invite_code` is
    /// the display-formatted code; `tenant_name` names the project group.
    async fn send_invite(&self, to: &str, invite_code: &str, tenant_name: &str) -> SendEmailResult;
}

/// No SMTP host/enabled: every send reports that email delivery is not configured.
pub struct StubEmailSender;

#[async_trait]
impl EmailSender for StubEmailSender {
    async fn send_invite(&self, _to: &str, _invite_code: &str, _tenant_name: &str) -> SendEmailResult {
        SendEmailResult {
            status: "not_configured".to_owned(),
            message: "SMTP is not configured. Save and enable an SMTP host, or share the invite code directly."
                .to_owned(),
        }
    }
}

/// Deliver through the operator's SMTP relay (`lettre`, STARTTLS on 587 / implicit TLS on 465).
#[allow(clippy::too_many_arguments)]
pub async fn send_invite_via_smtp(
    host: &str,
    port: u16,
    username: Option<&str>,
    password: Option<&str>,
    from: &str,
    to: &str,
    invite_code: &str,
    tenant_name: &str,
) -> SendEmailResult {
    let from_mb: Mailbox = match from.parse() {
        Ok(m) => m,
        Err(_) => {
            return SendEmailResult {
                status: "error".to_owned(),
                message: format!("Invalid From address: {from}"),
            };
        }
    };
    let to_mb: Mailbox = match to.parse() {
        Ok(m) => m,
        Err(_) => {
            return SendEmailResult {
                status: "error".to_owned(),
                message: format!("Invalid recipient: {to}"),
            };
        }
    };
    let email = match Message::builder()
        .from(from_mb)
        .to(to_mb)
        .subject(format!("Invitation to join {tenant_name}"))
        .body(format!(
            "You have been invited to {tenant_name}.\n\nInvite code: {invite_code}\n"
        )) {
        Ok(e) => e,
        Err(e) => {
            return SendEmailResult {
                status: "error".to_owned(),
                message: e.to_string(),
            };
        }
    };
    let transport = match build_transport(host, port, username, password) {
        Ok(t) => t,
        Err(e) => {
            return SendEmailResult {
                status: "error".to_owned(),
                message: e,
            };
        }
    };
    match transport.send(email).await {
        Ok(_) => SendEmailResult {
            status: "sent".to_owned(),
            message: format!("Invite email sent to {to}"),
        },
        Err(e) => SendEmailResult {
            status: "error".to_owned(),
            message: e.to_string(),
        },
    }
}

fn build_transport(
    host: &str,
    port: u16,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let mut builder = if port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(host).map_err(|e| e.to_string())?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host).map_err(|e| e.to_string())?
    };
    builder = builder.port(port).timeout(Some(Duration::from_secs(8)));
    if let (Some(u), Some(p)) = (username, password)
        && !u.is_empty()
        && !p.is_empty()
    {
        builder = builder.credentials(Credentials::new(u.to_owned(), p.to_owned()));
    }
    Ok(builder.build())
}
