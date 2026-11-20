//! Price discovery for the admin model price page.
//!
//! Two sources feed it:
//!
//! * **A company source** — the admin's own LiteLLM or relay (中转站) address.
//!   Formats vary by gateway, so [`parse_catalog`] walks the JSON looking for
//!   anything that looks like "a model name next to prices" instead of
//!   hard-coding one schema. A page that is HTML (a console SPA) is not data;
//!   the fetcher then tries the gateway's well-known JSON endpoints on the
//!   same origin.
//! * **The public LiteLLM price catalog** — a community-maintained table of
//!   vendor list prices, used as the "official reference" when the company
//!   source does not know a model. It is a reference, never an authority, and
//!   the console says so before a price from it is applied.
//!
//! Matching is deliberately conservative: exact name, then the same name
//! ignoring a provider prefix, then the same name ignoring a trailing date /
//! snapshot suffix. Nothing fuzzier — a wrong price that looks right is worse
//! than "not found".

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::models::{PriceLookupDto, PriceLookupItemDto, PriceSuggestionDto};
use dream_core_common::license::BillingUnit;
use serde_json::{Map, Value};

/// One model's price as a source states it, normalised to the units the price
/// table stores (USD per million tokens, or USD per image / second / request).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CatalogEntry {
    pub key: String,
    pub unit: BillingUnit,
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
    pub cache_read_usd_per_mtok: Option<f64>,
    pub cache_write_usd_per_mtok: Option<f64>,
    pub unit_price_usd: f64,
    /// Other names the source knows this model by — a relay's `backendNames`
    /// (the upstream models behind its own alias). Matched only loosely.
    pub aliases: Vec<String>,
}

/// Round away float noise from `2.5e-06 * 1e6 = 2.4999999999999996`.
fn r6(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

fn number(map: &Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key)
        .and_then(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        })
        .filter(|n| n.is_finite() && *n >= 0.0)
}

fn first_number(map: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| number(map, k))
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Pull an entry out of one JSON object, if it states a price.
fn entry_from_object(map: &Map<String, Value>, key_hint: Option<&str>) -> Option<CatalogEntry> {
    let name = ["modelName", "model_name", "model", "id", "name"]
        .iter()
        .find_map(|k| text(map, k))
        .or(key_hint)?;

    // Where the prices live: the object itself (LiteLLM catalog), its
    // `model_info` (LiteLLM proxy /model/info), or a `pricing` block (relay
    // consoles quoting "cost per million").
    let model_info = map.get("model_info").and_then(Value::as_object);
    let pricing = map.get("pricing").and_then(Value::as_object);
    let has_cost = |m: &Map<String, Value>| m.keys().any(|k| k.contains("cost_per") || k.contains("CostPer"));
    let source = if has_cost(map) {
        map
    } else if let Some(info) = model_info.filter(|m| has_cost(m)) {
        info
    } else {
        pricing.filter(|m| has_cost(m))?
    };

    let mut entry = CatalogEntry {
        key: name.to_owned(),
        aliases: map
            .get("backendNames")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default(),
        ..Default::default()
    };

    // A relay quoting prices per million tokens already uses our unit.
    let per_million = |keys: &[&str]| first_number(source, keys).map(r6);
    // LiteLLM quotes dollars per single token.
    let per_token = |keys: &[&str]| first_number(source, keys).map(|v| r6(v * 1_000_000.0));
    let input = per_million(&["inputCostPerMillion", "input_cost_per_million"])
        .or_else(|| per_token(&["input_cost_per_token"]));
    let output = per_million(&["outputCostPerMillion", "output_cost_per_million"])
        .or_else(|| per_token(&["output_cost_per_token"]));
    let cache_read = per_million(&["cacheReadCostPerMillion", "cache_read_cost_per_million"])
        .or_else(|| per_token(&["cache_read_input_token_cost"]));
    let cache_write = per_million(&["cacheWriteCostPerMillion", "cache_write_cost_per_million"])
        .or_else(|| per_token(&["cache_creation_input_token_cost"]));

    let mode = text(source, "mode")
        .or_else(|| text(map, "mode"))
        .or_else(|| model_info.and_then(|m| text(m, "mode")));
    let per_image = first_number(source, &["output_cost_per_image", "input_cost_per_image"]);
    let per_second = first_number(
        source,
        &[
            "output_cost_per_second",
            "output_cost_per_video_per_second",
            "input_cost_per_second",
            "input_cost_per_audio_per_second",
        ],
    );
    let per_request = first_number(source, &["cost_per_request", "input_cost_per_request"]);
    let token_priced = input.is_some() || output.is_some();

    // The mode decides when a model states both token and asset prices (an
    // image model that bills image tokens AND lists a per-image figure);
    // otherwise whichever price exists wins.
    if mode == Some("video_generation") && per_second.is_some() {
        entry.unit = BillingUnit::Second;
        entry.unit_price_usd = r6(per_second.unwrap_or(0.0));
    } else if mode == Some("image_generation") && per_image.is_some() && !token_priced {
        entry.unit = BillingUnit::Image;
        entry.unit_price_usd = r6(per_image.unwrap_or(0.0));
    } else if token_priced {
        entry.unit = BillingUnit::Token;
    } else if let Some(price) = per_image {
        entry.unit = BillingUnit::Image;
        entry.unit_price_usd = r6(price);
    } else if let Some(price) = per_second {
        entry.unit = BillingUnit::Second;
        entry.unit_price_usd = r6(price);
    } else if let Some(price) = per_request {
        entry.unit = BillingUnit::Request;
        entry.unit_price_usd = r6(price);
    } else {
        return None;
    }
    if entry.unit == BillingUnit::Token {
        entry.input_usd_per_mtok = input.unwrap_or(0.0);
        entry.output_usd_per_mtok = output.unwrap_or(0.0);
        entry.cache_read_usd_per_mtok = cache_read;
        entry.cache_write_usd_per_mtok = cache_write;
    }
    Some(entry)
}

