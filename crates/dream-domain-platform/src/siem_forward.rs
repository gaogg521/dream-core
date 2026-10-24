//! Forwarding the audit log to the company's SIEM.
//!
//! A company that saves a SIEM endpoint expects its audit events to arrive
//! there. This is the part that sends them: a ticker reads `one_audit_logs`
//! strictly after a stored cursor, delivers the batch (Splunk HEC, a generic
//! HTTP collector, or syslog over TCP), and moves the cursor only once the
//! receiver accepted it. A receiver that is down therefore loses nothing, and a
//! restart neither repeats nor skips events.
//!
//! Only events created after export was switched on are sent: turning it on
//! must not flood a SIEM with years of history.

use std::sync::Arc;
use std::time::Duration;

use dream_core_common::{decrypt_string, now_ms};
use dream_core_db::db_params;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use crate::error::PlatformError;
use crate::service::PlatformService;

const TICK: Duration = Duration::from_secs(30);
const BATCH: i64 = 500;
/// Batches one tick may send, so a long backlog drains without monopolising it.
const MAX_BATCHES_PER_TICK: usize = 10;
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
const ERROR_CHARS: usize = 500;

/// What one forwarding pass for one company did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiemForwardReport {
    /// `"ok"`, `"error"`, or `"idle"` (not enabled / not configured).
    pub status: String,
    pub forwarded: u64,
    pub message: String,
}

impl SiemForwardReport {
    fn idle(message: &str) -> Self {
        Self {
            status: "idle".into(),
            forwarded: 0,
            message: message.into(),
        }
    }
}

/// `id, user_id, username, action, resource, ip_address, user_agent, created_at, latency_ms, result`
type AuditRow = (
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
    Option<String>,
);

/// `kind, endpoint, secret_encrypted, updated_at`
type SiemTarget = (Option<String>, Option<String>, Option<String>, i64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Splunk,
    Http,
    Syslog,
}

impl Kind {
    fn parse(raw: Option<&str>) -> Option<Self> {
        match raw?.trim().to_ascii_lowercase().as_str() {
            "splunk" => Some(Self::Splunk),
            "http" => Some(Self::Http),
            "syslog" => Some(Self::Syslog),
            _ => None,
        }
    }
}

fn event_json(tenant_id: &str, row: &AuditRow) -> serde_json::Value {
    json!({
        "id": row.0,
        "tenantId": tenant_id,
        "userId": row.1,
        "username": row.2,
        "action": row.3,
        "resource": row.4,
        "ipAddress": row.5,
        "userAgent": row.6,
        "createdAt": row.7,
        "latencyMs": row.8,
        "result": row.9.as_deref().unwrap_or("success"),
    })
}

