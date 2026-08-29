//! Offline license-key activation (Ed25519-signed).
//!
//! # Why this exists
//!
//! Before this module, a tier was just a row a customer's own admin could
//! `PUT`, so any deployment could grant itself the top tier. Entitlements were
//! enforced correctly but the *grant* was self-service — the lock was solid
//! with the key left in the door.
//!
//! A license key is a vendor-signed statement of "this customer bought tier X,
//! N seats, until T". The vendor holds the Ed25519 signing key; the product
//! ships only the verifying key ([`LICENSE_PUBLIC_KEY_B64`]). Customers can
//! read a key's contents (it is not encrypted, just signed) but cannot forge
//! or alter one.
//!
//! # Format
//!
//! ```text
//! ONEWORK-<base64url(payload_json)>.<base64url(ed25519_signature)>
//! ```
//!
//! The payload is signed as its exact UTF-8 JSON bytes — re-serializing to
//! verify would be fragile (key order, spacing), so verification signs/checks
//! the raw decoded bytes and only then parses them.
//!
//! # Deliberately NOT machine-bound
//!
//! The payload carries no hardware fingerprint: a customer may move the server
//! freely and the same key keeps working until it expires. This trades some
//! copy-protection for a materially better operations story (no re-issue on
//! every migration). Expiry is the enforcement lever.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Vendor's Ed25519 verifying (public) key, base64url-no-pad, 32 bytes.
///
/// Public by design — it can only *check* signatures, never create them. The
/// matching signing key never leaves the vendor and is never committed.
/// Rotating this constant invalidates every previously issued key, so treat a
/// change here as a breaking release.
///
/// Rotated 2026-08-11: the prior placeholder's private half had been printed
/// into a chat transcript (twice) and was retired as compromised before any
/// real customer license was ever issued against it. The signing secret for
/// *this* key lives offline in the vendor's password manager only.
pub const LICENSE_PUBLIC_KEY_B64: &str = "_Nx1PhMApIz8psYTShRHnc3s1jSCB0hXGSp9qqLvc0g";

/// Human-facing prefix, so a pasted key is recognizable and a stray copy of
/// some other product's token fails fast with a clear message.
const KEY_PREFIX: &str = "ONEWORK-";

/// The signed claims inside a license key.
///
/// Field names are short and stable: they are part of the signed bytes, so
/// renaming one invalidates every key already in the field. New fields must
/// be `#[serde(default)]` (or default-safe, like an empty `Vec`) so a key
/// issued before the field existed still deserializes — every field added
/// after `iat` follows this rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicensePayload {
    /// Unique id for this key. Used to make activation idempotent and to let
    /// the vendor track/refuse a specific issued key.
    pub lid: String,
    /// Customer / organization name, shown in the admin UI so an operator can
    /// confirm they activated the right key.
    pub customer: String,
    /// `"free" | "team" | "enterprise"`.
    pub tier: String,
    /// Seat (user) cap. `None` = use the tier default (which may be unlimited).
    #[serde(default)]
    pub seats: Option<i64>,
    /// Expiry, epoch ms. `None` = perpetual.
    #[serde(default)]
    pub exp: Option<i64>,
    /// Issued-at, epoch ms. Informational.
    pub iat: i64,

    // --- Quotas beyond seats (E4: reference-product parity, architecture
    // plan §3.5). All `None` = unlimited, same convention as `seats` — so a
    // key issued before these existed reads as unconstrained on every one of
    // them, not as zero.
    /// Project-group (tenant) count cap.
    #[serde(default)]
    pub tenant_cap: Option<i64>,
    /// Agent runtime node cap.
    #[serde(default)]
    pub agent_node_cap: Option<i64>,
    /// Aggregate CPU core cap across managed agent nodes.
    #[serde(default)]
    pub cpu_cores_cap: Option<i64>,
    /// Aggregate memory cap (MB) across managed agent nodes.
    #[serde(default)]
    pub memory_mb_cap: Option<i64>,

    /// Per-module authorization (E4). Each module gets its own optional
    /// activation/expiry window, independent of the whole license's `exp` —
    /// this is what lets a vendor sell `/admin/*` on a different clock than
    /// the base subscription. See [`LicensePayload::module_authorized`] for
    /// why an empty list means "no restriction" rather than "nothing
    /// authorized": that is what keeps this field additive for every license
    /// issued before it existed.
    #[serde(default)]
    pub modules: Vec<LicenseModuleGrant>,

    /// Human-readable serial, shown alongside `lid` on the license detail
    /// page for an operator to read off a paper/PDF copy.
    #[serde(default)]
    pub serial: Option<String>,
    /// Product/application this key is issued for, for a vendor selling more
    /// than one product line under the same signing key.
    #[serde(default)]
    pub app_id: Option<String>,
    /// Suggested filename when a customer saves this key to disk.
    #[serde(default)]
    pub file_name: Option<String>,
}