fn collect(value: &Value, key_hint: Option<&str>, out: &mut Vec<CatalogEntry>, depth: usize) {
    if depth > 6 {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                collect(item, None, out, depth + 1);
            }
        }
        Value::Object(map) => {
            if let Some(entry) = entry_from_object(map, key_hint) {
                out.push(entry);
                return;
            }
            for (key, child) in map {
                if key == "sample_spec" {
                    continue;
                }
                collect(child, Some(key), out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Every priced model a JSON document states. Handles the public LiteLLM
/// catalog (`{ "<model>": {input_cost_per_token…} }`), LiteLLM proxy
/// `/model/info` (`{ data: [{model_name, model_info}] }`) and relay consoles
/// that quote `pricing.inputCostPerMillion`.
pub fn parse_catalog(root: &Value) -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    collect(root, None, &mut out, 0);
    out
}

fn is_digits(part: &str, len: usize) -> bool {
    part.len() == len && part.chars().all(|c| c.is_ascii_digit())
}

/// `provider/Model-Name-20250514` → `model-name`: lowercase, no provider
/// prefix, no `@version` / `:tag`, no trailing snapshot date (`-20250514`,
/// `-2025-05-14`, `-261017`) and no `-latest`.
pub fn normalize_model_name(name: &str) -> String {
    let mut s = name.trim().to_ascii_lowercase();
    if let Some(i) = s.rfind('/') {
        s = s[i + 1..].to_owned();
    }
    if let Some(i) = s.find('@') {
        s.truncate(i);
    }
    if let Some(i) = s.find(':') {
        s.truncate(i);
    }
    let mut parts: Vec<&str> = s.split('-').collect();
    let n = parts.len();
    if n >= 4 && is_digits(parts[n - 3], 4) && is_digits(parts[n - 2], 2) && is_digits(parts[n - 1], 2) {
        parts.truncate(n - 3);
    }
    while parts.len() > 1 {
        let last = parts[parts.len() - 1];
        let snapshot = last.chars().all(|c| c.is_ascii_digit()) && (last.len() == 6 || last.len() == 8);
        if snapshot || last == "latest" {
            parts.pop();
        } else {
            break;
        }
    }
    parts.join("-")
}

fn bare_name(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    lower.rsplit('/').next().unwrap_or(&lower).to_owned()
}

/// How a candidate ranks among several keys that all name the same model:
/// unprefixed first (the canonical entry), then first-party provider
/// prefixes, then whatever else; shorter keys break ties.
fn rank(key: &str) -> (u8, usize) {
    const FIRST_PARTY: [&str; 8] = [
        "openai/",
        "anthropic/",
        "gemini/",
        "deepseek/",
        "dashscope/",
        "moonshot/",
        "volcengine/",
        "zai/",
    ];
    let lower = key.to_ascii_lowercase();
    let tier = if !lower.contains('/') {
        0
    } else if FIRST_PARTY.iter().any(|p| lower.starts_with(p)) {
        1
    } else {
        2
    };
    (tier, key.len())
}

/// A catalog entry found for a model, and whether the name matched as given
/// (`exact`) or only after ignoring a provider prefix's absence or a snapshot
/// suffix.
#[derive(Debug, Clone, PartialEq)]
pub struct Match<'a> {
    pub entry: &'a CatalogEntry,
    pub exact: bool,
}

pub fn find_match<'a>(model: &str, entries: &'a [CatalogEntry]) -> Option<Match<'a>> {
    let lower = model.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    let best = |candidates: Vec<&'a CatalogEntry>| candidates.into_iter().min_by_key(|e| rank(&e.key));

    // A name that matches only a third-party gateway's entry (`aiml/dall-e-3`,
    // `gmi/anthropic/…`) is a gateway's resale price, not the vendor's — found,
    // but never presented as an exact match.
    let canonical = |entry: &'a CatalogEntry| rank(&entry.key).0 <= 1;
    let exact: Vec<_> = entries.iter().filter(|e| e.key.to_ascii_lowercase() == lower).collect();
    if let Some(entry) = best(exact) {
        return Some(Match {
            exact: canonical(entry),
            entry,
        });
    }
    let bare = bare_name(&lower);
    let same_bare: Vec<_> = entries.iter().filter(|e| bare_name(&e.key) == bare).collect();
    if let Some(entry) = best(same_bare) {
        return Some(Match {
            exact: canonical(entry),
            entry,
        });
    }
    let wanted = normalize_model_name(&lower);
    if wanted.is_empty() {
        return None;
    }
    let same_family: Vec<_> = entries
        .iter()
        .filter(|e| normalize_model_name(&e.key) == wanted)
        .collect();
    if let Some(entry) = best(same_family) {
        return Some(Match { entry, exact: false });
    }
    // Last resort: a relay lists the upstream model behind its alias
    // (`deepseek-flash-latest` → `deepseek-v4-1-flash-volc`). The vendor / route
    // tag on the end of a backend name is not part of the model, so it is
    // dropped before comparing; the result is always flagged as not exact.
    let behind_alias: Vec<_> = entries
        .iter()
        .filter(|e| {
            e.aliases.iter().any(|alias| {
                let alias = normalize_model_name(alias);
                alias == wanted || alias.rsplit_once('-').is_some_and(|(model, _tag)| model == wanted)
            })
        })
        .collect();
    best(behind_alias).map(|entry| Match { entry, exact: false })
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const OFFICIAL_TTL: Duration = Duration::from_secs(6 * 3600);
const OFFICIAL_URLS: [&str; 2] = [
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json",
    "https://cdn.jsdelivr.net/gh/BerriAI/litellm@main/model_prices_and_context_window.json",
];
/// JSON endpoints tried on the same origin when the address the admin gave is
/// a console page (HTML) rather than data.
const GATEWAY_JSON_PATHS: [&str; 3] = ["/api/admin/model-list", "/model/info", "/v1/model/info"];

