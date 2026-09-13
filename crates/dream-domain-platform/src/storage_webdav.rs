//! WebDAV driver — the practical way to reach an enterprise NAS.
//!
//! Chosen over SMB/NFS on purpose: Synology, QNAP, TrueNAS and Windows Server
//! all expose WebDAV with a checkbox, it is plain HTTP so it crosses the same
//! network path the rest of the product already uses, and it needs no kernel
//! mount or credential cache on the server. SMB would mean an SMB client in
//! the backend and a mounted share per tenant.
//!
//! Only the two operations the console needs: PROPFIND Depth:0 to prove the
//! credentials reach the share, PROPFIND Depth:1 to list one level.
//!
//! The XML is parsed with `quick-xml` (already in the tree via aws-sdk-s3) in
//! a deliberately narrow way — namespace prefixes vary by server (`d:`, `D:`,
//! none at all), so matching is on the local name only.

use async_trait::async_trait;
use base64::Engine as _;
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::PlatformError;
use crate::storage_driver::{DriverConfig, DriverEntry, DriverListing, StorageDriver};

pub struct WebDavDriver;

/// Ask for exactly the four properties we render. Servers may return more;
/// asking narrowly keeps large directories cheap.
const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:resourcetype/>
    <d:getcontentlength/>
    <d:getlastmodified/>
    <d:displayname/>
  </d:prop>
</d:propfind>"#;

/// Join the configured endpoint, the share ("bucket") and a prefix into one
/// URL, without doubling or dropping separators.
fn build_url(cfg: &DriverConfig, prefix: &str) -> String {
    let base = cfg.endpoint.trim_end_matches('/');
    let share = cfg.bucket.trim_matches('/');
    let mut url = base.to_owned();
    if !share.is_empty() {
        url.push('/');
        url.push_str(share);
    }
    let prefix = prefix.trim_start_matches('/');
    if !prefix.is_empty() {
        url.push('/');
        url.push_str(prefix.trim_end_matches('/'));
    }
    url.push('/');
    url
}

/// One `<response>` from a multistatus body.
#[derive(Default)]
struct DavResponse {
    href: String,
    is_collection: bool,
    size: Option<i64>,
    last_modified: Option<i64>,
}

/// Local name of a tag, ignoring whatever namespace prefix the server used.
fn local_name(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    s.rsplit(':').next().unwrap_or(&s).to_ascii_lowercase()
}