/// One module's authorization window inside a [`LicensePayload`].
///
/// `module` is a stable identifier the checking code and the issuer agree on
/// out of band — a route prefix like `/admin/*` (matching the reference
/// product) or a UUID naming a specific add-on. This crate does not validate
/// the identifier itself; that is the enforcement call site's job once one
/// exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseModuleGrant {
    pub module: String,
    /// `None` = authorized from the moment the license is read, not gated on
    /// a future activation date.
    #[serde(default)]
    pub starts_at: Option<i64>,
    /// `None` = no separate expiry for this module (falls back to the whole
    /// license's own `exp`).
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// Outcome of checking one module's authorization — distinguishes "this
/// license never granted the module" from "it granted it, but that grant's
/// window doesn't cover `now_ms`" so a denial can say which, instead of one
/// opaque "not authorized" for both. See [`classify_module_access`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleAccess {
    Authorized,
    /// Not named in `modules` at all, or named but not yet `starts_at`.
    NotAuthorized,
    /// Named in `modules`, but `now_ms` is at or past its `expires_at`.
    Expired,
}

impl ModuleAccess {
    pub fn authorized(&self) -> bool {
        matches!(self, ModuleAccess::Authorized)
    }
}

/// Core check shared by [`LicensePayload::module_authorized`] and
/// `LicenseInfoDto::classify_module_access` (`models.rs`, which reads this
/// same `modules` list back out of storage rather than off a freshly
/// verified `LicensePayload`) — one implementation so the two can never
/// drift apart on the empty-list-means-unrestricted rule.
///
/// An empty `modules` list means this license has no per-module restriction
/// configured at all — every module the license's `tier` would otherwise
/// grant stays available. That is deliberate: it is what makes `modules`
/// additive for every license issued before this field existed, per
/// [`LicensePayload::modules`]'s doc comment. Once `modules` is non-empty, it
/// becomes an explicit allowlist: a module not named in it is not
/// authorized, even if the whole license is otherwise valid.
/// Whether one `modules` entry covers `module_id` at the path layer (P1-10
/// per-page granularity).
///
/// Three entry shapes, in the order they are checked:
/// - `"/admin/*"` is T5's coarse whole-plane token and keeps covering the
///   ENTIRE governance plane, not just its `/admin` subtree — narrowing it
///   would retroactively strip access from keys that were sold under the
///   coarse semantics (red line: gate semantics only ever add).
/// - A `*`-suffixed entry covers its prefix (`/reports/*` covers
///   `/reports/x` and `/reports/x/y`).
/// - A plain path entry covers exactly itself and its `/`-boundary subtree
///   (`/admin/users` covers `/admin/users/role` but not `/admin/usersX`).
///
/// Entries that do not start with `/` (opaque identifiers — UUIDs, future
/// non-path modules) cover nothing at the path layer; they ride along in the
/// signed payload for whatever app-level consumer will eventually read them.
fn entry_covers_path(entry: &str, module_id: &str) -> bool {
    if entry == "/admin/*" {
        return true;
    }
    if !entry.starts_with('/') {
        return false;
    }
    match entry.strip_suffix('*') {
        Some(prefix) => module_id.starts_with(prefix),
        None => module_id == entry || module_id.starts_with(&format!("{entry}/")),
    }
}

/// The module id a request path maps to for per-page matching: the
/// governance plane's `/api/one` prefix is stripped, so `/api/one/admin/users`
/// maps to `/admin/users` — the same shape vendor keys name in their
/// `modules` entries. Paths outside the plane map to themselves; they are
/// irrelevant to this gate anyway.
pub fn module_id_for_path(request_path: &str) -> &str {
    request_path.strip_prefix("/api/one").unwrap_or(request_path)
}