/// Credentials the admin supplies for a source behind a login. Used for the
/// one request and never stored or logged.
#[derive(Default, Clone)]
pub struct FetchAuth {
    pub cookie: Option<String>,
    pub authorization: Option<String>,
}

impl FetchAuth {
    pub fn is_empty(&self) -> bool {
        self.cookie.as_deref().is_none_or(|c| c.trim().is_empty())
            && self.authorization.as_deref().is_none_or(|a| a.trim().is_empty())
    }
}

#[derive(Debug)]
pub enum CustomFetch {
    Entries {
        entries: Vec<CatalogEntry>,
        from: String,
    },
    /// The source answered 401/403, or the credentials were refused.
    AuthRequired {
        message: String,
    },
    Failed {
        message: String,
    },
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("dream-core-price-sync")
        .build()
        .map_err(|e| e.to_string())
}

/// Refuse addresses a price page is never at and a request must not reach:
/// non-http(s) schemes, and link-local / unspecified IPs (cloud metadata
/// services live at 169.254.169.254). Private ranges stay allowed — a
/// company's internal gateway is exactly the intended target.
pub fn check_fetch_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "地址格式不正确".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("只支持 http / https 地址".into());
    }
    let host = url.host_str().ok_or_else(|| "地址缺少主机名".to_owned())?;
    if host.eq_ignore_ascii_case("metadata.google.internal") {
        return Err("该地址不允许访问".into());
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        let blocked = match ip {
            std::net::IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified(),
            std::net::IpAddr::V6(v6) => v6.is_unspecified() || (v6.segments()[0] & 0xffc0) == 0xfe80,
        };
        if blocked {
            return Err("该地址不允许访问".into());
        }
    }
    Ok(url)
}