/// Splunk HEC takes concatenated `{time, event}` objects.
fn splunk_body(tenant_id: &str, rows: &[AuditRow]) -> String {
    rows.iter()
        .map(|row| {
            json!({
                "time": row.7 as f64 / 1000.0,
                "source": "one-work",
                "sourcetype": "_json",
                "event": event_json(tenant_id, row),
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ndjson_body(tenant_id: &str, rows: &[AuditRow]) -> String {
    rows.iter()
        .map(|row| event_json(tenant_id, row).to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// RFC 5424 lines, newline-framed (RFC 6587 non-transparent framing), facility
/// local0; a failed action is `warning`, everything else `info`.
fn syslog_body(tenant_id: &str, rows: &[AuditRow]) -> String {
    rows.iter()
        .map(|row| {
            let failed = row.9.as_deref().is_some_and(|r| r != "success");
            let priority = 16 * 8 + if failed { 4 } else { 6 };
            let seconds = row.7.div_euclid(1000);
            let millis = row.7.rem_euclid(1000);
            let days = seconds.div_euclid(86_400);
            let day_seconds = seconds.rem_euclid(86_400);
            let (year, month, day) = civil_from_days(days);
            // The body is a single line of JSON: no control characters survive
            // `to_string`, so a hostile resource name cannot split the record.
            format!(
                "<{priority}>1 {year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z one-work dreamcore - audit - {}\n",
                day_seconds / 3600,
                day_seconds % 3600 / 60,
                day_seconds % 60,
                event_json(tenant_id, row)
            )
        })
        .collect()
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn syslog_address(endpoint: &str) -> Result<String, String> {
    let rest = endpoint.split_once("://").map(|(_, r)| r).unwrap_or(endpoint);
    let rest = rest.trim().trim_end_matches('/');
    if rest.is_empty() {
        return Err("the syslog endpoint is empty".into());
    }
    Ok(match rest.rsplit_once(':') {
        Some((_, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => rest.to_owned(),
        _ => format!("{rest}:514"),
    })
}

fn clip(text: String) -> String {
    text.chars().take(ERROR_CHARS).collect()
}

async fn send(
    kind: Kind,
    endpoint: &str,
    secret: Option<&str>,
    tenant_id: &str,
    rows: &[AuditRow],
) -> Result<(), String> {
    match kind {
        Kind::Syslog => {
            let address = syslog_address(endpoint)?;
            let body = syslog_body(tenant_id, rows);
            let write = async {
                let mut stream = tokio::net::TcpStream::connect(&address).await?;
                stream.write_all(body.as_bytes()).await?;
                stream.flush().await?;
                stream.shutdown().await
            };
            tokio::time::timeout(SEND_TIMEOUT, write)
                .await
                .map_err(|_| format!("syslog {address} did not answer within {}s", SEND_TIMEOUT.as_secs()))?
                .map_err(|e| format!("syslog {address}: {e}"))
        }
        Kind::Splunk | Kind::Http => {
            let client = reqwest::Client::builder()
                .timeout(SEND_TIMEOUT)
                .build()
                .map_err(|e| e.to_string())?;
            let url = endpoint.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("the endpoint must start with http:// or https://".into());
            }
            let mut request = client.post(url);
            request = if kind == Kind::Splunk {
                let mut r = request
                    .header("content-type", "application/json")
                    .body(splunk_body(tenant_id, rows));
                if let Some(token) = secret.filter(|t| !t.is_empty()) {
                    r = r.header("authorization", format!("Splunk {token}"));
                }
                r
            } else {
                let mut r = request
                    .header("content-type", "application/x-ndjson")
                    .body(ndjson_body(tenant_id, rows));
                if let Some(token) = secret.filter(|t| !t.is_empty()) {
                    r = r.bearer_auth(token);
                }
                r
            };
            let response = request
                .send()
                .await
                .map_err(|e| format!("could not reach {url}: {e}"))?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(format!("{url} answered HTTP {}", response.status().as_u16()))
            }
        }
    }
}

impl PlatformService {
    /// Deliver the audit events a company has not yet sent to its SIEM.
    pub async fn forward_siem_once(&self, tenant_id: &str) -> Result<SiemForwardReport, PlatformError> {
        let target: Option<SiemTarget> = self
            .db
            .fetch_optional_as::<SiemTarget>(
                "SELECT kind, endpoint, secret_encrypted, updated_at FROM one_siem_config \
                 WHERE tenant_id = ? AND enabled = 1",
                &db_params![tenant_id],
            )
            .await?;
        let Some((kind, endpoint, secret_encrypted, enabled_at)) = target else {
            return Ok(SiemForwardReport::idle("export is not enabled"));
        };
        let Some(kind) = Kind::parse(kind.as_deref()) else {
            return Ok(SiemForwardReport::idle("choose Splunk, HTTP or syslog to export"));
        };
        let Some(endpoint) = endpoint.filter(|e| !e.trim().is_empty()) else {
            return Ok(SiemForwardReport::idle("no endpoint saved"));
        };
        let secret = secret_encrypted.and_then(|e| decrypt_string(&e, &self.encryption_key).ok());

        // Standalone installs have no audit table; nothing to send.
        let audit_present = self
            .db
            .fetch_one_scalar::<i64>(
                match self.db.backend() {
                    dream_core_db::DbBackend::Sqlite => {
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'one_audit_logs'"
                    }
                    dream_core_db::DbBackend::MySql => {
                        "SELECT COUNT(*) FROM information_schema.tables \
                         WHERE table_schema = DATABASE() AND table_name = 'one_audit_logs'"
                    }
                },
                &[],
            )
            .await?
            > 0;
        if !audit_present {
            return Ok(SiemForwardReport::idle("this deployment has no audit log"));
        }

        let cursor: Option<(i64, String)> = self
            .db
            .fetch_optional_as::<(i64, String)>(
                "SELECT last_created_at, last_id FROM one_siem_cursor WHERE tenant_id = ?",
                &db_params![tenant_id],
            )
            .await?;
        let (mut after_at, mut after_id) = match cursor {
            Some(c) => c,
            None => {
                // First run: start where export was switched on.
                self.db
                    .execute(
                        "INSERT INTO one_siem_cursor (tenant_id, last_created_at, last_id) VALUES (?, ?, '')",
                        &db_params![tenant_id, enabled_at],
                    )
                    .await?;
                (enabled_at, String::new())
            }
        };

        let mut forwarded: u64 = 0;
        for _ in 0..MAX_BATCHES_PER_TICK {
            let rows: Vec<AuditRow> = self
                .db
                .fetch_all_as::<AuditRow>(
                    "SELECT id, user_id, username, action, resource, ip_address, user_agent, created_at, latency_ms, result \
                     FROM one_audit_logs \
                     WHERE tenant_id = ? AND (created_at > ? OR (created_at = ? AND id > ?)) \
                     ORDER BY created_at ASC, id ASC LIMIT ?",
                    &db_params![tenant_id, after_at, after_at, &after_id, BATCH],
                )
                .await?;
            let Some(last) = rows.last() else { break };
            let (next_at, next_id) = (last.7, last.0.clone());
            if let Err(error) = send(kind, &endpoint, secret.as_deref(), tenant_id, &rows).await {
                let error = clip(error);
                self.record_siem_run(tenant_id, "error", Some(&error), forwarded)
                    .await?;
                return Ok(SiemForwardReport {
                    status: "error".into(),
                    forwarded,
                    message: error,
                });
            }
            let count = rows.len() as u64;
            forwarded += count;
            after_at = next_at;
            after_id = next_id;
            self.db
                .execute(
                    "UPDATE one_siem_cursor SET last_created_at = ?, last_id = ? WHERE tenant_id = ?",
                    &db_params![after_at, &after_id, tenant_id],
                )
                .await?;
            if (count as i64) < BATCH {
                break;
            }
        }
        self.record_siem_run(tenant_id, "ok", None, forwarded).await?;
        Ok(SiemForwardReport {
            status: "ok".into(),
            forwarded,
            message: if forwarded == 0 {
                "nothing new to send".into()
            } else {
                format!("sent {forwarded} events")
            },
        })
    }

    async fn record_siem_run(
        &self,
        tenant_id: &str,
        status: &str,
        error: Option<&str>,
        forwarded: u64,
    ) -> Result<(), PlatformError> {
        self.db
            .execute(
                "UPDATE one_siem_cursor SET last_run_at = ?, last_status = ?, last_error = ?, \
                 forwarded_total = forwarded_total + ? WHERE tenant_id = ?",
                &db_params![now_ms(), status, error, forwarded as i64, tenant_id],
            )
            .await?;
        Ok(())
    }

    /// One pass for every company that has export switched on.
    pub async fn forward_siem_all(&self) {
        let tenants = match self
            .db
            .fetch_all_scalar::<String>("SELECT tenant_id FROM one_siem_config WHERE enabled = 1", &[])
            .await
        {
            Ok(t) => t,
            Err(error) => {
                tracing::warn!(%error, "could not list companies with SIEM export enabled");
                return;
            }
        };
        for tenant_id in tenants {
            match self.forward_siem_once(&tenant_id).await {
                Ok(report) if report.status == "error" => {
                    tracing::warn!(tenant_id, error = %report.message, "SIEM export failed; will retry");
                }
                Ok(report) if report.forwarded > 0 => {
                    tracing::info!(
                        tenant_id,
                        forwarded = report.forwarded,
                        "SIEM export delivered audit events"
                    );
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(tenant_id, %error, "SIEM export pass failed"),
            }
        }
    }

    /// Run [`Self::forward_siem_all`] every half minute for the life of the process.
    pub fn spawn_siem_forwarder(self: &Arc<Self>) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                service.forward_siem_all().await;
            }
        });
    }

    /// Delivery state for the console: `(last_run_at, last_status, last_error, forwarded_total)`.
    pub(crate) async fn siem_delivery_state(
        &self,
        tenant_id: &str,
    ) -> Result<Option<(Option<i64>, Option<String>, Option<String>, i64)>, PlatformError> {
        Ok(self
            .db
            .fetch_optional_as::<(Option<i64>, Option<String>, Option<String>, i64)>(
                "SELECT last_run_at, last_status, last_error, forwarded_total FROM one_siem_cursor WHERE tenant_id = ?",
                &db_params![tenant_id],
            )
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, at: i64, result: &str) -> AuditRow {
        (
            id.into(),
            Some("u1".into()),
            Some("alice".into()),
            "login".into(),
            Some("res\nsplit".into()),
            Some("10.0.0.1".into()),
            None,
            at,
            Some(5),
            Some(result.into()),
        )
    }

    #[test]
    fn civil_dates_are_right_around_leap_days_and_the_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_385), (2025, 10, 24));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn a_syslog_record_is_one_line_with_the_right_priority_and_time() {
        // 2026-10-24T01:02:03.004Z
        let at = 1_792_803_723_004;
        let body = syslog_body("t1", &[row("a", at, "success"), row("b", at, "failure")]);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "a newline inside a field must not split a record: {body:?}"
        );
        assert!(lines[0].starts_with("<134>1 2026-10-24T01:02:03.004Z one-work dreamcore - audit - {"));
        assert!(lines[1].starts_with("<132>1 "), "a failure is warning severity");
        assert!(lines[0].contains("\"tenantId\":\"t1\"") && lines[0].contains("res\\nsplit"));
    }

    #[test]
    fn syslog_endpoints_default_to_port_514() {
        assert_eq!(syslog_address("siem.corp").unwrap(), "siem.corp:514");
        assert_eq!(syslog_address("tcp://siem.corp:6514/").unwrap(), "siem.corp:6514");
        assert!(syslog_address("  ").is_err());
    }

    #[test]
    fn splunk_events_carry_epoch_seconds_and_http_is_ndjson() {
        let rows = [row("a", 1_500, "success")];
        let splunk = splunk_body("t1", &rows);
        let value: serde_json::Value = serde_json::from_str(&splunk).unwrap();
        assert_eq!(value["time"], 1.5);
        assert_eq!(value["event"]["action"], "login");
        let ndjson = ndjson_body("t1", &[row("a", 1, "success"), row("b", 2, "success")]);
        assert_eq!(ndjson.lines().count(), 2);
        assert!(ndjson.ends_with('\n'));
    }

    // ---- delivery, against real listeners ------------------------------------

    use std::sync::Mutex;

    #[derive(Default)]
    struct Received {
        bodies: Vec<String>,
        auth: Vec<String>,
    }

    type CollectorState = (Arc<Mutex<Received>>, Arc<Mutex<u16>>);

    /// An HTTP collector that answers `status` and records what it was sent.
    async fn collector(status: Arc<Mutex<u16>>) -> (String, Arc<Mutex<Received>>) {
        let received = Arc::new(Mutex::new(Received::default()));
        let state: CollectorState = (received.clone(), status);
        let app = axum::Router::new()
            .route(
                "/hec",
                axum::routing::post(
                    |axum::extract::State((received, status)): axum::extract::State<CollectorState>,
                     headers: axum::http::HeaderMap,
                     body: String| async move {
                        let mut r = received.lock().unwrap();
                        r.bodies.push(body);
                        r.auth.push(
                            headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_owned(),
                        );
                        axum::http::StatusCode::from_u16(*status.lock().unwrap()).unwrap()
                    },
                ),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hec", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, received)
    }

    async fn setup() -> (dream_core_db::Database, PlatformService) {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let pool = dream_core_db::DbPool::Sqlite(db.pool().clone());
        crate::migrate::run_one_platform_migrations(&pool).await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE one_audit_logs (id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL DEFAULT 'default', \
             user_id TEXT, username TEXT, action TEXT NOT NULL, resource TEXT, ip_address TEXT, user_agent TEXT, \
             created_at INTEGER NOT NULL, latency_ms INTEGER, result TEXT NOT NULL DEFAULT 'success');",
        )
        .execute(db.pool())
        .await
        .unwrap();
        (db, PlatformService::new(pool, [7u8; 32]))
    }

    async fn audit(db: &dream_core_db::Database, id: &str, tenant: &str, at: i64) {
        sqlx::query(
            "INSERT INTO one_audit_logs (id, tenant_id, user_id, username, action, created_at) \
             VALUES (?, ?, 'u', 'alice', 'login', ?)",
        )
        .bind(id)
        .bind(tenant)
        .bind(at)
        .execute(db.pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn only_events_after_export_was_switched_on_are_sent_and_never_twice() {
        let (db, service) = setup().await;
        let (url, received) = collector(Arc::new(Mutex::new(200))).await;
        // History from before export existed.
        audit(&db, "old", "t1", now_ms() - 3_600_000).await;
        service
            .set_siem_config("t1", Some("splunk"), Some(&url), Some("hec-token"), true)
            .await
            .unwrap();
        audit(&db, "new-1", "t1", now_ms() + 10).await;
        audit(&db, "other-company", "t2", now_ms() + 10).await;

        let first = service.forward_siem_once("t1").await.unwrap();
        assert_eq!((first.status.as_str(), first.forwarded), ("ok", 1), "{first:?}");
        {
            let r = received.lock().unwrap();
            assert_eq!(r.bodies.len(), 1);
            assert!(r.bodies[0].contains("new-1"));
            assert!(!r.bodies[0].contains("\"old\"") && !r.bodies[0].contains("other-company"));
            assert_eq!(r.auth[0], "Splunk hec-token");
        }

        let again = service.forward_siem_once("t1").await.unwrap();
        assert_eq!(again.forwarded, 0, "an event is delivered once");
        assert_eq!(received.lock().unwrap().bodies.len(), 1);
        let dto = service.get_siem_config("t1").await.unwrap();
        assert_eq!(
            (dto.forwarded_total, dto.last_forward_status.as_deref()),
            (1, Some("ok"))
        );
    }

    #[tokio::test]
    async fn a_receiver_that_is_down_loses_nothing() {
        let (db, service) = setup().await;
        let status = Arc::new(Mutex::new(503));
        let (url, received) = collector(status.clone()).await;
        service
            .set_siem_config("t1", Some("http"), Some(&url), None, true)
            .await
            .unwrap();
        audit(&db, "e1", "t1", now_ms() + 10).await;
        audit(&db, "e2", "t1", now_ms() + 20).await;

        let failed = service.forward_siem_once("t1").await.unwrap();
        assert_eq!((failed.status.as_str(), failed.forwarded), ("error", 0));
        assert!(failed.message.contains("503"), "{failed:?}");
        let dto = service.get_siem_config("t1").await.unwrap();
        assert_eq!(dto.last_forward_status.as_deref(), Some("error"));
        assert!(dto.last_forward_error.unwrap().contains("503"));

        *status.lock().unwrap() = 200;
        audit(&db, "e3", "t1", now_ms() + 30).await;
        let recovered = service.forward_siem_once("t1").await.unwrap();
        assert_eq!((recovered.status.as_str(), recovered.forwarded), ("ok", 3));
        let bodies = received.lock().unwrap().bodies.join("");
        for id in ["e1", "e2", "e3"] {
            assert!(bodies.contains(id), "{id} must arrive after the outage: {bodies}");
        }
    }

    #[tokio::test]
    async fn events_sharing_a_millisecond_are_not_skipped_across_batches() {
        let (db, service) = setup().await;
        let (url, received) = collector(Arc::new(Mutex::new(200))).await;
        service
            .set_siem_config("t1", Some("http"), Some(&url), None, true)
            .await
            .unwrap();
        let at = now_ms() + 100;
        for i in 0..(BATCH + 5) {
            audit(&db, &format!("same-{i:04}"), "t1", at).await;
        }
        let report = service.forward_siem_once("t1").await.unwrap();
        assert_eq!(report.forwarded, (BATCH + 5) as u64, "{report:?}");
        let lines: usize = received.lock().unwrap().bodies.iter().map(|b| b.lines().count()).sum();
        assert_eq!(lines as i64, BATCH + 5);
    }

    #[tokio::test]
    async fn syslog_records_arrive_over_tcp() {
        let (db, service) = setup().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let reader = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut text = String::new();
            socket.read_to_string(&mut text).await.unwrap();
            text
        });
        service
            .set_siem_config("t1", Some("syslog"), Some(&address), None, true)
            .await
            .unwrap();
        audit(&db, "s1", "t1", now_ms() + 10).await;
        let report = service.forward_siem_once("t1").await.unwrap();
        assert_eq!(report.forwarded, 1, "{report:?}");
        let text = reader.await.unwrap();
        assert!(
            text.starts_with("<134>1 ") && text.contains("\"id\":\"s1\"") && text.ends_with('\n'),
            "{text:?}"
        );
    }

    #[tokio::test]
    async fn a_disabled_or_unconfigured_company_sends_nothing_and_re_enabling_does_not_replay() {
        let (db, service) = setup().await;
        let (url, received) = collector(Arc::new(Mutex::new(200))).await;
        assert_eq!(service.forward_siem_once("t1").await.unwrap().status, "idle");
        service
            .set_siem_config("t1", Some("http"), Some(&url), None, true)
            .await
            .unwrap();
        audit(&db, "a", "t1", now_ms() + 10).await;
        service.forward_siem_once("t1").await.unwrap();
        service
            .set_siem_config("t1", Some("http"), Some(&url), None, false)
            .await
            .unwrap();
        audit(&db, "while-off", "t1", now_ms() + 20).await;
        assert_eq!(service.forward_siem_once("t1").await.unwrap().status, "idle");
        // Switching it back on starts from now: what happened while it was off stays off.
        tokio::time::sleep(Duration::from_millis(60)).await;
        service
            .set_siem_config("t1", Some("http"), Some(&url), None, true)
            .await
            .unwrap();
        let report = service.forward_siem_once("t1").await.unwrap();
        assert_eq!(report.forwarded, 0, "{report:?}");
        assert!(!received.lock().unwrap().bodies.join("").contains("while-off"));
    }
}