/// Parse a WebDAV multistatus body into its responses.
///
/// Tolerant by design: a property we do not recognise is skipped, and a
/// response missing `getcontentlength` (every collection) simply has no size.
fn parse_multistatus(xml: &str) -> Result<Vec<DavResponse>, PlatformError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut out: Vec<DavResponse> = Vec::new();
    let mut current: Option<DavResponse> = None;
    let mut field: Option<String> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                match name.as_str() {
                    "response" => current = Some(DavResponse::default()),
                    "href" | "getcontentlength" | "getlastmodified" => field = Some(name),
                    _ => {}
                }
            }
            // `<collection/>` is an empty element, so it never reaches
            // Event::Start — missing this is why a naive parser reports every
            // folder as a zero-byte file.
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == "collection"
                    && let Some(cur) = current.as_mut()
                {
                    cur.is_collection = true;
                }
            }
            Ok(Event::Text(e)) => {
                if let (Some(f), Some(cur)) = (field.as_deref(), current.as_mut()) {
                    // quick-xml 0.41: decode() handles the encoding, unescape()
                    // handles the entities — they are separate calls here.
                    let decoded = e.decode().unwrap_or_default().into_owned();
                    let text = quick_xml::escape::unescape(&decoded)
                        .map(|c| c.into_owned())
                        .unwrap_or(decoded)
                        .trim()
                        .to_string();
                    match f {
                        "href" => cur.href = text,
                        "getcontentlength" => cur.size = text.parse::<i64>().ok(),
                        "getlastmodified" => cur.last_modified = parse_http_date(&text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                if name == "response" {
                    if let Some(cur) = current.take() {
                        out.push(cur);
                    }
                } else if Some(name.as_str()) == field.as_deref() {
                    field = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(PlatformError::BadRequest(format!("malformed WebDAV response: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// RFC 1123 date → epoch millis. Hand-rolled because pulling a date crate for
/// one field is not worth it, and a `None` here only costs a dash in the UI.
fn parse_http_date(raw: &str) -> Option<i64> {
    // "Sat, 13 Sep 2026 11:17:59 GMT"
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    let day: i64 = parts[1].parse().ok()?;
    let month = match parts[2] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts[3].parse().ok()?;
    let hms: Vec<&str> = parts[4].split(':').collect();
    if hms.len() != 3 {
        return None;
    }
    let (h, m, s): (i64, i64, i64) = (hms[0].parse().ok()?, hms[1].parse().ok()?, hms[2].parse().ok()?);

    // Days since epoch via the civil-from-days algorithm (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400) + h * 3600 + m * 60 + s) * 1000)
}

fn auth_header(cfg: &DriverConfig) -> String {
    let raw = format!("{}:{}", cfg.access_key_id, cfg.secret_access_key);
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

async fn propfind(cfg: &DriverConfig, url: &str, depth: &str) -> Result<(u16, String), PlatformError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| PlatformError::Internal(format!("http client: {e}")))?;

    let res = client
        .request(reqwest::Method::from_bytes(b"PROPFIND").expect("valid method"), url)
        .header("Authorization", auth_header(cfg))
        .header("Depth", depth)
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(PROPFIND_BODY)
        .send()
        .await
        .map_err(|e| {
            // reqwest's Display is already operator-readable here ("connection
            // refused", "dns error"); the smithy-style dump problem is S3-only.
            PlatformError::BadRequest(format!("{e}"))
        })?;

    let status = res.status().as_u16();
    let body = res.text().await.unwrap_or_default();
    Ok((status, body))
}

#[async_trait]
impl StorageDriver for WebDavDriver {
    async fn probe(&self, cfg: &DriverConfig) -> Result<String, PlatformError> {
        let url = build_url(cfg, "");
        let (status, _) = propfind(cfg, &url, "0").await?;
        match status {
            207 => Ok(format!("WebDAV share {} reachable", cfg.bucket)),
            401 | 403 => Err(PlatformError::BadRequest(format!("{status} — 用户名或密码被拒绝"))),
            404 => Err(PlatformError::BadRequest(format!("404 — 路径不存在: {url}"))),
            other => Err(PlatformError::BadRequest(format!("HTTP {other}"))),
        }
    }

    async fn list(
        &self,
        cfg: &DriverConfig,
        prefix: &str,
        _token: Option<&str>,
        limit: i32,
    ) -> Result<DriverListing, PlatformError> {
        let url = build_url(cfg, prefix);
        let (status, body) = propfind(cfg, &url, "1").await?;
        if status != 207 {
            return Err(PlatformError::BadRequest(format!("HTTP {status}")));
        }

        let responses = parse_multistatus(&body)?;
        // The first response is the collection itself; everything after is a
        // child. Comparing decoded hrefs is unreliable across servers, so this
        // relies on the ordering WebDAV guarantees for Depth:1.
        let entries: Vec<DriverEntry> = responses
            .into_iter()
            .skip(1)
            .take(limit.clamp(1, 1000) as usize)
            .filter_map(|r| {
                let decoded = percent_decode(&r.href);
                let name = decoded.trim_end_matches('/').rsplit('/').next()?.to_owned();
                if name.is_empty() {
                    return None;
                }
                Some(DriverEntry {
                    key: if prefix.is_empty() {
                        format!("{name}{}", if r.is_collection { "/" } else { "" })
                    } else {
                        format!("{prefix}{name}{}", if r.is_collection { "/" } else { "" })
                    },
                    name,
                    is_prefix: r.is_collection,
                    size_bytes: if r.is_collection { None } else { r.size },
                    last_modified: r.last_modified,
                })
            })
            .collect();

        // WebDAV has no continuation token; Depth:1 returns the whole level.
        Ok(DriverListing {
            entries,
            next_token: None,
        })
    }
}

/// Minimal percent-decoding for hrefs. Servers escape spaces and CJK names.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&raw[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(endpoint: &str, bucket: &str) -> DriverConfig {
        DriverConfig {
            endpoint: endpoint.into(),
            bucket: bucket.into(),
            region: String::new(),
            access_key_id: "nas".into(),
            secret_access_key: "pw".into(),
            force_path_style: true,
        }
    }

    #[test]
    fn urls_join_without_doubling_or_dropping_slashes() {
        assert_eq!(build_url(&cfg("http://nas:5005", "team"), ""), "http://nas:5005/team/");
        assert_eq!(
            build_url(&cfg("http://nas:5005/", "/team/"), ""),
            "http://nas:5005/team/"
        );
        assert_eq!(
            build_url(&cfg("http://nas:5005", "team"), "media/"),
            "http://nas:5005/team/media/"
        );
        assert_eq!(
            build_url(&cfg("http://nas:5005", ""), "media/"),
            "http://nas:5005/media/"
        );
    }

    #[test]
    fn multistatus_parses_folders_and_files_regardless_of_namespace_prefix() {
        // `D:` here, `d:` on Synology, bare on some others — matching is on
        // the local name for exactly this reason.
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/team/</D:href><D:propstat><D:prop>
    <D:resourcetype><D:collection/></D:resourcetype>
  </D:prop></D:propstat></D:response>
  <D:response><D:href>/team/media/</D:href><D:propstat><D:prop>
    <D:resourcetype><D:collection/></D:resourcetype>
    <D:getlastmodified>Sat, 13 Sep 2026 11:17:59 GMT</D:getlastmodified>
  </D:prop></D:propstat></D:response>
  <D:response><D:href>/team/%E6%8A%A5%E5%91%8A.pdf</D:href><D:propstat><D:prop>
    <D:resourcetype/>
    <D:getcontentlength>2048</D:getcontentlength>
    <D:getlastmodified>Sat, 13 Sep 2026 11:18:00 GMT</D:getlastmodified>
  </D:prop></D:propstat></D:response>
</D:multistatus>"#;

        let parsed = parse_multistatus(xml).unwrap();
        assert_eq!(parsed.len(), 3, "self + two children");
        assert!(parsed[0].is_collection);
        assert!(parsed[1].is_collection, "empty <collection/> must still mark a folder");
        assert!(!parsed[2].is_collection);
        assert_eq!(parsed[2].size, Some(2048));
        assert!(parsed[2].last_modified.is_some());
    }

    #[test]
    fn hrefs_are_percent_decoded() {
        assert_eq!(percent_decode("/team/%E6%8A%A5%E5%91%8A.pdf"), "/team/报告.pdf");
        assert_eq!(percent_decode("/team/a%20b.txt"), "/team/a b.txt");
        // A stray % that is not an escape must not eat the rest of the string.
        assert_eq!(percent_decode("/team/100%.txt"), "/team/100%.txt");
    }

    #[test]
    fn http_dates_convert_to_epoch_millis() {
        // 2026-09-13T11:17:59Z
        assert_eq!(
            parse_http_date("Sat, 13 Sep 2026 11:17:59 GMT"),
            Some(1_789_298_279_000)
        );
        // A known fixed point, independent of the above.
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("not a date"), None);
        assert_eq!(parse_http_date(""), None);
    }
}