async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err("返回内容过大".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn bearer(value: &str) -> String {
    let value = value.trim();
    if value.contains(' ') {
        value.to_owned()
    } else {
        format!("Bearer {value}")
    }
}

enum Probe {
    Entries(Vec<CatalogEntry>),
    Auth,
    NotData,
    Failed(String),
}

async fn probe(client: &reqwest::Client, url: reqwest::Url, auth: &FetchAuth) -> Probe {
    let mut request = client.get(url).header("Accept", "application/json");
    if let Some(cookie) = auth.cookie.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        request = request.header("Cookie", cookie);
    }
    if let Some(value) = auth.authorization.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        request = request.header("Authorization", bearer(value));
    }
    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => return Probe::Failed(format!("无法连接：{}", e.without_url())),
    };
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Probe::Auth;
    }
    if !status.is_success() {
        return Probe::Failed(format!("返回 HTTP {}", status.as_u16()));
    }
    let body = match read_capped(response).await {
        Ok(b) => b,
        Err(e) => return Probe::Failed(e),
    };
    match serde_json::from_slice::<Value>(&body) {
        Ok(json) => {
            let entries = parse_catalog(&json);
            if entries.is_empty() {
                Probe::NotData
            } else {
                Probe::Entries(entries)
            }
        }
        Err(_) => Probe::NotData,
    }
}

/// Fetch prices from the company's own source. `url` may be the JSON itself or
/// a console page; for a page the gateway's well-known JSON endpoints on the
/// same origin are tried.
pub async fn fetch_custom(url: &str, auth: &FetchAuth) -> CustomFetch {
    let parsed = match check_fetch_url(url) {
        Ok(u) => u,
        Err(message) => return CustomFetch::Failed { message },
    };
    let client = match client() {
        Ok(c) => c,
        Err(message) => return CustomFetch::Failed { message },
    };
    let mut candidates = vec![parsed.clone()];
    for path in GATEWAY_JSON_PATHS {
        if let Ok(candidate) = parsed.join(path) {
            candidates.push(candidate);
        }
    }
    let mut needs_auth = false;
    let mut last_failure = None;
    for candidate in candidates {
        let label = candidate.path().to_owned();
        match probe(&client, candidate, auth).await {
            Probe::Entries(entries) => {
                return CustomFetch::Entries {
                    entries,
                    from: format!("{}{}", parsed.host_str().unwrap_or_default(), label),
                };
            }
            Probe::Auth => needs_auth = true,
            Probe::NotData => {}
            Probe::Failed(message) => last_failure = Some(message),
        }
    }
    if needs_auth {
        let message = if auth.is_empty() {
            "该来源需要登录后才能读取价格".to_owned()
        } else {
            "提供的认证信息未通过，请重新获取后再试".to_owned()
        };
        return CustomFetch::AuthRequired { message };
    }
    CustomFetch::Failed {
        message: last_failure.unwrap_or_else(|| {
            "没有在该地址识别到模型价格（支持 LiteLLM /model/info、价格表 JSON 与中转站模型列表）".into()
        }),
    }
}