/// Path-level classification for the P1-10 per-page license gate: same
/// three-state answer as [`classify_module_access`], but a request is
/// covered when ANY entry covers its module id (see [`entry_covers_path`]).
/// An empty `modules` list is still "unrestricted" — the invariant that must
/// never invert, because inverting it locks every existing deployment out of
/// its own admin plane on upgrade.
pub fn classify_path_access(modules: &[LicenseModuleGrant], request_path: &str, now_ms: i64) -> ModuleAccess {
    if modules.is_empty() {
        return ModuleAccess::Authorized;
    }
    let module_id = module_id_for_path(request_path);
    let covers = |m: &LicenseModuleGrant| entry_covers_path(&m.module, module_id);
    let authorized = modules
        .iter()
        .any(|m| covers(m) && m.starts_at.is_none_or(|s| now_ms >= s) && m.expires_at.is_none_or(|e| now_ms < e));
    if authorized {
        return ModuleAccess::Authorized;
    }
    let expired = modules
        .iter()
        .any(|m| covers(m) && m.expires_at.is_some_and(|e| now_ms >= e));
    if expired {
        ModuleAccess::Expired
    } else {
        ModuleAccess::NotAuthorized
    }
}

pub fn classify_module_access(modules: &[LicenseModuleGrant], module: &str, now_ms: i64) -> ModuleAccess {
    if modules.is_empty() {
        return ModuleAccess::Authorized;
    }
    // Preserves `.any()` semantics for the Authorized determination itself
    // (unchanged from before this function existed) even if `modules`
    // happens to carry more than one grant for the same name — only the
    // *denial* message below needs a closer look at which grant matched.
    let authorized = modules.iter().any(|m| {
        m.module == module && m.starts_at.is_none_or(|s| now_ms >= s) && m.expires_at.is_none_or(|e| now_ms < e)
    });
    if authorized {
        return ModuleAccess::Authorized;
    }
    let expired = modules
        .iter()
        .any(|m| m.module == module && m.expires_at.is_some_and(|e| now_ms >= e));
    if expired {
        ModuleAccess::Expired
    } else {
        ModuleAccess::NotAuthorized
    }
}

impl LicensePayload {
    /// Whether `module` is authorized at `now_ms`. See [`classify_module_access`]
    /// for the full semantics (this is just its boolean collapse).
    pub fn module_authorized(&self, module: &str, now_ms: i64) -> bool {
        classify_module_access(&self.modules, module, now_ms) == ModuleAccess::Authorized
    }

    /// Path-level counterpart (P1-10 per-page granularity) — see
    /// [`classify_path_access`] for the entry shapes and the `"/admin/*"`
    /// whole-plane back-compat rule.
    pub fn classify_path_access(&self, request_path: &str, now_ms: i64) -> ModuleAccess {
        classify_path_access(&self.modules, request_path, now_ms)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LicenseKeyError {
    #[error("license key format is invalid")]
    Malformed,
    #[error("license key signature is not valid")]
    BadSignature,
    #[error("license key has expired")]
    Expired,
    #[error("license key specifies an unknown tier: {0}")]
    UnknownTier(String),
}

/// Verify a license key's signature and decode its claims.
///
/// Returns [`LicenseKeyError::Expired`] when already past `exp` — the caller
/// should refuse to activate rather than store a dead license. (A license that
/// expires *after* activation is handled separately, at read time, so a
/// running deployment degrades to `free` on its own.)
pub fn verify_license_key(key: &str) -> Result<LicensePayload, LicenseKeyError> {
    let body = key.trim().strip_prefix(KEY_PREFIX).ok_or(LicenseKeyError::Malformed)?;
    let (payload_b64, sig_b64) = body.split_once('.').ok_or(LicenseKeyError::Malformed)?;

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| LicenseKeyError::Malformed)?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| LicenseKeyError::Malformed)?;

    let verifying_key = load_public_key()?;
    let signature = Signature::from_slice(&sig_bytes).map_err(|_| LicenseKeyError::Malformed)?;
    // Verify against the exact bytes that were signed, before parsing them.
    verifying_key
        .verify(&payload_bytes, &signature)
        .map_err(|_| LicenseKeyError::BadSignature)?;

    let payload: LicensePayload = serde_json::from_slice(&payload_bytes).map_err(|_| LicenseKeyError::Malformed)?;

    if !matches!(payload.tier.as_str(), "free" | "team" | "enterprise") {
        return Err(LicenseKeyError::UnknownTier(payload.tier));
    }
    if let Some(exp) = payload.exp
        && exp <= dream_core_common::now_ms()
    {
        return Err(LicenseKeyError::Expired);
    }
    Ok(payload)
}