type OfficialCache = tokio::sync::Mutex<Option<(Instant, Arc<Vec<CatalogEntry>>)>>;

fn official_cache() -> &'static OfficialCache {
    static CACHE: OnceLock<OfficialCache> = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// The public LiteLLM price catalog, cached for a few hours. A stale copy is
/// served when a refresh fails — an old reference price beats none.
pub async fn official_catalog() -> Result<Arc<Vec<CatalogEntry>>, String> {
    let mut cache = official_cache().lock().await;
    if let Some((fetched, entries)) = cache.as_ref().filter(|(at, _)| at.elapsed() < OFFICIAL_TTL) {
        let _ = fetched;
        return Ok(entries.clone());
    }
    let client = client()?;
    let mut failure = String::from("无法连接公开价格库");
    for url in OFFICIAL_URLS {
        let Ok(url) = reqwest::Url::parse(url) else { continue };
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => match read_capped(response).await {
                Ok(body) => match serde_json::from_slice::<Value>(&body) {
                    Ok(json) => {
                        let entries = Arc::new(parse_catalog(&json));
                        if !entries.is_empty() {
                            *cache = Some((Instant::now(), entries.clone()));
                            return Ok(entries);
                        }
                    }
                    Err(e) => failure = format!("公开价格库格式异常：{e}"),
                },
                Err(e) => failure = e,
            },
            Ok(response) => failure = format!("公开价格库返回 HTTP {}", response.status().as_u16()),
            Err(e) => failure = format!("无法连接公开价格库：{}", e.without_url()),
        }
    }
    match cache.as_ref() {
        Some((_, stale)) => Ok(stale.clone()),
        None => Err(failure),
    }
}

fn item(model: &str, source: &str, found: Option<Match<'_>>) -> PriceLookupItemDto {
    match found {
        Some(Match { entry, exact }) => PriceLookupItemDto {
            model: model.to_owned(),
            found: true,
            source: Some(source.to_owned()),
            matched_key: Some(entry.key.clone()),
            exact,
            price: Some(PriceSuggestionDto {
                billing_unit: entry.unit.as_str().to_owned(),
                input_usd_per_mtok: entry.input_usd_per_mtok,
                output_usd_per_mtok: entry.output_usd_per_mtok,
                cache_read_usd_per_mtok: entry.cache_read_usd_per_mtok,
                cache_write_usd_per_mtok: entry.cache_write_usd_per_mtok,
                unit_price_usd: entry.unit_price_usd,
            }),
        },
        None => PriceLookupItemDto {
            model: model.to_owned(),
            found: false,
            source: None,
            matched_key: None,
            exact: false,
            price: None,
        },
    }
}