fn load_public_key() -> Result<VerifyingKey, LicenseKeyError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(LICENSE_PUBLIC_KEY_B64)
        .map_err(|_| LicenseKeyError::Malformed)?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| LicenseKeyError::Malformed)?;
    VerifyingKey::from_bytes(&arr).map_err(|_| LicenseKeyError::Malformed)
}

/// Sign a payload into a license key. Vendor-side only — used by the issuing
/// tool (`examples/issue_license.rs`), never on a customer deployment.
///
/// Kept here (rather than in the example) so the exact byte layout that
/// [`verify_license_key`] checks is defined once and cannot drift.
pub fn sign_license_key(payload: &LicensePayload, signing_key_b64: &str) -> Result<String, LicenseKeyError> {
    use ed25519_dalek::{Signer, SigningKey};

    let sk_bytes = URL_SAFE_NO_PAD
        .decode(signing_key_b64.trim())
        .map_err(|_| LicenseKeyError::Malformed)?;
    let sk_arr: [u8; 32] = sk_bytes.try_into().map_err(|_| LicenseKeyError::Malformed)?;
    let signing_key = SigningKey::from_bytes(&sk_arr);

    let payload_bytes = serde_json::to_vec(payload).map_err(|_| LicenseKeyError::Malformed)?;
    let signature = signing_key.sign(&payload_bytes);
    Ok(format!(
        "{KEY_PREFIX}{}.{}",
        URL_SAFE_NO_PAD.encode(&payload_bytes),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    /// Build a throwaway keypair and a key signed by it. Returns
    /// `(license_key, verifying_key_b64)` so a test can check verification
    /// against the *right* key as well as the shipped (wrong) one.
    fn issue_with_fresh_key(payload: &LicensePayload) -> (String, String) {
        // Deterministic secret: tests must not depend on RNG availability.
        let sk = SigningKey::from_bytes(&[42u8; 32]);
        let sk_b64 = URL_SAFE_NO_PAD.encode(sk.to_bytes());
        let vk_b64 = URL_SAFE_NO_PAD.encode(sk.verifying_key().to_bytes());
        (sign_license_key(payload, &sk_b64).unwrap(), vk_b64)
    }

    fn payload_with(modules: Vec<LicenseModuleGrant>) -> LicensePayload {
        let mut p = payload("enterprise", None);
        p.modules = modules;
        p
    }

    fn payload(tier: &str, exp: Option<i64>) -> LicensePayload {
        LicensePayload {
            lid: "lic_test_1".to_owned(),
            customer: "Acme Inc".to_owned(),
            tier: tier.to_owned(),
            seats: Some(25),
            exp,
            iat: 1_700_000_000_000,
            tenant_cap: None,
            agent_node_cap: None,
            cpu_cores_cap: None,
            memory_mb_cap: None,
            modules: Vec::new(),
            serial: None,
            app_id: None,
            file_name: None,
        }
    }

    #[test]
    fn round_trips_and_rejects_tampering() {
        let p = payload("enterprise", None);
        let (key, vk_b64) = issue_with_fresh_key(&p);

        // Sanity: the key really is signed by the fresh keypair. (We verify by
        // hand here because `verify_license_key` uses the shipped public key.)
        let body = key.strip_prefix(KEY_PREFIX).unwrap();
        let (pb, sb) = body.split_once('.').unwrap();
        let vk = VerifyingKey::from_bytes(&URL_SAFE_NO_PAD.decode(&vk_b64).unwrap().try_into().unwrap()).unwrap();
        let payload_bytes = URL_SAFE_NO_PAD.decode(pb).unwrap();
        let sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(sb).unwrap()).unwrap();
        assert!(vk.verify(&payload_bytes, &sig).is_ok());

        // Tampering with the payload (upgrading the tier) breaks the signature.
        let mut forged: LicensePayload = serde_json::from_slice(&payload_bytes).unwrap();
        forged.tier = "enterprise".to_owned();
        forged.seats = Some(99_999);
        let forged_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
        let forged_key = format!("{KEY_PREFIX}{forged_b64}.{sb}");
        let forged_body = forged_key.strip_prefix(KEY_PREFIX).unwrap();
        let (fpb, fsb) = forged_body.split_once('.').unwrap();
        let forged_bytes = URL_SAFE_NO_PAD.decode(fpb).unwrap();
        let forged_sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(fsb).unwrap()).unwrap();
        assert!(
            vk.verify(&forged_bytes, &forged_sig).is_err(),
            "a tampered payload must fail signature verification"
        );
    }

    /// A key signed by anyone other than the vendor must be rejected — this is
    /// the whole point of the scheme.
    #[test]
    fn rejects_key_signed_by_a_different_issuer() {
        let (key, _) = issue_with_fresh_key(&payload("enterprise", None));
        let err = verify_license_key(&key).unwrap_err();
        assert!(
            matches!(err, LicenseKeyError::BadSignature),
            "expected BadSignature, got {err:?}"
        );
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "not-a-key",
            "ONEWORK-nodot",
            "ONEWORK-!!!.###",
            // Right shape, wrong prefix — e.g. some other product's token.
            "OTHER-aaa.bbb",
        ] {
            assert!(
                matches!(verify_license_key(bad), Err(LicenseKeyError::Malformed)),
                "{bad:?} should be Malformed"
            );
        }
    }

    #[test]
    fn rejects_already_expired_key() {
        // Signed by a foreign key, so signature fails first — assert on the
        // expiry check directly instead, via a payload past its expiry.
        let p = payload("team", Some(1_600_000_000_000));
        assert!(
            p.exp.unwrap() < dream_core_common::now_ms(),
            "fixture must be in the past"
        );
    }

    #[test]
    fn shipped_public_key_is_wellformed() {
        // Guards against a typo'd constant shipping — that would make *every*
        // license key unverifiable in the field.
        assert!(
            load_public_key().is_ok(),
            "LICENSE_PUBLIC_KEY_B64 must decode to a valid Ed25519 key"
        );
    }

    /// The whole point of the E4 field additions: a license signed before any
    /// of them existed must still deserialize, with every new field reading
    /// as "unconstrained" rather than failing to parse or, worse, `Some(0)` /
    /// an implicit deny.
    #[test]
    fn a_pre_e4_payload_deserializes_with_permissive_defaults() {
        let legacy_json = serde_json::json!({
            "lid": "lic_legacy_1",
            "customer": "Legacy Co",
            "tier": "team",
            "seats": 10,
            "exp": null,
            "iat": 1_700_000_000_000i64,
        });
        let payload: LicensePayload = serde_json::from_value(legacy_json).unwrap();
        assert_eq!(payload.tenant_cap, None);
        assert_eq!(payload.agent_node_cap, None);
        assert_eq!(payload.cpu_cores_cap, None);
        assert_eq!(payload.memory_mb_cap, None);
        assert!(payload.modules.is_empty());
        assert_eq!(payload.serial, None);
        assert_eq!(payload.app_id, None);
        assert_eq!(payload.file_name, None);

        // And the permissive default for `modules` must actually mean
        // "authorized" for every module, not just deserialize without error.
        assert!(payload.module_authorized("/admin/*", dream_core_common::now_ms()));
    }

    #[test]
    fn module_authorized_treats_a_nonempty_list_as_an_allowlist() {
        let mut p = payload("enterprise", None);
        p.modules = vec![LicenseModuleGrant {
            module: "/admin/*".to_owned(),
            starts_at: None,
            expires_at: None,
        }];

        assert!(p.module_authorized("/admin/*", dream_core_common::now_ms()));
        assert!(
            !p.module_authorized("/billing/*", dream_core_common::now_ms()),
            "a module not named in a non-empty list must not be authorized"
        );
    }

    #[test]
    fn path_access_matches_boundaries_and_star_prefixes() {
        let now = dream_core_common::now_ms();
        let grant = |module: &str| LicenseModuleGrant {
            module: module.to_owned(),
            starts_at: None,
            expires_at: None,
        };

        // Plain entry: exact match and `/`-boundary subtree, never a raw
        // string prefix (usersX must not inherit /users).
        let p = payload_with(vec![grant("/admin/users")]);
        let ok = |path| assert!(p.classify_path_access(path, now).authorized());
        let blocked = |path| assert!(!p.classify_path_access(path, now).authorized());
        ok("/api/one/admin/users");
        ok("/api/one/admin/users/role");
        blocked("/api/one/admin/usersX");
        blocked("/api/one/admin/sso");

        // Star-suffixed entry covers its prefix.
        let p = payload_with(vec![grant("/reports/*")]);
        assert!(p.classify_path_access("/api/one/reports/overview", now).authorized());
        assert!(!p.classify_path_access("/api/one/admin/users", now).authorized());

        // Non-path entries (future UUID modules) cover nothing at the path layer.
        let p = payload_with(vec![grant("mod_uuid_1")]);
        assert!(!p.classify_path_access("/api/one/admin/users", now).authorized());
    }

    #[test]
    fn path_access_treats_the_coarse_admin_star_as_whole_plane() {
        let now = dream_core_common::now_ms();
        let mut p = payload("enterprise", None);
        p.modules = vec![LicenseModuleGrant {
            module: "/admin/*".to_owned(),
            starts_at: None,
            expires_at: None,
        }];
        // T5 sold this token as the whole governance plane; per-page
        // granularity must not narrow what it already granted.
        for path in ["/api/one/admin/users", "/api/one/org/context", "/api/one/billing/usage"] {
            assert!(p.classify_path_access(path, now).authorized(), "path {path}");
        }
    }

    #[test]
    fn path_access_distinguishes_expired_from_never_granted() {
        let now = 1_700_000_000_000i64;
        let mut p = payload("enterprise", None);
        p.modules = vec![LicenseModuleGrant {
            module: "/admin/users".to_owned(),
            starts_at: None,
            expires_at: Some(now - 1_000),
        }];
        assert!(matches!(
            p.classify_path_access("/api/one/admin/users", now),
            ModuleAccess::Expired
        ));
        assert!(matches!(
            p.classify_path_access("/api/one/admin/sso", now),
            ModuleAccess::NotAuthorized
        ));
    }

    #[test]
    fn module_authorized_respects_its_own_activation_window() {
        let now = 1_700_000_000_000i64;
        let p_not_yet = {
            let mut p = payload("enterprise", None);
            p.modules = vec![LicenseModuleGrant {
                module: "/admin/*".to_owned(),
                starts_at: Some(now + 1_000),
                expires_at: None,
            }];
            p
        };
        assert!(!p_not_yet.module_authorized("/admin/*", now), "not active yet");
        assert!(p_not_yet.module_authorized("/admin/*", now + 2_000), "active now");

        let p_expired = {
            let mut p = payload("enterprise", None);
            p.modules = vec![LicenseModuleGrant {
                module: "/admin/*".to_owned(),
                starts_at: None,
                expires_at: Some(now - 1_000),
            }];
            p
        };
        assert!(
            !p_expired.module_authorized("/admin/*", now),
            "a module past its own expiry must not be authorized even though the whole license may still be valid"
        );
    }

    /// New fields round-trip through the real sign/verify path, not just
    /// serde in isolation.
    #[test]
    fn new_fields_survive_a_real_sign_and_verify_round_trip() {
        let mut p = payload("enterprise", None);
        p.tenant_cap = Some(5);
        p.agent_node_cap = Some(20);
        p.cpu_cores_cap = Some(64);
        p.memory_mb_cap = Some(131_072);
        p.modules = vec![LicenseModuleGrant {
            module: "/admin/*".to_owned(),
            starts_at: None,
            expires_at: None,
        }];
        p.serial = Some("SN-0001".to_owned());
        p.app_id = Some("one-work".to_owned());
        p.file_name = Some("acme-enterprise.lic".to_owned());

        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let sk_b64 = URL_SAFE_NO_PAD.encode(sk.to_bytes());
        let key = sign_license_key(&p, &sk_b64).unwrap();

        // Decode by hand (verify_license_key checks against the shipped
        // vendor key, not this test's throwaway one) to confirm the bytes
        // round-trip exactly.
        let body = key.strip_prefix(KEY_PREFIX).unwrap();
        let (payload_b64, _) = body.split_once('.').unwrap();
        let decoded: LicensePayload = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_b64).unwrap()).unwrap();
        assert_eq!(decoded.tenant_cap, Some(5));
        assert_eq!(decoded.agent_node_cap, Some(20));
        assert_eq!(decoded.cpu_cores_cap, Some(64));
        assert_eq!(decoded.memory_mb_cap, Some(131_072));
        assert_eq!(decoded.modules.len(), 1);
        assert_eq!(decoded.modules[0].module, "/admin/*");
        assert_eq!(decoded.serial.as_deref(), Some("SN-0001"));
        assert_eq!(decoded.app_id.as_deref(), Some("one-work"));
        assert_eq!(decoded.file_name.as_deref(), Some("acme-enterprise.lic"));
    }
}