/// Look prices up for `models`: first in the company's own source (when an
/// address is given), then — for whatever it does not know — in the public
/// reference catalog. A source that wants a login stops the lookup with
/// `custom_status = "auth_required"` so the console can ask the admin to
/// authenticate instead of silently quoting reference prices over their own.
pub async fn lookup(models: &[String], source_url: Option<&str>, auth: &FetchAuth) -> PriceLookupDto {
    let mut dto = PriceLookupDto {
        items: models.iter().map(|m| item(m, "", None)).collect(),
        custom_status: "unused".into(),
        custom_message: None,
        custom_from: None,
        official_status: "unused".into(),
        official_message: None,
    };
    if let Some(url) = source_url.map(str::trim).filter(|u| !u.is_empty()) {
        match fetch_custom(url, auth).await {
            CustomFetch::Entries { entries, from } => {
                dto.custom_status = "ok".into();
                dto.custom_from = Some(from);
                for (slot, model) in dto.items.iter_mut().zip(models) {
                    *slot = item(model, "custom", find_match(model, &entries));
                }
            }
            CustomFetch::AuthRequired { message } => {
                dto.custom_status = "auth_required".into();
                dto.custom_message = Some(message);
                return dto;
            }
            CustomFetch::Failed { message } => {
                dto.custom_status = "failed".into();
                dto.custom_message = Some(message);
            }
        }
    }
    if dto.items.iter().any(|i| !i.found) {
        match official_catalog().await {
            Ok(entries) => {
                dto.official_status = "ok".into();
                for (slot, model) in dto.items.iter_mut().zip(models) {
                    if !slot.found {
                        *slot = item(model, "official", find_match(model, &entries));
                    }
                }
            }
            Err(message) => {
                dto.official_status = "failed".into();
                dto.official_message = Some(message);
            }
        }
    }
    dto
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry<'a>(entries: &'a [CatalogEntry], key: &str) -> &'a CatalogEntry {
        entries
            .iter()
            .find(|e| e.key == key)
            .unwrap_or_else(|| panic!("no entry {key}: {entries:?}"))
    }

    #[test]
    fn litellm_catalog_prices_are_per_million_and_units_follow_the_mode() {
        let catalog = json!({
            "sample_spec": {"input_cost_per_token": 9.0, "mode": "chat"},
            "gpt-4o": {
                "mode": "chat",
                "input_cost_per_token": 2.5e-06,
                "output_cost_per_token": 1e-05,
                "cache_read_input_token_cost": 1.25e-06
            },
            "claude-sonnet-4": {
                "mode": "chat",
                "input_cost_per_token": 3e-06,
                "output_cost_per_token": 1.5e-05,
                "cache_read_input_token_cost": 3e-07,
                "cache_creation_input_token_cost": 3.75e-06
            },
            "veo-3": {"mode": "video_generation", "output_cost_per_second": 0.4},
            "dall-e-3": {"mode": "image_generation", "output_cost_per_image": 0.04},
            "gpt-image-1": {"mode": "image_generation", "input_cost_per_token": 5e-06, "output_cost_per_token": 4e-05, "output_cost_per_image": 0.04},
            "whisper-1": {"mode": "audio_transcription", "input_cost_per_second": 0.0001},
            "no-price": {"mode": "chat", "max_tokens": 4096}
        });
        let entries = parse_catalog(&catalog);
        assert!(
            entries.iter().all(|e| e.key != "sample_spec" && e.key != "no-price"),
            "{entries:?}"
        );

        let gpt = entry(&entries, "gpt-4o");
        assert_eq!(
            (gpt.unit, gpt.input_usd_per_mtok, gpt.output_usd_per_mtok),
            (BillingUnit::Token, 2.5, 10.0)
        );
        assert_eq!(gpt.cache_read_usd_per_mtok, Some(1.25));
        let sonnet = entry(&entries, "claude-sonnet-4");
        assert_eq!(
            (sonnet.cache_read_usd_per_mtok, sonnet.cache_write_usd_per_mtok),
            (Some(0.3), Some(3.75))
        );

        let veo = entry(&entries, "veo-3");
        assert_eq!((veo.unit, veo.unit_price_usd), (BillingUnit::Second, 0.4));
        let dalle = entry(&entries, "dall-e-3");
        assert_eq!((dalle.unit, dalle.unit_price_usd), (BillingUnit::Image, 0.04));
        // An image model that bills tokens is a token model, whatever else it lists.
        assert_eq!(entry(&entries, "gpt-image-1").unit, BillingUnit::Token);
        assert_eq!(entry(&entries, "whisper-1").unit, BillingUnit::Second);
    }

    #[test]
    fn litellm_proxy_model_info_and_relay_console_shapes_parse() {
        let proxy = json!({"data": [
            {"model_name": "team-gpt", "litellm_params": {"model": "openai/gpt-4o"},
             "model_info": {"mode": "chat", "input_cost_per_token": 2e-06, "output_cost_per_token": 8e-06}}
        ]});
        let entries = parse_catalog(&proxy);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (
                entries[0].key.as_str(),
                entries[0].input_usd_per_mtok,
                entries[0].output_usd_per_mtok
            ),
            ("team-gpt", 2.0, 8.0)
        );

        // The shape a relay console (Joy MaaS) lists: cost per MILLION already.
        let relay = json!({"models": [{
            "modelName": "deepseek-v4-flash", "category": "chat", "vision": false,
            "pricing": {"inputCostPerMillion": 0.3, "outputCostPerMillion": 1.2,
                        "cacheReadCostPerMillion": 0.03, "cacheWriteCostPerMillion": 0}
        }]});
        let entries = parse_catalog(&relay);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!((e.input_usd_per_mtok, e.output_usd_per_mtok), (0.3, 1.2));
        assert_eq!(
            (e.cache_read_usd_per_mtok, e.cache_write_usd_per_mtok),
            (Some(0.03), Some(0.0))
        );
    }

    /// The exact shape the company's Joy MaaS gateway returns from
    /// `/api/admin/model-list` (captured 2026-11-20; prices here are made up).
    /// Image, speech and rerank models are priced per million tokens too, so
    /// every entry is a token price — and a full-width model name still parses.
    #[test]
    fn joy_maas_model_list_parses_including_media_and_full_width_names() {
        let list = json!({"models": [
            {"modelName": "deepseek-flash-latest", "backendNames": ["deepseek-flash-deepseek", "deepseek-v4-1-flash-volc"],
             "pricing": {"inputCostPerMillion": 0.3, "outputCostPerMillion": 1.18, "cacheReadCostPerMillion": 0.006, "cacheWriteCostPerMillion": 0.44999999999999996},
             "category": "文本模型", "maxContextWindow": 1048576, "vision": true},
            {"modelName": "ＧＬＭ-5-2-claude", "backendNames": ["glm-5-2"],
             "pricing": {"inputCostPerMillion": 1.0, "outputCostPerMillion": 4.0, "cacheReadCostPerMillion": 0.2, "cacheWriteCostPerMillion": 1.2},
             "category": "文本模型", "maxContextWindow": 1048576, "vision": false},
            {"modelName": "doubao-seedream-5-0-pro", "backendNames": ["seedream"],
             "pricing": {"inputCostPerMillion": 0, "outputCostPerMillion": 16.6666666667, "cacheReadCostPerMillion": 0, "cacheWriteCostPerMillion": 0},
             "category": "图片模型", "maxContextWindow": 0, "vision": false}
        ]});
        let entries = parse_catalog(&list);
        assert_eq!(entries.len(), 3);
        let flash = entry(&entries, "deepseek-flash-latest");
        assert_eq!(
            (flash.unit, flash.input_usd_per_mtok, flash.output_usd_per_mtok),
            (BillingUnit::Token, 0.3, 1.18)
        );
        // 0.44999999999999996 from the wire is tidied, not shown as float noise.
        assert_eq!(
            (flash.cache_read_usd_per_mtok, flash.cache_write_usd_per_mtok),
            (Some(0.006), Some(0.45))
        );
        assert!(entries.iter().any(|e| e.key == "ＧＬＭ-5-2-claude"));
        let image = entry(&entries, "doubao-seedream-5-0-pro");
        assert_eq!(
            (image.unit, image.input_usd_per_mtok, image.output_usd_per_mtok),
            (BillingUnit::Token, 0.0, 16.666667)
        );
        assert_eq!(
            image.cache_read_usd_per_mtok,
            Some(0.0),
            "an explicit zero cache price stays zero, not 'default'"
        );
        // The gateway's own alias is an exact match; a backend's name reaches the
        // same entry only loosely, and says so.
        assert!(find_match("deepseek-flash-latest", &entries).unwrap().exact);
        let via_backend = find_match("deepseek-v4-1-flash-volc", &entries).unwrap();
        assert_eq!(
            (via_backend.entry.key.as_str(), via_backend.exact),
            ("deepseek-flash-latest", false)
        );
    }

    /// A relay's alias models list the upstream models behind them; a member's
    /// channel often uses the upstream's own dated name. That matches through
    /// the backend name — and is flagged, never presented as exact.
    #[test]
    fn upstream_models_match_through_a_relays_backend_names() {
        let list = json!({"models": [
            {"modelName": "deepseek-flash-latest",
             "backendNames": ["deepseek-flash-deepseek", "deepseek-v4-1-flash-volc"],
             "pricing": {"inputCostPerMillion": 0.3, "outputCostPerMillion": 1.18}},
            {"modelName": "glm-latest", "backendNames": ["glm-5-3-zhipu"],
             "pricing": {"inputCostPerMillion": 1.1, "outputCostPerMillion": 3.9}}
        ]});
        let entries = parse_catalog(&list);
        let m = find_match("deepseek-v4-1-flash-260910", &entries).unwrap();
        assert_eq!((m.entry.key.as_str(), m.exact), ("deepseek-flash-latest", false));
        // Another model that merely shares leading words is still not matched.
        assert!(find_match("deepseek-v4-1-pro-260910", &entries).is_none());
        assert!(find_match("deepseek-v4-1", &entries).is_none());
    }

    #[test]
    fn snapshot_suffixes_and_prefixes_normalise_away() {
        assert_eq!(normalize_model_name("openai/GPT-4o-2024-08-06"), "gpt-4o");
        assert_eq!(normalize_model_name("claude-sonnet-4-20250514"), "claude-sonnet-4");
        assert_eq!(
            normalize_model_name("deepseek-v4-1-flash-261017"),
            "deepseek-v4-1-flash"
        );
        assert_eq!(normalize_model_name("vertex_ai/gemini-2.5-pro@001"), "gemini-2.5-pro");
        assert_eq!(normalize_model_name("qwen-plus-latest"), "qwen-plus");
        // Real version numbers survive.
        assert_eq!(normalize_model_name("gpt-4"), "gpt-4");
        assert_eq!(normalize_model_name("glm-4-5"), "glm-4-5");
    }

    #[test]
    fn matching_prefers_the_canonical_key_and_never_guesses_across_families() {
        let mk = |key: &str, input: f64| CatalogEntry {
            key: key.into(),
            input_usd_per_mtok: input,
            ..Default::default()
        };
        let entries = vec![
            mk("azure/gpt-4o", 5.0),
            mk("gpt-4o", 2.5),
            mk("openrouter/openai/gpt-4o", 9.0),
            mk("claude-sonnet-4-20250514", 3.0),
            mk("deepseek-chat", 0.27),
        ];
        let m = find_match("gpt-4o", &entries).unwrap();
        assert_eq!((m.entry.input_usd_per_mtok, m.exact), (2.5, true));
        // Same model behind a gateway prefix still lands on the canonical entry.
        assert_eq!(find_match("my-gw/gpt-4o", &entries).unwrap().entry.key, "gpt-4o");
        // A different snapshot of the same family: found, but flagged as not exact.
        let m = find_match("claude-sonnet-4-20251001", &entries).unwrap();
        assert_eq!((m.entry.key.as_str(), m.exact), ("claude-sonnet-4-20250514", false));
        // Only a third-party gateway lists it: found, but flagged as not exact.
        let gateway_only = vec![mk("aiml/dall-e-3", 0.0)];
        let m = find_match("dall-e-3", &gateway_only).unwrap();
        assert_eq!((m.entry.key.as_str(), m.exact), ("aiml/dall-e-3", false));
        // Different model that merely shares a prefix is NOT matched.
        assert!(find_match("gpt-4o-mini", &entries).is_none());
        assert!(find_match("deepseek-v4-1-flash-261017", &entries).is_none());
        assert!(find_match("   ", &entries).is_none());
    }

    #[test]
    fn fetch_urls_refuse_metadata_and_non_http_targets() {
        assert!(check_fetch_url("https://litellm.internal.example/support-models").is_ok());
        assert!(
            check_fetch_url("http://10.1.2.3:4000/model/info").is_ok(),
            "an internal gateway is the point"
        );
        assert!(check_fetch_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(check_fetch_url("http://metadata.google.internal/").is_err());
        assert!(check_fetch_url("file:///etc/passwd").is_err());
        assert!(check_fetch_url("not a url").is_err());
    }

    #[test]
    fn bare_tokens_become_bearer_but_full_schemes_pass_through() {
        assert_eq!(bearer("sk-abc"), "Bearer sk-abc");
        assert_eq!(bearer("Basic dXNlcjpwYXNz"), "Basic dXNlcjpwYXNz");
    }
}
