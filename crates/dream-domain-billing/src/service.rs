//! Billing-plane business logic: license tier, seat enforcement, usage
//! metering, and the (stubbed) payment provider.
//!
//! License attaches to an SSO company (`one_enterprises`). A user's company is
//! resolved from `one_enterprise_members`. Personal / standalone users have no
//! company row → not in the billing system (every check is permissive, seats
//! uncounted, usage recorded with a NULL enterprise). This is the red line.

use std::sync::Arc;

use dream_core_common::license::{
    Feature, Tier, estimate_cost_micros, estimate_media_cost_micros, tier_allows, tier_seat_limit,
};
use dream_core_common::{generate_prefixed_id, now_ms};
use sqlx::SqlitePool;

use crate::error::BillingError;
use crate::models::{
    AgentSessionDto, AgentSessionPageDto, CheckoutResultDto, DepartmentBudgetDto, EnterpriseReportDto, EntitlementDto,
    LatencyTrendPointDto, LicenseInfoDto, LlmCallDto, LlmCallPageDto, MediaAssetDto, PlanDto, TopUserDto,
    UsageBucketDto, UsageEventDto, UsageEventPageDto, UsageSummaryDto,
};

/// Pluggable payment backend. The default `ManualBillingProvider` is a stub
/// (no real payments); a real Stripe/… provider can drop in later without
/// touching callers.
pub trait BillingProvider: Send + Sync {
    /// Begin a checkout for `target_tier`. The stub returns a `manual` result.
    fn create_checkout(&self, enterprise_id: &str, target_tier: &str) -> CheckoutResultDto;
    fn name(&self) -> &'static str;
}

/// No payment provider configured: upgrades are provisioned manually by an
/// admin (`PUT /tier`). Structurally present so real payment is a drop-in.
pub struct ManualBillingProvider;

impl BillingProvider for ManualBillingProvider {
    fn create_checkout(&self, _enterprise_id: &str, _target_tier: &str) -> CheckoutResultDto {
        CheckoutResultDto {
            status: "manual".to_owned(),
            message: "No payment provider is configured. Contact your administrator to provision this plan.".to_owned(),
            checkout_url: None,
        }
    }

    fn name(&self) -> &'static str {
        "manual"
    }
}

/// One completed MODEL CALL, as reported for the per-call LLM trace (P2-5).
/// Finer than one agent turn: a single turn can contain several model calls
/// (tool rounds, vision delegates, error retries), each billed and timed on
/// its own.
///
/// A struct rather than a positional argument list, same reason as
/// [`MediaUsage`]: nine mostly-primitive values, several of them `Option`al,
/// so a mis-ordered call site would compile and silently mis-attribute the
/// trace.
///
/// Tenancy is deliberately NOT a field: the hot path (ai-agent) knows only the
/// `user_id` — resolving the enterprise (and dropping personal users) is
/// billing's job, done inside [`BillingService::record_llm_call`]. Same split
/// as `record_media_asset`. `id` and `created_at` are stamped here too.
#[derive(Debug, Clone)]
pub struct NewLlmCall {
    pub user_id: String,
    pub conversation_id: Option<String>,
    pub model: Option<String>,
    /// Where the call's shape came from: `acp` / `dream_engine` / `direct_cli`.
    pub provider: Option<String>,
    /// The tool round this call belonged to, when it was one.
    pub tool_name: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub duration_ms: Option<i64>,
    /// `None` = the call succeeded; otherwise the failure reason. Failed calls
    /// are recorded like successful ones — a retry storm is exactly what this
    /// trace exists to expose.
    pub error: Option<String>,
}

/// How long per-call trace rows are kept, in days. Matches the 30-day default
/// window every other billing dashboard query uses (`THIRTY_DAYS_MS` in the
/// routes, `BUDGET_WINDOW_MS` here): an admin debugging "what happened last
/// week" is the use case, and anything older is recoverable neither from the
/// per-turn aggregate nor from the provider console, but the raw rows are
/// high-volume enough that an unbounded table would grow into the deployment's
/// largest table for no operational benefit. Purge is explicit
/// (`purge_llm_calls_older_than` / the admin endpoint), not a background
/// sweeper — scheduling is the wiring layer's decision.
pub const LLM_CALL_RETENTION_DAYS: i64 = 30;

/// P1-3: minimum number of measured `duration_ms` rows below which the
/// enterprise report reports NO percentile at all. A distribution computed
/// from two or three calls is a statistic in name only — it would render as a
/// stable-looking latency the very next day's burst disproves. Honest gap:
/// `null` ("not enough data") beats a confidently wrong number.
pub const MIN_LATENCY_SAMPLES: i64 = 10;

#[derive(Clone)]
pub struct BillingService {
    pool: SqlitePool,
    provider: Arc<dyn BillingProvider>,
}

/// A stored license row (absent → free defaults).
struct License {
    tier: Tier,
    seat_limit: Option<i64>,
    expires_at: Option<i64>,
    /// Rolling-30-day estimated-cost budget in USD-micros; `None` = no cap (P1-2).
    cost_cap_micros: Option<i64>,
    /// Allowed model names; empty = all allowed (P1-2).
    allowed_models: Vec<String>,
}

/// Rolling budget window (P1-2): 30 days.
const BUDGET_WINDOW_MS: i64 = 30 * 24 * 3600 * 1000;

/// One completed media generation, as reported for metering.
///
/// A struct rather than a positional argument list: these are seven mostly-
/// primitive values, several of them `i64`, so a mis-ordered call site would
/// compile and silently meter the wrong thing.
#[derive(Debug, Clone, Copy)]
pub struct MediaUsage<'a> {
    pub user_id: &'a str,
    /// `"image"` or `"video"` — decides whether duration participates in cost.
    pub kind: &'a str,
    pub model: &'a str,
    /// Number of assets actually produced (not requested).
    pub count: i64,
    /// Video only; ignored for images.
    pub duration_seconds: i64,
    /// The user's own price for this model, when they entered one. Overrides the
    /// built-in rate table, which is a coarse illustration next to the contract
    /// they are actually billed under.
    pub unit_price_micros: Option<i64>,
    /// Where the generation happened. Attribution, not content — it is what lets
    /// an admin follow a charge back to somewhere. Optional because a caller may
    /// genuinely not have one.
    pub conversation_id: Option<&'a str>,
}

/// T8 ledger search filters. A named struct (not positional params) because
/// there are more than clippy's `too_many_arguments` threshold once
/// `enterprise_id` and `&self` are added, and because most of these are the
/// same primitive `Option<&str>` shape — position-mixups would compile.
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaAssetFilters<'a> {
    pub kind: Option<&'a str>,
    pub model: Option<&'a str>,
    pub user_id: Option<&'a str>,
    pub since: Option<i64>,
    /// Only meaningful when the company has opted into prompt retention —
    /// see `list_media_assets`'s doc comment for why no special-casing is
    /// needed when it hasn't.
    pub prompt_contains: Option<&'a str>,
    /// Defaults to 200, clamped to [1, 1000] — this is a search UI, not an
    /// unbounded export.
    pub limit: Option<i64>,
}

/// Raw row shape for `list_media_assets`, before the enterprise-scoped filter
/// context is folded away — `sqlx::FromRow` needs a concrete struct, and
/// `enterprise_id` itself is not projected back (it's already the filter).
#[derive(sqlx::FromRow)]
struct MediaAssetRow {
    id: String,
    user_id: String,
    department_id: Option<String>,
    conversation_id: Option<String>,
    kind: String,
    model: Option<String>,
    file_path: String,
    prompt: Option<String>,
    created_at: i64,
}

impl MediaAssetRow {
    fn into_dto(self) -> MediaAssetDto {
        MediaAssetDto {
            id: self.id,
            user_id: self.user_id,
            department_id: self.department_id,
            conversation_id: self.conversation_id,
            kind: self.kind,
            model: self.model,
            file_path: self.file_path,
            prompt: self.prompt,
            created_at: self.created_at,
        }
    }
}

/// Ordering for "is this an upgrade?". Kept local rather than deriving `Ord` on
/// `Tier` in dream-common, because tier ordering is a *billing* policy, not an
/// intrinsic property of the enum.
fn tier_rank(tier: Tier) -> u8 {
    match tier {
        Tier::Free => 0,
        Tier::Team => 1,
        Tier::Enterprise => 2,
    }
}

/// Parse the stored `allowed_models` JSON array; malformed / null → empty
/// (= all models allowed).
fn parse_allowed_models(json: Option<&str>) -> Vec<String> {
    json.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

impl BillingService {
    pub fn new(pool: SqlitePool, provider: Arc<dyn BillingProvider>) -> Self {
        Self { pool, provider }
    }

    /// The caller's SSO company, or `None` for personal / standalone users
    /// (who are outside the billing system entirely).
    pub async fn resolve_enterprise_id(&self, user_id: &str) -> Result<Option<String>, BillingError> {
        let row: Option<String> =
            sqlx::query_scalar("SELECT enterprise_id FROM one_enterprise_members WHERE user_id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or(None);
        Ok(row)
    }

    /// Whether the caller holds an ACTIVE (governed, billable) seat, as
    /// opposed to no company row at all, or a `pending` row created when they
    /// logged in while the plan's seat cap was already full (T6-4).
    ///
    /// This is deliberately a separate query from `resolve_enterprise_id`
    /// rather than folding seat_status into its return: that function's
    /// `None` means "personal user, skip every check" everywhere it is called,
    /// and a pending member is the opposite of that — they DO belong to a
    /// company, and must be denied, not waved through. Reusing `None` for both
    /// would silently restore the exact bug this column exists to close.
    async fn has_active_seat(&self, user_id: &str) -> Result<bool, BillingError> {
        let status: Option<String> =
            sqlx::query_scalar("SELECT seat_status FROM one_enterprise_members WHERE user_id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or(None);
        // No row → not a company member at all, handled by `resolve_enterprise_id`
        // returning `None` upstream; this function is only consulted once a
        // caller already has `Some(enterprise_id)`.
        Ok(status.as_deref() == Some("active"))
    }

    /// The caller's CURRENT department (T7), or `None` if unassigned. Reads
    /// `one_user_org.department_id` — owned by one-org, same cross-crate raw-
    /// SQL idiom this file already uses for `one_enterprise_members` — and is
    /// tolerant of the table being absent (standalone/personal builds, or a
    /// deployment where P2-3 departments were never migrated).
    async fn resolve_department_id(&self, user_id: &str) -> Result<Option<String>, BillingError> {
        // Double `Option`: the outer one is "does the membership row exist",
        // the inner one is the column's own nullability (unassigned). Only
        // the outer layer is truly "value absent" — collapsing both without
        // this would need the column decode to fail for a NULL department_id,
        // which is not an error, it is the overwhelmingly common case.
        let row: Option<Option<String>> =
            sqlx::query_scalar("SELECT department_id FROM one_user_org WHERE user_id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or(None);
        Ok(row.flatten())
    }

    async fn license_of(&self, enterprise_id: &str) -> Result<License, BillingError> {
        // tier, seat_limit, expires_at, cost_cap_micros, allowed_models(json)
        type LicenseRow = (String, Option<i64>, Option<i64>, Option<i64>, Option<String>);
        let row: Option<LicenseRow> = sqlx::query_as(
            "SELECT tier, seat_limit, expires_at, monthly_cost_cap_micros, allowed_models \
             FROM one_enterprise_license WHERE enterprise_id = ?",
        )
        .bind(enterprise_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some((tier, seat_limit, expires_at, cost_cap_micros, allowed_models_json)) => {
                // Expiry is enforced here, at the single read point every gate
                // funnels through, so a lapsed license degrades everywhere at
                // once without a background job. The row is left untouched: the
                // admin UI still shows what was bought and when it ran out, and
                // renewing re-activates it without losing history.
                let expired = expires_at.is_some_and(|exp| exp <= dream_core_common::now_ms());
                License {
                    tier: if expired { Tier::Free } else { Tier::parse(&tier) },
                    // A lapsed license also loses its seat override, otherwise
                    // an expired enterprise plan would keep an unlimited cap.
                    seat_limit: if expired { None } else { seat_limit },
                    expires_at,
                    cost_cap_micros,
                    allowed_models: parse_allowed_models(allowed_models_json.as_deref()),
                }
            }
            // No row → a company created before it was licensed, or an unknown
            // id: default to the entry tier (least privilege).
            None => License {
                tier: Tier::Free,
                seat_limit: None,
                expires_at: None,
                cost_cap_micros: None,
                allowed_models: Vec::new(),
            },
        })
    }

    /// Effective seat cap: explicit override, else the tier default. `None` =
    /// unlimited.
    fn effective_seat_limit(license: &License) -> Option<i64> {
        license
            .seat_limit
            .or_else(|| tier_seat_limit(license.tier).map(|n| n as i64))
    }

    /// ACTIVE seats only — what `seat_limit` caps. See `PlanDto::seat_used`.
    async fn seat_used(&self, enterprise_id: &str) -> Result<i64, BillingError> {
        let used: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM one_enterprise_members WHERE enterprise_id = ? AND seat_status = 'active'",
        )
        .bind(enterprise_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        Ok(used)
    }

    /// Members waiting on a seat. See `PlanDto::seat_pending`.
    async fn seat_pending(&self, enterprise_id: &str) -> Result<i64, BillingError> {
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM one_enterprise_members WHERE enterprise_id = ? AND seat_status = 'pending'",
        )
        .bind(enterprise_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        Ok(pending)
    }

    /// Whether the company can take one more member under its plan. Companies
    /// with an unlimited tier — and the personal case (`None` enterprise) —
    /// always can.
    pub async fn can_add_seat(&self, enterprise_id: Option<&str>) -> Result<bool, BillingError> {
        let Some(eid) = enterprise_id else {
            return Ok(true);
        };
        let license = self.license_of(eid).await?;
        match Self::effective_seat_limit(&license) {
            None => Ok(true),
            Some(limit) => Ok(self.seat_used(eid).await? < limit),
        }
    }

    /// Whether `feature` is included in the company's plan. Personal (`None`
    /// enterprise) is always allowed — the red line.
    pub async fn entitlement(&self, enterprise_id: Option<&str>, feature: Feature) -> Result<bool, BillingError> {
        let Some(eid) = enterprise_id else {
            return Ok(true);
        };
        let license = self.license_of(eid).await?;
        Ok(tier_allows(license.tier, feature))
    }

    /// Downgrade-only tier change (self-service).
    ///
    /// A customer admin may *drop* to a cheaper tier (e.g. to free up an
    /// entitlement they are not using) but may never raise one — an upgrade
    /// must come from a vendor-signed license via [`Self::activate_license`].
    /// Without this asymmetry the whole licensing scheme is decorative: the
    /// gates are enforced correctly, but anyone could grant themselves the top
    /// tier. Raising a tier here returns [`BillingError::UpgradeRequiresLicense`].
    pub async fn set_tier(&self, enterprise_id: &str, tier: Tier, seat_limit: Option<i64>) -> Result<(), BillingError> {
        let current = self.license_of(enterprise_id).await?;
        if tier_rank(tier) > tier_rank(current.tier) {
            return Err(BillingError::UpgradeRequiresLicense);
        }
        // A downgrade also clears any license expiry/seat override: the plan is
        // now whatever the admin chose, not what a (possibly still-valid) key
        // said. Re-activating the key restores it.
        sqlx::query(
            "INSERT INTO one_enterprise_license (enterprise_id, tier, seat_limit, expires_at, updated_at) \
             VALUES (?, ?, ?, NULL, ?) \
             ON CONFLICT(enterprise_id) DO UPDATE SET tier = excluded.tier, seat_limit = excluded.seat_limit, \
                 expires_at = NULL, updated_at = excluded.updated_at",
        )
        .bind(enterprise_id)
        .bind(tier.as_str())
        .bind(seat_limit)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Activate a vendor-signed license key: the only path that can *raise* a
    /// tier. Verification is offline (Ed25519 against the built-in public key)
    /// so an air-gapped deployment can be licensed.
    ///
    /// Idempotent by the key's `lid` claim — re-pasting the same key refreshes
    /// the entitlement without stacking activation rows.
    pub async fn activate_license(
        &self,
        enterprise_id: &str,
        license_key: &str,
        activated_by: &str,
    ) -> Result<crate::license_key::LicensePayload, BillingError> {
        let payload = crate::license_key::verify_license_key(license_key)?;

        // Re-serialized rather than storing the raw signed payload bytes: this
        // table is a read model for the admin UI, not a re-verification
        // source — `verify_license_key` already ran above, and a raw copy
        // would need its own tamper story once it left the signed envelope.
        let modules_json =
            serde_json::to_string(&payload.modules).map_err(|e| BillingError::Internal(e.to_string()))?;

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO one_license_activation \
                 (license_id, enterprise_id, customer, tier, seats, expires_at, issued_at, activated_at, activated_by, \
                  tenant_cap, agent_node_cap, cpu_cores_cap, memory_mb_cap, modules, serial, app_id, file_name) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(license_id) DO UPDATE SET enterprise_id = excluded.enterprise_id, \
                 activated_at = excluded.activated_at, activated_by = excluded.activated_by, \
                 tenant_cap = excluded.tenant_cap, agent_node_cap = excluded.agent_node_cap, \
                 cpu_cores_cap = excluded.cpu_cores_cap, memory_mb_cap = excluded.memory_mb_cap, \
                 modules = excluded.modules, serial = excluded.serial, app_id = excluded.app_id, \
                 file_name = excluded.file_name",
        )
        .bind(&payload.lid)
        .bind(enterprise_id)
        .bind(&payload.customer)
        .bind(&payload.tier)
        .bind(payload.seats)
        .bind(payload.exp)
        .bind(payload.iat)
        .bind(now_ms())
        .bind(activated_by)
        .bind(payload.tenant_cap)
        .bind(payload.agent_node_cap)
        .bind(payload.cpu_cores_cap)
        .bind(payload.memory_mb_cap)
        .bind(&modules_json)
        .bind(&payload.serial)
        .bind(&payload.app_id)
        .bind(&payload.file_name)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO one_enterprise_license (enterprise_id, tier, seat_limit, expires_at, updated_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(enterprise_id) DO UPDATE SET tier = excluded.tier, seat_limit = excluded.seat_limit, \
                 expires_at = excluded.expires_at, updated_at = excluded.updated_at",
        )
        .bind(enterprise_id)
        .bind(&payload.tier)
        .bind(payload.seats)
        .bind(payload.exp)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        tracing::info!(
            enterprise_id,
            license_id = %payload.lid,
            tier = %payload.tier,
            "license activated"
        );
        Ok(payload)
    }

    /// The license currently backing this company's entitlements, if any was
    /// ever activated. Shown in the admin UI so an operator can see what was
    /// bought, for whom, and when it lapses.
    pub async fn active_license(&self, enterprise_id: &str) -> Result<Option<LicenseInfoDto>, BillingError> {
        #[allow(clippy::type_complexity)]
        type Row = (
            String,
            String,
            String,
            Option<i64>,
            Option<i64>,
            i64,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT license_id, customer, tier, seats, expires_at, activated_at, \
                    tenant_cap, agent_node_cap, cpu_cores_cap, memory_mb_cap, modules, serial, app_id, file_name \
             FROM one_license_activation WHERE enterprise_id = ? ORDER BY activated_at DESC LIMIT 1",
        )
        .bind(enterprise_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(
                license_id,
                customer,
                tier,
                seats,
                expires_at,
                activated_at,
                tenant_cap,
                agent_node_cap,
                cpu_cores_cap,
                memory_mb_cap,
                modules_json,
                serial,
                app_id,
                file_name,
            )| LicenseInfoDto {
                license_id,
                customer,
                tier,
                seats,
                expires_at,
                activated_at,
                expired: expires_at.is_some_and(|e| e <= now_ms()),
                tenant_cap,
                agent_node_cap,
                cpu_cores_cap,
                memory_mb_cap,
                // A row written before billing_006 (or a corrupt value —
                // neither should ever block reading the rest of the license)
                // falls back to "no per-module restriction configured",
                // same permissive default as an absent field.
                modules: serde_json::from_str(&modules_json).unwrap_or_default(),
                serial,
                app_id,
                file_name,
            },
        ))
    }

    /// Set the model-control policy (P1-2): rolling-30-day spend cap
    /// (USD-micros; `None` = no cap) and allowed model list (`None`/empty = all
    /// allowed). Billing-admin path.
    pub async fn set_model_control(
        &self,
        enterprise_id: &str,
        cost_cap_micros: Option<i64>,
        allowed_models: &[String],
    ) -> Result<(), BillingError> {
        let allowed_json = if allowed_models.is_empty() {
            None
        } else {
            Some(serde_json::to_string(allowed_models).unwrap_or_else(|_| "[]".to_owned()))
        };
        // Upsert onto the (existing or default) license row.
        sqlx::query(
            "INSERT INTO one_enterprise_license (enterprise_id, tier, monthly_cost_cap_micros, allowed_models, updated_at) \
             VALUES (?, 'free', ?, ?, ?) \
             ON CONFLICT(enterprise_id) DO UPDATE SET monthly_cost_cap_micros = excluded.monthly_cost_cap_micros, \
                 allowed_models = excluded.allowed_models, updated_at = excluded.updated_at",
        )
        .bind(enterprise_id)
        .bind(cost_cap_micros)
        .bind(allowed_json)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Estimated spend (USD-micros) for the company over the rolling budget
    /// window.
    async fn budget_used_micros(&self, enterprise_id: &str) -> Result<i64, BillingError> {
        let since = now_ms() - BUDGET_WINDOW_MS;
        let used: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(estimated_cost_micros), 0) FROM one_usage_events \
             WHERE enterprise_id = ? AND created_at >= ?",
        )
        .bind(enterprise_id)
        .bind(since)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        Ok(used)
    }

    /// Cumulative estimated spend (USD-micros) for one conversation, summed
    /// across every `one_usage_events` row `record_turn`/`record_media_usage`
    /// wrote for it. Self-scoped by `user_id` — the caller can only ever sum
    /// rows they themselves generated, so no separate "is this conversation
    /// yours" ownership check is needed (unlike the enterprise-wide
    /// `budget_used_micros` above, this is meant to be called by any
    /// authenticated member for their own conversation, not just billing
    /// admins). Backends that never write a row here (a brand-new
    /// conversation with no turns yet) return 0, not an error.
    pub async fn conversation_cost(&self, user_id: &str, conversation_id: &str) -> Result<i64, BillingError> {
        let used: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(estimated_cost_micros), 0) FROM one_usage_events \
             WHERE user_id = ? AND conversation_id = ?",
        )
        .bind(user_id)
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        Ok(used)
    }

    /// Set (or, with `None`, clear) a department's spend cap (T7). Same
    /// rolling-30-day window as the company-level cap; a department cap is a
    /// tighter constraint layered UNDER the company one, never a replacement
    /// for it — a department under its own cap can still be blocked by the
    /// company running out of budget, and vice versa.
    pub async fn set_department_budget(
        &self,
        enterprise_id: &str,
        department_id: &str,
        cost_cap_micros: Option<i64>,
    ) -> Result<(), BillingError> {
        sqlx::query(
            "INSERT INTO one_department_budgets (department_id, enterprise_id, cost_cap_micros, updated_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(department_id) DO UPDATE SET \
                 cost_cap_micros = excluded.cost_cap_micros, updated_at = excluded.updated_at",
        )
        .bind(department_id)
        .bind(enterprise_id)
        .bind(cost_cap_micros)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn department_budget_cap(&self, department_id: &str) -> Result<Option<i64>, BillingError> {
        let cap: Option<Option<i64>> =
            sqlx::query_scalar("SELECT cost_cap_micros FROM one_department_budgets WHERE department_id = ?")
                .bind(department_id)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or(None);
        Ok(cap.flatten())
    }

    /// Estimated spend (USD-micros) for one department over the rolling
    /// budget window. Reads `one_usage_events.department_id`, stamped at
    /// record time — see the migration's doc comment for why that is
    /// denormalized rather than resolved live from `one_user_org`.
    async fn department_budget_used_micros(&self, department_id: &str) -> Result<i64, BillingError> {
        let since = now_ms() - BUDGET_WINDOW_MS;
        let used: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(estimated_cost_micros), 0) FROM one_usage_events \
             WHERE department_id = ? AND created_at >= ?",
        )
        .bind(department_id)
        .bind(since)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        Ok(used)
    }

    /// Every department that has ever had a cap configured for this company,
    /// with current-window spend (T7 dashboard). Departments with no cap and
    /// no spend simply do not appear — nothing to show an admin about them.
    pub async fn list_department_budgets(&self, enterprise_id: &str) -> Result<Vec<DepartmentBudgetDto>, BillingError> {
        let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
            "SELECT department_id, cost_cap_micros FROM one_department_budgets \
             WHERE enterprise_id = ? ORDER BY updated_at DESC",
        )
        .bind(enterprise_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (department_id, cost_cap_micros) in rows {
            let cost_used_micros = self.department_budget_used_micros(&department_id).await?;
            out.push(DepartmentBudgetDto {
                department_id,
                cost_cap_micros,
                cost_used_micros,
            });
        }
        Ok(out)
    }

    /// T8: whether this company has opted into storing generation prompts.
    /// Absent row = default = never store them.
    pub async fn media_ledger_retain_prompts(&self, enterprise_id: &str) -> Result<bool, BillingError> {
        let retain: Option<bool> =
            sqlx::query_scalar("SELECT retain_prompts FROM one_media_ledger_settings WHERE enterprise_id = ?")
                .bind(enterprise_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(retain.unwrap_or(false))
    }

    /// Admin-only: opt the company in or out of prompt retention.
    pub async fn set_media_ledger_retain_prompts(
        &self,
        enterprise_id: &str,
        retain_prompts: bool,
    ) -> Result<(), BillingError> {
        sqlx::query(
            "INSERT INTO one_media_ledger_settings (enterprise_id, retain_prompts, updated_at) \
             VALUES (?, ?, ?) \
             ON CONFLICT(enterprise_id) DO UPDATE SET \
                 retain_prompts = excluded.retain_prompts, updated_at = excluded.updated_at",
        )
        .bind(enterprise_id)
        .bind(retain_prompts)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// T8: record one generated FILE in the consolidated ledger (a job that
    /// produces N assets is N calls, not one). Enterprise-scoped only — a
    /// personal/no-company user is a no-op, same red line every other
    /// governance surface in this crate honors, and it keeps this table from
    /// silently becoming a per-user media index nobody asked for.
    ///
    /// `prompt` is always accepted from the caller but only persisted when
    /// the company has opted in — enforced HERE, not trusted from the client,
    /// same principle as every other policy check in this file.
    pub async fn record_media_asset(
        &self,
        user_id: &str,
        kind: &str,
        model: Option<&str>,
        file_path: &str,
        prompt: Option<&str>,
        conversation_id: Option<&str>,
    ) -> Result<(), BillingError> {
        let Some(enterprise_id) = self.resolve_enterprise_id(user_id).await? else {
            return Ok(());
        };
        let department_id = self.resolve_department_id(user_id).await?;
        let retained_prompt = if self.media_ledger_retain_prompts(&enterprise_id).await? {
            prompt
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO one_media_assets \
                (id, user_id, enterprise_id, department_id, conversation_id, kind, model, file_path, prompt, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(generate_prefixed_id("media"))
        .bind(user_id)
        .bind(&enterprise_id)
        .bind(department_id)
        .bind(conversation_id)
        .bind(kind)
        .bind(model)
        .bind(file_path)
        .bind(retained_prompt)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// T8: admin-only search over the ledger. `filters.prompt_contains` is
    /// harmless to pass even when the company never opted into retention —
    /// the column is NULL for every row in that case, so a `LIKE` against it
    /// simply matches nothing, no special-casing required.
    pub async fn list_media_assets(
        &self,
        enterprise_id: &str,
        filters: MediaAssetFilters<'_>,
    ) -> Result<Vec<MediaAssetDto>, BillingError> {
        let mut sql = String::from(
            "SELECT id, user_id, department_id, conversation_id, kind, model, file_path, prompt, created_at \
             FROM one_media_assets WHERE enterprise_id = ?",
        );
        if filters.kind.is_some() {
            sql.push_str(" AND kind = ?");
        }
        if filters.model.is_some() {
            sql.push_str(" AND model = ?");
        }
        if filters.user_id.is_some() {
            sql.push_str(" AND user_id = ?");
        }
        if filters.since.is_some() {
            sql.push_str(" AND created_at >= ?");
        }
        if filters.prompt_contains.is_some() {
            sql.push_str(" AND prompt LIKE ?");
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ?");

        let mut query = sqlx::query_as::<_, MediaAssetRow>(&sql).bind(enterprise_id);
        if let Some(kind) = filters.kind {
            query = query.bind(kind);
        }
        if let Some(model) = filters.model {
            query = query.bind(model);
        }
        if let Some(user_id) = filters.user_id {
            query = query.bind(user_id);
        }
        if let Some(since) = filters.since {
            query = query.bind(since);
        }
        if let Some(needle) = filters.prompt_contains {
            query = query.bind(format!("%{}%", needle.replace('%', "\\%").replace('_', "\\_")));
        }
        query = query.bind(filters.limit.unwrap_or(200).clamp(1, 1000));

        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(MediaAssetRow::into_dto).collect())
    }

    /// Pre-send gate (P1-2): reject when the company is over its spend budget,
    /// or the requested `model` is not on its allowlist. Personal / no-company
    /// users, and companies with neither control set, always pass (red line).
    ///
    /// ⚠️ T6-4: a company member is not automatically a GOVERNED member. Someone
    /// who logged in after the plan's seat cap filled has a row (so they are
    /// found here, not mistaken for personal) but no assigned seat, and
    /// therefore no policy that was configured for them — the only correct
    /// answer is to deny outright, before even looking at the allowlist/budget
    /// (which, on a company with neither control set, would otherwise pass
    /// everyone through unconditionally).
    pub async fn check_send_allowed(&self, user_id: &str, model: Option<&str>) -> Result<(), BillingError> {
        let Some(enterprise_id) = self.resolve_enterprise_id(user_id).await? else {
            return Ok(());
        };
        if !self.has_active_seat(user_id).await? {
            return Err(BillingError::SeatLimitExceeded);
        }
        let license = self.license_of(&enterprise_id).await?;

        // Model allowlist.
        if !license.allowed_models.is_empty()
            && let Some(model) = model.map(str::trim).filter(|s| !s.is_empty())
            && !license.allowed_models.iter().any(|m| m == model)
        {
            return Err(BillingError::ModelNotAllowed(model.to_owned()));
        }

        // Spend cap — company-wide first: if the whole company is out of
        // budget that is the more useful thing to tell the caller, and it
        // blocks everyone regardless of department.
        if let Some(cap) = license.cost_cap_micros
            && self.budget_used_micros(&enterprise_id).await? >= cap
        {
            return Err(BillingError::BudgetExceeded);
        }

        // T7: a department cap is a tighter constraint layered under the
        // company one. Unassigned members have no department to check.
        if let Some(department_id) = self.resolve_department_id(user_id).await?
            && let Some(cap) = self.department_budget_cap(&department_id).await?
            && self.department_budget_used_micros(&department_id).await? >= cap
        {
            return Err(BillingError::DepartmentBudgetExceeded);
        }
        Ok(())
    }

    /// Allowlist-only check (P1-2): whether `model` may be selected under the
    /// company policy. Used at the model-switch point (budget is enforced
    /// separately at send). Personal / no-allowlist → allowed.
    ///
    /// Same T6-4 guard as `check_send_allowed`: a pending (unseated) member is
    /// denied outright rather than falling through to a possibly-empty
    /// allowlist that would otherwise let them pick any model.
    pub async fn check_model_allowed(&self, user_id: &str, model: &str) -> Result<(), BillingError> {
        let Some(enterprise_id) = self.resolve_enterprise_id(user_id).await? else {
            return Ok(());
        };
        if !self.has_active_seat(user_id).await? {
            return Err(BillingError::SeatLimitExceeded);
        }
        let license = self.license_of(&enterprise_id).await?;
        let model = model.trim();
        if !license.allowed_models.is_empty() && !model.is_empty() && !license.allowed_models.iter().any(|m| m == model)
        {
            return Err(BillingError::ModelNotAllowed(model.to_owned()));
        }
        Ok(())
    }

    /// Whether a media generation (image / video) may run under company policy.
    ///
    /// Media reaches the provider through the built-in MCP tool, which never
    /// passed through `SendGate` — so until this existed, the most expensive
    /// calls in the product bypassed both the spend cap and the model
    /// allowlist entirely. The policy is deliberately the same one the chat
    /// path uses (one allowlist, one budget) rather than a parallel set of
    /// rules an admin would have to discover and maintain separately.
    ///
    /// Personal / no-company users pass, same red line as everywhere else.
    pub async fn check_media_allowed(&self, user_id: &str, model: &str) -> Result<(), BillingError> {
        self.check_send_allowed(user_id, Some(model)).await
    }

    /// Record one completed media generation against the company's usage.
    ///
    /// Reuses `one_usage_events`: media has no token counts, so those columns
    /// stay NULL and only the estimated cost is carried. That keeps media
    /// inside the existing budget rollup (`budget_used_micros`) and the usage
    /// dashboard without a schema change.
    ///
    /// `conversation_id` is attribution, not content — it is what lets an admin
    /// follow a charge back to where it happened. Optional because a caller may
    /// genuinely not have one; the column has always been nullable.
    pub async fn record_media_usage(&self, usage: MediaUsage<'_>) -> Result<(), BillingError> {
        let MediaUsage {
            user_id,
            kind,
            model,
            count,
            duration_seconds,
            unit_price_micros,
            conversation_id,
        } = usage;
        let enterprise_id = self.resolve_enterprise_id(user_id).await?;
        let department_id = self.resolve_department_id(user_id).await?;
        // A price the user entered for their own provider beats our built-in
        // table: the table is a coarse illustration, theirs is the contract they
        // are actually billed under.
        let cost = match unit_price_micros.filter(|price| *price > 0) {
            Some(price) => {
                let units = if kind.eq_ignore_ascii_case("video") {
                    let seconds = if duration_seconds > 0 { duration_seconds } else { 5 };
                    count.max(0) * seconds
                } else {
                    count.max(0)
                };
                units * price
            }
            None => estimate_media_cost_micros(kind, model, count, duration_seconds),
        };
        // A media call that costs 0 does not just report oddly — it consumes
        // none of the company's spend cap, so the cap silently stops binding for
        // that model. The built-in rate table matches on model name, and a
        // gateway with its own naming (very common) misses it entirely. Nothing
        // here invents a number: the row is recorded as-is, but an operator can
        // now find out why their cap is not moving, and fix it by entering a
        // unit price for the model.
        if cost == 0 && count > 0 && enterprise_id.is_some() {
            tracing::warn!(
                model,
                kind,
                "media usage recorded at zero cost: no built-in rate matched this model and no unit \
                 price was configured, so it does not count against the company's spend cap"
            );
        }
        sqlx::query(
            "INSERT INTO one_usage_events \
                (id, user_id, enterprise_id, department_id, conversation_id, model, input_tokens, output_tokens, total_tokens, estimated_cost_micros, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, NULL, ?, ?)",
        )
        .bind(generate_prefixed_id("usage"))
        .bind(user_id)
        .bind(enterprise_id)
        .bind(department_id)
        .bind(conversation_id)
        .bind(model)
        .bind(cost)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The company plan for the dashboard: tier, seat usage, entitlements.
    pub async fn plan(&self, enterprise_id: &str) -> Result<PlanDto, BillingError> {
        let license = self.license_of(enterprise_id).await?;
        let entitlements = dream_core_common::license::ALL_FEATURES
            .iter()
            .map(|f| EntitlementDto {
                feature: f.as_str().to_owned(),
                allowed: tier_allows(license.tier, *f),
            })
            .collect();
        Ok(PlanDto {
            enterprise_id: enterprise_id.to_owned(),
            tier: license.tier.as_str().to_owned(),
            seat_used: self.seat_used(enterprise_id).await?,
            seat_limit: Self::effective_seat_limit(&license),
            seat_pending: self.seat_pending(enterprise_id).await?,
            expires_at: license.expires_at,
            entitlements,
            cost_cap_micros: license.cost_cap_micros,
            cost_used_micros: self.budget_used_micros(enterprise_id).await?,
            allowed_models: license.allowed_models,
        })
    }

    /// Record one metered turn. `enterprise_id` is resolved from the user;
    /// personal users record with a NULL enterprise. Tokens are best-effort
    /// (may be `None`); cost is an estimate from the model rate table.
    ///
    /// `channel_id` is the raw `providers.id` of the configuration that served
    /// the turn (`prov_chan_<channel_id>` for enterprise channels; `None` for
    /// historical callers and sources with no provider id, which bucket as
    /// `"unknown"` — see the migration's doc comment for why this is stored
    /// verbatim with no cross-crate join).
    pub async fn record_turn(
        &self,
        user_id: &str,
        conversation_id: Option<&str>,
        model: Option<&str>,
        channel_id: Option<&str>,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
    ) -> Result<(), BillingError> {
        let enterprise_id = self.resolve_enterprise_id(user_id).await?;
        // T7: stamped from the user's department AT THIS MOMENT, not resolved
        // live when a report is later pulled — see the migration's doc
        // comment for why (a department is a cost center; reassigning someone
        // must not reshuffle where past spend counted).
        let department_id = self.resolve_department_id(user_id).await?;
        let total_tokens = match (input_tokens, output_tokens) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        let cost = model.map(|m| estimate_cost_micros(m, input_tokens.unwrap_or(0), output_tokens.unwrap_or(0)));
        // Same diagnosis hatch as `record_media_usage`: a turn costed at 0
        // consumes none of the company's spend cap, so the cap silently stops
        // binding for that model. The built-in rate table matches on model
        // name, and a gateway with its own naming misses it entirely — as does
        // a vision delegate whose model the table has never heard of. Nothing
        // here invents a rate (that would be a pricing decision); the row is
        // recorded as-is, but an operator can now find out why their cap is
        // not moving.
        if cost == Some(0) && total_tokens.unwrap_or(0) > 0 && enterprise_id.is_some() {
            tracing::warn!(
                model,
                "turn usage recorded at zero cost: no built-in rate matched this model, so it does not count                  against the company's spend cap"
            );
        }
        sqlx::query(
            "INSERT INTO one_usage_events \
                (id, user_id, enterprise_id, department_id, conversation_id, model, channel_id, input_tokens, output_tokens, total_tokens, estimated_cost_micros, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(generate_prefixed_id("usage"))
        .bind(user_id)
        .bind(enterprise_id)
        .bind(department_id)
        .bind(conversation_id)
        .bind(model)
        .bind(channel_id)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(total_tokens)
        .bind(cost)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Deletes every billing/usage record attributed to `enterprise_id`:
    /// license/tier/spend-cap/allowlist, metered usage events, department
    /// budgets, the media ledger and its retention setting, and license
    /// activation history. Called by one-enterprise's `disband_company`
    /// through the `CompanyDisbandCascade` trait it wires up in
    /// `dream-app` (same layer, no direct dependency). Authorization is
    /// enforced by that caller; this trusts the `enterprise_id` it is given.
    pub async fn delete_enterprise_billing_data(&self, enterprise_id: &str) -> Result<(), BillingError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM one_enterprise_license WHERE enterprise_id = ?")
            .bind(enterprise_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM one_usage_events WHERE enterprise_id = ?")
            .bind(enterprise_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM one_llm_calls WHERE enterprise_id = ?")
            .bind(enterprise_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM one_department_budgets WHERE enterprise_id = ?")
            .bind(enterprise_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM one_media_assets WHERE enterprise_id = ?")
            .bind(enterprise_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM one_media_ledger_settings WHERE enterprise_id = ?")
            .bind(enterprise_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM one_license_activation WHERE enterprise_id = ?")
            .bind(enterprise_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::warn!(enterprise_id, "enterprise billing history deleted (企业注销级联)");
        Ok(())
    }

    /// Aggregate usage for a company since `since_ms`, grouped by user, model,
    /// channel, and day, plus grand totals.
    pub async fn usage_summary(&self, enterprise_id: &str, since_ms: i64) -> Result<UsageSummaryDto, BillingError> {
        let by_user = self.buckets(enterprise_id, since_ms, "user_id").await?;
        let by_model = self
            .buckets(enterprise_id, since_ms, "COALESCE(model, 'unknown')")
            .await?;
        let by_channel = self
            .buckets(enterprise_id, since_ms, "COALESCE(channel_id, 'unknown')")
            .await?;
        let by_day = self
            .buckets(
                enterprise_id,
                since_ms,
                "strftime('%Y-%m-%d', created_at / 1000, 'unixepoch')",
            )
            .await?;
        let by_department = self
            .buckets(enterprise_id, since_ms, "COALESCE(department_id, 'unassigned')")
            .await?;

        let (total_turns, total_tokens, total_cost): (i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(total_tokens), 0), COALESCE(SUM(estimated_cost_micros), 0) \
             FROM one_usage_events WHERE enterprise_id = ? AND created_at >= ?",
        )
        .bind(enterprise_id)
        .bind(since_ms)
        .fetch_one(&self.pool)
        .await?;

        // Media rows are the ones with no token counts (media is metered per
        // asset, never per token), so `total_tokens IS NULL` identifies them
        // without a schema change. A zero cost among those means nothing priced
        // the call — neither the built-in table nor a unit price the admin
        // entered — and it therefore consumed none of the spend cap.
        let (unpriced_media_calls, unpriced_models): (i64, Option<String>) = sqlx::query_as(
            "SELECT COUNT(*), GROUP_CONCAT(DISTINCT model) FROM one_usage_events \
             WHERE enterprise_id = ? AND created_at >= ? \
               AND total_tokens IS NULL AND model IS NOT NULL AND estimated_cost_micros = 0",
        )
        .bind(enterprise_id)
        .bind(since_ms)
        .fetch_one(&self.pool)
        .await?;

        Ok(UsageSummaryDto {
            since: since_ms,
            total_turns,
            total_tokens,
            estimated_cost_micros: total_cost,
            by_user,
            by_model,
            by_channel,
            by_day,
            by_department,
            unpriced_media_calls,
            unpriced_media_models: unpriced_models
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }

    /// Grouped aggregation. `key_expr` is a trusted SQL expression (never user
    /// input) selecting the bucket key.
    async fn buckets(
        &self,
        enterprise_id: &str,
        since_ms: i64,
        key_expr: &str,
    ) -> Result<Vec<UsageBucketDto>, BillingError> {
        let sql = format!(
            "SELECT {key_expr} AS k, COUNT(*), COALESCE(SUM(total_tokens), 0), COALESCE(SUM(estimated_cost_micros), 0) \
             FROM one_usage_events WHERE enterprise_id = ? AND created_at >= ? \
             GROUP BY k ORDER BY COUNT(*) DESC"
        );
        let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(&sql)
            .bind(enterprise_id)
            .bind(since_ms)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|(key, turns, total_tokens, cost)| UsageBucketDto {
                key,
                turns,
                total_tokens,
                estimated_cost_micros: cost,
            })
            .collect())
    }

    /// One page of raw `one_usage_events` rows (E5 "可观测" / LLM Trace) — the
    /// per-call-shaped view `usage_summary`'s buckets aggregate away.
    /// Filterable by user and/or model; always scoped to `enterprise_id` and
    /// `since_ms`, same as `usage_summary`. `limit` is clamped to [1, 200]
    /// so an unbounded query string can't force an unbounded response.
    pub async fn list_usage_events(
        &self,
        enterprise_id: &str,
        since_ms: i64,
        user_id: Option<&str>,
        model: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<UsageEventPageDto, BillingError> {
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);

        let mut where_sql = "WHERE enterprise_id = ? AND created_at >= ?".to_owned();
        if user_id.is_some() {
            where_sql.push_str(" AND user_id = ?");
        }
        if model.is_some() {
            where_sql.push_str(" AND model = ?");
        }

        let count_sql = format!("SELECT COUNT(*) FROM one_usage_events {where_sql}");
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql)
            .bind(enterprise_id)
            .bind(since_ms);
        if let Some(u) = user_id {
            count_query = count_query.bind(u);
        }
        if let Some(m) = model {
            count_query = count_query.bind(m);
        }
        let total = count_query.fetch_one(&self.pool).await?;

        let list_sql = format!(
            "SELECT id, user_id, conversation_id, model, channel_id, input_tokens, output_tokens, total_tokens, \
                    estimated_cost_micros, created_at \
             FROM one_usage_events {where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?"
        );
        type Row = (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            i64,
            i64,
        );
        let mut list_query = sqlx::query_as::<_, Row>(&list_sql).bind(enterprise_id).bind(since_ms);
        if let Some(u) = user_id {
            list_query = list_query.bind(u);
        }
        if let Some(m) = model {
            list_query = list_query.bind(m);
        }
        let rows = list_query.bind(limit).bind(offset).fetch_all(&self.pool).await?;

        let events = rows
            .into_iter()
            .map(
                |(
                    id,
                    user_id,
                    conversation_id,
                    model,
                    channel_id,
                    input_tokens,
                    output_tokens,
                    total_tokens,
                    cost,
                    created_at,
                )| {
                    UsageEventDto {
                        id,
                        user_id,
                        conversation_id,
                        model,
                        channel_id,
                        input_tokens,
                        output_tokens,
                        total_tokens,
                        estimated_cost_micros: cost,
                        created_at,
                    }
                },
            )
            .collect();

        Ok(UsageEventPageDto { events, total })
    }

    /// P2-5: record one completed MODEL CALL in the per-call trace
    /// (`one_llm_calls`). See [`NewLlmCall`] for what one row means; the hot
    /// path reaches this through dream-core-conversation's
    /// `LlmCallTraceRecorder` seam (same layering as `UsageRecorder`, which
    /// this crate never saw directly either — dream-app adapts between
    /// them).
    ///
    /// Enterprise is resolved from the user, NOT passed in — same split as
    /// `record_media_asset`, so the hot path never has to know about tenancy.
    /// A personal / no-company user records NOTHING (no NULL-enterprise rows
    /// either, unlike `record_turn`): this is a high-volume diagnostic surface
    /// that only exists for the governed/admin-view plane, and `None` = don't
    /// record is the standing red line. No cost is estimated here — pricing
    /// and spend caps stay on `one_usage_events`; this table is observability
    /// only.
    pub async fn record_llm_call(&self, call: NewLlmCall) -> Result<(), BillingError> {
        let NewLlmCall {
            user_id,
            conversation_id,
            model,
            provider,
            tool_name,
            input_tokens,
            output_tokens,
            duration_ms,
            error,
        } = call;
        let Some(enterprise_id) = self.resolve_enterprise_id(&user_id).await? else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO one_llm_calls \
                (id, enterprise_id, user_id, conversation_id, model, provider, tool_name, \
                 input_tokens, output_tokens, duration_ms, error, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(generate_prefixed_id("llmcall"))
        .bind(&enterprise_id)
        .bind(&user_id)
        .bind(conversation_id)
        .bind(model)
        .bind(provider)
        .bind(tool_name)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(duration_ms)
        .bind(error)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// P2-5: one page of raw `one_llm_calls` rows — the per-MODEL-CALL view
    /// one level finer than `list_usage_events`. Filter, pagination, and
    /// clamping mirror `list_usage_events` exactly (limit clamped to [1, 200],
    /// `total` covers the whole filtered set) so the admin UI reuses the same
    /// page shape for both granularities. Admin-only at the route; failed
    /// calls (`error IS NOT NULL`) are listed exactly like successful ones.
    pub async fn list_llm_calls(
        &self,
        enterprise_id: &str,
        since_ms: i64,
        user_id: Option<&str>,
        model: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<LlmCallPageDto, BillingError> {
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);

        let mut where_sql = "WHERE enterprise_id = ? AND created_at >= ?".to_owned();
        if user_id.is_some() {
            where_sql.push_str(" AND user_id = ?");
        }
        if model.is_some() {
            where_sql.push_str(" AND model = ?");
        }

        let count_sql = format!("SELECT COUNT(*) FROM one_llm_calls {where_sql}");
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql)
            .bind(enterprise_id)
            .bind(since_ms);
        if let Some(u) = user_id {
            count_query = count_query.bind(u);
        }
        if let Some(m) = model {
            count_query = count_query.bind(m);
        }
        let total = count_query.fetch_one(&self.pool).await?;

        let list_sql = format!(
            "SELECT id, user_id, conversation_id, model, provider, tool_name, input_tokens, output_tokens, \
                    duration_ms, error, created_at \
             FROM one_llm_calls {where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?"
        );
        type Row = (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
            Option<i64>,
            Option<String>,
            i64,
        );
        let mut list_query = sqlx::query_as::<_, Row>(&list_sql).bind(enterprise_id).bind(since_ms);
        if let Some(u) = user_id {
            list_query = list_query.bind(u);
        }
        if let Some(m) = model {
            list_query = list_query.bind(m);
        }
        let rows = list_query.bind(limit).bind(offset).fetch_all(&self.pool).await?;

        let calls = rows
            .into_iter()
            .map(
                |(
                    id,
                    user_id,
                    conversation_id,
                    model,
                    provider,
                    tool_name,
                    input_tokens,
                    output_tokens,
                    duration_ms,
                    error,
                    created_at,
                )| {
                    LlmCallDto {
                        id,
                        user_id,
                        conversation_id,
                        model,
                        provider,
                        tool_name,
                        input_tokens,
                        output_tokens,
                        duration_ms,
                        error,
                        created_at,
                    }
                },
            )
            .collect();

        Ok(LlmCallPageDto { calls, total })
    }

    /// P2-5 retention: delete every trace row for `enterprise_id` created
    /// before `before_ms` (epoch ms), returning how many were removed.
    /// Enterprise-scoped on purpose — a purge of one company must never touch
    /// another's history in the same table. Callers wanting the default window
    /// pass `now_ms() - LLM_CALL_RETENTION_DAYS * 24 * 3600 * 1000` (the admin
    /// endpoint does this when `beforeMs` is omitted).
    pub async fn purge_llm_calls_older_than(&self, enterprise_id: &str, before_ms: i64) -> Result<u64, BillingError> {
        let result = sqlx::query("DELETE FROM one_llm_calls WHERE enterprise_id = ? AND created_at < ?")
            .bind(enterprise_id)
            .bind(before_ms)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// P1-3 enterprise report: the §1 metric list NOT already covered by
    /// [`Self::usage_summary`]. Dimension split between the two endpoints is
    /// deliberate and documented on [`EnterpriseReportDto`]: the frontend
    /// report page calls both — per-user / per-department buckets stay on
    /// `usage`, adoption-rate and latency/rollup fields live here — so no
    /// number is ever computed twice from two different code paths.
    ///
    /// Empty-table tolerant: a personal-tier or freshly installed company has
    /// no `one_llm_calls` rows (and maybe no `one_usage_events` rows at all);
    /// every derived field degrades to `null` / `0` rather than erroring.
    pub async fn enterprise_report(
        &self,
        enterprise_id: &str,
        since_ms: i64,
    ) -> Result<EnterpriseReportDto, BillingError> {
        let now = now_ms();
        const SEVEN_DAYS_MS: i64 = 7 * 24 * 3600 * 1000;

        // WAU / MAU: DISTINCT users over FIXED trailing windows from now —
        // deliberately independent of `since`, which scopes the spend/token
        // dimensions: an admin paging back to last quarter still wants
        // "who is active this week" to mean this week.
        let wau: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT user_id) FROM one_usage_events \
             WHERE enterprise_id = ? AND created_at >= ?",
        )
        .bind(enterprise_id)
        .bind(now - SEVEN_DAYS_MS)
        .fetch_one(&self.pool)
        .await?;
        let mau: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT user_id) FROM one_usage_events \
             WHERE enterprise_id = ? AND created_at >= ?",
        )
        .bind(enterprise_id)
        .bind(now - BUDGET_WINDOW_MS)
        .fetch_one(&self.pool)
        .await?;

        // Per-capita tokens: window total / window active users. Division by
        // zero yields 0 — an average over zero active users is not 0, it is
        // undefined, but the report page renders "no activity either way", so
        // 0 with no special UI state is the honest-enough contract.
        let (active_users, window_tokens): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(DISTINCT user_id), COALESCE(SUM(total_tokens), 0) \
             FROM one_usage_events WHERE enterprise_id = ? AND created_at >= ?",
        )
        .bind(enterprise_id)
        .bind(since_ms)
        .fetch_one(&self.pool)
        .await?;
        let avg_tokens_per_user = if active_users > 0 {
            window_tokens as f64 / active_users as f64
        } else {
            0.0
        };

        // Top users by tokens. Same table and same user dimension as
        // `usage_summary`'s by_user bucket — but that bucket is ordered by
        // call count and unbounded, so the Top10 ordering is computed here
        // rather than re-sorted (and re-fetched in full) client-side.
        let top_rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT user_id, COALESCE(SUM(total_tokens), 0), COALESCE(SUM(estimated_cost_micros), 0) \
             FROM one_usage_events WHERE enterprise_id = ? AND created_at >= ? \
             GROUP BY user_id ORDER BY 2 DESC LIMIT 10",
        )
        .bind(enterprise_id)
        .bind(since_ms)
        .fetch_all(&self.pool)
        .await?;
        let top_users = top_rows
            .into_iter()
            .map(|(user_id, total_tokens, cost)| TopUserDto {
                user_id,
                total_tokens,
                estimated_cost_micros: cost,
            })
            .collect();

        // Call counts + success rate. `COUNT(error)` counts non-NULL errors
        // only — the "model call completed without error" rate OpenOcta's
        // "tool success rate ≥ 95%" KPI is approximated by (see the DTO doc).
        let (llm_call_count, llm_error_count): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COUNT(error) FROM one_llm_calls \
             WHERE enterprise_id = ? AND created_at >= ?",
        )
        .bind(enterprise_id)
        .bind(since_ms)
        .fetch_one(&self.pool)
        .await?;
        let tool_success_rate = if llm_call_count > 0 {
            Some((llm_call_count - llm_error_count) as f64 / llm_call_count as f64)
        } else {
            None
        };

        // Percentiles are computed in Rust, not SQL: SQLite has no percentile
        // function, and a GROUP_CONCAT hack would be both slower and far
        // harder to state a correctness claim about. One indexed scan pulls
        // every measured duration with its day already bucketed in SQL (same
        // strftime expression as `usage_summary`'s by_day), then Rust does
        // the sorting once for the window total and once per day group.
        type DurationRow = (String, i64);
        let duration_rows: Vec<DurationRow> = sqlx::query_as(
            "SELECT strftime('%Y-%m-%d', created_at / 1000, 'unixepoch') AS day, duration_ms \
             FROM one_llm_calls \
             WHERE enterprise_id = ? AND created_at >= ? AND duration_ms IS NOT NULL",
        )
        .bind(enterprise_id)
        .bind(since_ms)
        .fetch_all(&self.pool)
        .await?;

        let mut all_durations: Vec<i64> = duration_rows.iter().map(|(_, d)| *d).collect();
        all_durations.sort_unstable();
        // The n>=10 honesty rule applies ONLY to the window-wide figures: per
        // day, any day with at least one measured call reports what was
        // actually measured (`samples` carries the n so the chart can show
        // thin days as thin) — nulling most days would blank the trend for
        // exactly the smaller deployments the report is for.
        let latency_p50 = if all_durations.len() as i64 >= MIN_LATENCY_SAMPLES {
            percentile_of_sorted(&all_durations, 0.5)
        } else {
            None
        };
        let latency_p95 = if all_durations.len() as i64 >= MIN_LATENCY_SAMPLES {
            percentile_of_sorted(&all_durations, 0.95)
        } else {
            None
        };

        let mut by_day: std::collections::BTreeMap<String, Vec<i64>> = std::collections::BTreeMap::new();
        for (day, duration) in duration_rows {
            by_day.entry(day).or_default().push(duration);
        }
        let latency_trend = by_day
            .into_iter()
            .map(|(day, mut durations)| {
                durations.sort_unstable();
                LatencyTrendPointDto {
                    day,
                    p50: percentile_of_sorted(&durations, 0.5).unwrap_or_default(),
                    p95: percentile_of_sorted(&durations, 0.95).unwrap_or_default(),
                    samples: durations.len() as i64,
                }
            })
            .collect();

        Ok(EnterpriseReportDto {
            since: since_ms,
            wau,
            mau,
            avg_tokens_per_user,
            latency_p50,
            latency_p95,
            latency_trend,
            tool_success_rate,
            top_users,
            llm_call_count,
            llm_error_count,
        })
    }

    /// One page of agent sessions (E5 "可观测" / 智能体会话), derived by
    /// grouping `one_usage_events` on `conversation_id` — rows with no
    /// conversation id (never attributed to a specific session) are excluded
    /// rather than folded into one synthetic "no conversation" bucket, which
    /// would mix unrelated turns together under a misleading shared identity.
    pub async fn list_sessions(
        &self,
        enterprise_id: &str,
        since_ms: i64,
        limit: i64,
        offset: i64,
    ) -> Result<AgentSessionPageDto, BillingError> {
        let limit = limit.clamp(1, 200);
        let offset = offset.max(0);

        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT conversation_id) FROM one_usage_events \
             WHERE enterprise_id = ? AND created_at >= ? AND conversation_id IS NOT NULL",
        )
        .bind(enterprise_id)
        .bind(since_ms)
        .fetch_one(&self.pool)
        .await?;

        type Row = (String, String, i64, i64, i64, i64, i64);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT conversation_id, MIN(user_id), COUNT(*), COALESCE(SUM(total_tokens), 0), \
                    COALESCE(SUM(estimated_cost_micros), 0), MIN(created_at), MAX(created_at) \
             FROM one_usage_events \
             WHERE enterprise_id = ? AND created_at >= ? AND conversation_id IS NOT NULL \
             GROUP BY conversation_id ORDER BY MAX(created_at) DESC LIMIT ? OFFSET ?",
        )
        .bind(enterprise_id)
        .bind(since_ms)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        let mut sessions = Vec::with_capacity(rows.len());
        for (conversation_id, user_id, turn_count, total_tokens, cost, first_seen_at, last_seen_at) in rows {
            let models: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT model FROM one_usage_events \
                 WHERE enterprise_id = ? AND conversation_id = ? AND model IS NOT NULL",
            )
            .bind(enterprise_id)
            .bind(&conversation_id)
            .fetch_all(&self.pool)
            .await?;
            sessions.push(AgentSessionDto {
                conversation_id,
                user_id,
                models,
                turn_count,
                total_tokens,
                estimated_cost_micros: cost,
                first_seen_at,
                last_seen_at,
            });
        }

        Ok(AgentSessionPageDto { sessions, total })
    }

    /// Begin a checkout (stubbed by the manual provider).
    pub fn create_checkout(&self, enterprise_id: &str, target_tier: &str) -> CheckoutResultDto {
        self.provider.create_checkout(enterprise_id, target_tier)
    }

    /// Whether the caller may see the usage dashboard / provision a tier.
    ///
    /// Billing is **enterprise-scoped** (`one_enterprise_license` and
    /// `one_usage_events` are keyed by `enterprise_id`), so the guard has to be
    /// enterprise-scoped too: a company admin, or the deployment's
    /// `system_admin`.
    ///
    /// A plain `org_admin` is deliberately NOT enough. An org_admin administers
    /// one project group, but the tier, seat cap, spend cap and model allowlist
    /// they would be changing apply to the whole company — so in a company with
    /// several project groups, accepting org_admin would let group A's admin
    /// raise (or cut) the budget for everyone. The `system_admin` arm is what
    /// keeps personal / single-machine deployments working, where the machine
    /// owner holds that role and there is no company row at all.
    ///
    /// Tolerant of absent tables (personal mode → falls through to the role
    /// read, which itself tolerates a missing `one_user_org`).
    pub async fn is_billing_admin(&self, user_id: &str) -> Result<bool, BillingError> {
        let company_role: Option<String> =
            sqlx::query_scalar("SELECT role FROM one_enterprise_members WHERE user_id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or(None);
        if company_role.as_deref() == Some("admin") {
            return Ok(true);
        }
        // Server admin: active-tenant-aware role read, mirroring the cross-crate
        // role resolution in one-devops / one-sso.
        let org_role: Option<String> = sqlx::query_scalar(
            "SELECT uo.role FROM one_user_org uo WHERE uo.user_id = ? \
             ORDER BY (uo.tenant_id = (SELECT tenant_id FROM one_active_tenant WHERE user_id = uo.user_id)) DESC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or(None);
        Ok(org_role.as_deref() == Some("system_admin"))
    }
}

/// Percentile of a PRE-SORTED slice, floored index (`((n-1) * p).floor()`).
/// No linear interpolation on purpose: the floored index always lands on an
/// actually observed duration — "this many ms were measured" — rather than an
/// invented value between two observations, and the rule is trivially
/// reproducible by any client re-checking the report. `None` only for an
/// empty slice; the sample-size honesty gate lives at the caller
/// (`MIN_LATENCY_SAMPLES`), which decides whether ANY percentile is published.
fn percentile_of_sorted(sorted: &[i64], p: f64) -> Option<i64> {
    match sorted.len() {
        0 => None,
        1 => Some(sorted[0]),
        n => {
            let idx = (((n - 1) as f64) * p).floor() as usize;
            Some(sorted[idx.min(n - 1)])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn service() -> BillingService {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::migrate::tests::one_enterprise_tables(&pool).await;
        crate::migrate::run_one_billing_migrations(&pool).await.unwrap();
        BillingService::new(pool, Arc::new(ManualBillingProvider))
    }

    async fn add_members(svc: &BillingService, enterprise_id: &str, n: usize) {
        for i in 0..n {
            sqlx::query("INSERT INTO one_enterprise_members (user_id, enterprise_id, role, joined_at, updated_at) VALUES (?, ?, 'member', 0, 0)")
                .bind(format!("u{enterprise_id}{i}"))
                .bind(enterprise_id)
                .execute(&svc.pool)
                .await
                .unwrap();
        }
    }

    /// ⚠️ The point of company disband cascading into one-billing: every
    /// billing/usage record attributed to the disbanded company must be
    /// gone from every table it can appear in — an unrelated company's
    /// history in the same tables must survive untouched.
    #[tokio::test]
    async fn deleting_enterprise_billing_data_clears_every_table_for_that_company_only() {
        let svc = service().await;
        for ent in ["ent1", "ent2"] {
            sqlx::query(
                "INSERT INTO one_enterprise_license (enterprise_id, tier, updated_at) VALUES (?, 'enterprise', 0)",
            )
            .bind(ent)
            .execute(&svc.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO one_usage_events (id, user_id, enterprise_id, model, created_at) \
                 VALUES (?, 'u1', ?, 'claude-opus', 0)",
            )
            .bind(format!("evt-{ent}"))
            .bind(ent)
            .execute(&svc.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO one_department_budgets (department_id, enterprise_id, cost_cap_micros, updated_at) \
                 VALUES (?, ?, 1000, 0)",
            )
            .bind(format!("dep-{ent}"))
            .bind(ent)
            .execute(&svc.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO one_media_assets (id, user_id, enterprise_id, kind, file_path, created_at) \
                 VALUES (?, 'u1', ?, 'image', '/tmp/x.png', 0)",
            )
            .bind(format!("asset-{ent}"))
            .bind(ent)
            .execute(&svc.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO one_media_ledger_settings (enterprise_id, retain_prompts, updated_at) VALUES (?, 1, 0)",
            )
            .bind(ent)
            .execute(&svc.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO one_license_activation \
                    (license_id, enterprise_id, customer, tier, issued_at, activated_at, activated_by) \
                 VALUES (?, ?, 'Acme', 'enterprise', 0, 0, 'admin1')",
            )
            .bind(format!("lic-{ent}"))
            .bind(ent)
            .execute(&svc.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO one_llm_calls (id, enterprise_id, user_id, created_at) VALUES (?, ?, 'u1', 0)")
                .bind(format!("llmcall-{ent}"))
                .bind(ent)
                .execute(&svc.pool)
                .await
                .unwrap();
        }

        svc.delete_enterprise_billing_data("ent1").await.unwrap();

        for (table, column) in [
            ("one_enterprise_license", "enterprise_id"),
            ("one_usage_events", "enterprise_id"),
            ("one_llm_calls", "enterprise_id"),
            ("one_department_budgets", "enterprise_id"),
            ("one_media_assets", "enterprise_id"),
            ("one_media_ledger_settings", "enterprise_id"),
            ("one_license_activation", "enterprise_id"),
        ] {
            let gone: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE {column} = 'ent1'"))
                .fetch_one(&svc.pool)
                .await
                .unwrap();
            assert_eq!(gone, 0, "{table} must have no ent1 rows left");
            let survives: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE {column} = 'ent2'"))
                .fetch_one(&svc.pool)
                .await
                .unwrap();
            assert_eq!(survives, 1, "{table} must not have touched ent2's row");
        }
    }

    #[tokio::test]
    async fn personal_no_enterprise_allows_all_and_unlimited_seats() {
        let svc = service().await;
        // No enterprise → every feature allowed, seats always addable.
        assert!(svc.resolve_enterprise_id("nobody").await.unwrap().is_none());
        assert!(svc.can_add_seat(None).await.unwrap());
        for f in dream_core_common::license::ALL_FEATURES {
            assert!(svc.entitlement(None, f).await.unwrap(), "personal allows {f:?}");
        }
        // Recording usage with no enterprise is fine (NULL enterprise_id).
        svc.record_turn("nobody", Some("c1"), Some("claude-opus"), None, Some(10), Some(20))
            .await
            .unwrap();
    }

    /// Force a tier directly in the table, bypassing the license gate.
    ///
    /// Tests must not carry a real signing key (it would then live in the
    /// repo), so entitlement fixtures write the row directly. The *gate* on
    /// raising a tier is covered separately by
    /// `set_tier_refuses_upgrade_without_license`.
    async fn force_tier(svc: &BillingService, enterprise_id: &str, tier: Tier, expires_at: Option<i64>) {
        sqlx::query(
            "INSERT INTO one_enterprise_license (enterprise_id, tier, seat_limit, expires_at, updated_at) \
             VALUES (?, ?, NULL, ?, 0) \
             ON CONFLICT(enterprise_id) DO UPDATE SET tier = excluded.tier, expires_at = excluded.expires_at",
        )
        .bind(enterprise_id)
        .bind(tier.as_str())
        .bind(expires_at)
        .execute(&svc.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn free_tier_gates_features_and_caps_seats() {
        let svc = service().await;
        force_tier(&svc, "ent_free", Tier::Free, None).await;
        // Free: SSO disallowed.
        assert!(!svc.entitlement(Some("ent_free"), Feature::Sso).await.unwrap());
        // Seat cap 3: three ok, fourth blocked.
        add_members(&svc, "ent_free", 3).await;
        assert!(!svc.can_add_seat(Some("ent_free")).await.unwrap());
        // On team → SSO allowed, cap 25.
        force_tier(&svc, "ent_free", Tier::Team, None).await;
        assert!(svc.entitlement(Some("ent_free"), Feature::Sso).await.unwrap());
        assert!(svc.can_add_seat(Some("ent_free")).await.unwrap());
    }

    /// The commercial keystone: a customer's own admin must not be able to
    /// grant themselves a higher tier. Without this the entire licensing
    /// scheme is decorative.
    #[tokio::test]
    async fn set_tier_refuses_upgrade_without_license() {
        let svc = service().await;
        force_tier(&svc, "ent_x", Tier::Free, None).await;

        for target in [Tier::Team, Tier::Enterprise] {
            let err = svc.set_tier("ent_x", target, None).await.unwrap_err();
            assert_eq!(
                err.code(),
                "UPGRADE_REQUIRES_LICENSE",
                "raising free → {target:?} must be refused"
            );
        }
        // Still free — the refused calls changed nothing.
        assert!(!svc.entitlement(Some("ent_x"), Feature::Sso).await.unwrap());

        // Downgrades remain self-service.
        force_tier(&svc, "ent_x", Tier::Enterprise, None).await;
        svc.set_tier("ent_x", Tier::Team, None).await.unwrap();
        assert!(!svc.entitlement(Some("ent_x"), Feature::AuditLog).await.unwrap());
        svc.set_tier("ent_x", Tier::Free, None).await.unwrap();
        assert!(!svc.entitlement(Some("ent_x"), Feature::Sso).await.unwrap());
    }

    /// An expired license must degrade to free everywhere at once — including
    /// dropping any seat override it granted.
    #[tokio::test]
    async fn expired_license_degrades_to_free() {
        let svc = service().await;
        let past = dream_core_common::now_ms() - 1000;
        force_tier(&svc, "ent_exp", Tier::Enterprise, Some(past)).await;
        // Give it an explicit generous seat override too.
        sqlx::query("UPDATE one_enterprise_license SET seat_limit = 500 WHERE enterprise_id = 'ent_exp'")
            .execute(&svc.pool)
            .await
            .unwrap();

        // Enterprise features are gone...
        assert!(!svc.entitlement(Some("ent_exp"), Feature::AuditLog).await.unwrap());
        assert!(!svc.entitlement(Some("ent_exp"), Feature::Sso).await.unwrap());
        // ...and the seat cap falls back to free's 3, not the 500 override.
        add_members(&svc, "ent_exp", 3).await;
        assert!(
            !svc.can_add_seat(Some("ent_exp")).await.unwrap(),
            "an expired license must not keep its seat override"
        );

        // A still-valid license keeps working.
        let future = dream_core_common::now_ms() + 60_000;
        force_tier(&svc, "ent_ok", Tier::Enterprise, Some(future)).await;
        assert!(svc.entitlement(Some("ent_ok"), Feature::AuditLog).await.unwrap());
    }

    #[tokio::test]
    async fn existing_enterprise_grandfathered_to_top_tier() {
        let svc = service().await;
        // Simulate a pre-billing company, then re-run migration (grandfather).
        sqlx::query("INSERT INTO one_enterprises (id, provider, external_id, created_at, updated_at) VALUES ('ent_old', 'feishu', 'x', 0, 0)")
            .execute(&svc.pool)
            .await
            .unwrap();
        // Wipe ledger entry so the backfill re-runs against the new row.
        sqlx::query("DELETE FROM _one_migrations WHERE name = 'billing_001_init'")
            .execute(&svc.pool)
            .await
            .unwrap();
        crate::migrate::run_one_billing_migrations(&svc.pool).await.unwrap();
        // Grandfathered to enterprise: all features on, unlimited seats.
        assert!(svc.entitlement(Some("ent_old"), Feature::AuditLog).await.unwrap());
        let plan = svc.plan("ent_old").await.unwrap();
        assert_eq!(plan.tier, "enterprise");
        assert_eq!(plan.seat_limit, None);
    }

    #[tokio::test]
    async fn usage_summary_aggregates() {
        let svc = service().await;
        add_members(&svc, "ent1", 1).await;
        // Map the recording user to ent1 so record_turn resolves it.
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'alice' WHERE enterprise_id = 'ent1'")
            .execute(&svc.pool)
            .await
            .unwrap();
        svc.record_turn("alice", Some("c1"), Some("claude-opus-4-8"), None, Some(100), Some(200))
            .await
            .unwrap();
        svc.record_turn("alice", Some("c1"), Some("claude-opus-4-8"), None, Some(50), Some(50))
            .await
            .unwrap();
        let summary = svc.usage_summary("ent1", 0).await.unwrap();
        assert_eq!(summary.total_turns, 2);
        assert_eq!(summary.total_tokens, 400);
        assert!(summary.estimated_cost_micros > 0);
        assert_eq!(summary.by_user.len(), 1);
        assert_eq!(summary.by_user[0].key, "alice");
        assert_eq!(summary.by_model[0].key, "claude-opus-4-8");
    }

    /// The channel dimension (P1-4) must keep three row origins apart: an
    /// enterprise channel (raw `providers.id`, `prov_chan_<channel_id>`), a
    /// personally configured provider (an id with no registry row), and rows
    /// written before the column existed (NULL). The NULL bucket is labelled
    /// `unknown` — same convention as a NULL model — and none of the three may
    /// fold into each other, or the channel report quietly overcounts one and
    /// undercounts the rest.
    #[tokio::test]
    async fn usage_summary_buckets_channel_and_never_mixes_sources() {
        let svc = service().await;
        add_members(&svc, "ent1", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'alice' WHERE enterprise_id = 'ent1'")
            .execute(&svc.pool)
            .await
            .unwrap();
        svc.record_turn(
            "alice",
            Some("c1"),
            Some("claude-opus-4-8"),
            Some("prov_chan_chanA"),
            Some(10),
            Some(10),
        )
        .await
        .unwrap();
        svc.record_turn(
            "alice",
            Some("c1"),
            Some("gpt-4"),
            Some("prov_chan_chanA"),
            Some(10),
            Some(10),
        )
        .await
        .unwrap();
        svc.record_turn(
            "alice",
            Some("c1"),
            Some("claude-opus-4-8"),
            Some("personal-provider"),
            Some(10),
            Some(10),
        )
        .await
        .unwrap();
        // Pre-P1-4 row shape: no channel at all.
        svc.record_turn("alice", Some("c1"), Some("gpt-4"), None, Some(10), Some(10))
            .await
            .unwrap();

        let summary = svc.usage_summary("ent1", 0).await.unwrap();
        assert_eq!(summary.by_channel.len(), 3, "three sources, three buckets");
        let bucket = |key: &str| {
            summary
                .by_channel
                .iter()
                .find(|b| b.key == key)
                .unwrap_or_else(|| panic!("missing channel bucket {key}"))
        };
        assert_eq!(bucket("prov_chan_chanA").turns, 2);
        assert_eq!(bucket("prov_chan_chanA").total_tokens, 40);
        assert_eq!(bucket("personal-provider").turns, 1);
        assert_eq!(bucket("unknown").turns, 1);
        // Channel is an orthogonal dimension: the model buckets are unaffected.
        assert_eq!(summary.by_model.len(), 2);
        // The raw row keeps the column verbatim — the frontend, not this
        // crate, strips the `prov_chan_` prefix and resolves display names.
        let page = svc.list_usage_events("ent1", 0, None, None, 50, 0).await.unwrap();
        let event = page
            .events
            .iter()
            .find(|e| {
                e.model.as_deref() == Some("gpt-4")
                    && e.conversation_id.as_deref() == Some("c1")
                    && e.input_tokens == Some(10)
                    && e.output_tokens == Some(10)
            })
            .unwrap();
        assert!(
            event.channel_id.is_none(),
            "None channel must read back as NULL, not 'unknown'"
        );
    }

    /// Raw-row fixtures for the P1-3 report tests. The report is a READ
    /// surface; its write path (`record_llm_call` / `record_turn`) is covered
    /// by its own tests, so these bypass enterprise resolution and place rows
    /// with exact timestamps — percentile and window math needs that control.
    async fn add_usage_event_at(
        svc: &BillingService,
        enterprise_id: &str,
        user_id: &str,
        total_tokens: i64,
        created_at: i64,
    ) {
        sqlx::query(
            "INSERT INTO one_usage_events (id, user_id, enterprise_id, total_tokens, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(generate_prefixed_id("evt"))
        .bind(user_id)
        .bind(enterprise_id)
        .bind(total_tokens)
        .bind(created_at)
        .execute(&svc.pool)
        .await
        .unwrap();
    }

    async fn add_llm_call_at(
        svc: &BillingService,
        enterprise_id: &str,
        duration_ms: Option<i64>,
        error: Option<&str>,
        created_at: i64,
    ) {
        sqlx::query(
            "INSERT INTO one_llm_calls (id, enterprise_id, user_id, duration_ms, error, created_at) \
             VALUES (?, ?, 'u1', ?, ?, ?)",
        )
        .bind(generate_prefixed_id("llmcall"))
        .bind(enterprise_id)
        .bind(duration_ms)
        .bind(error)
        .bind(created_at)
        .execute(&svc.pool)
        .await
        .unwrap();
    }

    /// WAU/MAU are FIXED trailing windows from now (not scoped by `since`):
    /// a user active only 6 days ago is weekly-active even when the report's
    /// spend window reaches further back, and a 40-day-old user is neither.
    /// Other companies' rows never leak into the count.
    #[tokio::test]
    async fn wau_and_mau_count_distinct_users_over_their_fixed_windows() {
        let svc = service().await;
        let now = now_ms();
        const DAY: i64 = 24 * 3600 * 1000;
        add_usage_event_at(&svc, "ent1", "u-now", 10, now).await;
        add_usage_event_at(&svc, "ent1", "u-6d", 10, now - 6 * DAY).await;
        add_usage_event_at(&svc, "ent1", "u-10d", 10, now - 10 * DAY).await;
        add_usage_event_at(&svc, "ent1", "u-40d", 10, now - 40 * DAY).await;
        // Another company's active user must not be counted for ent1.
        add_usage_event_at(&svc, "ent2", "u-other", 10, now).await;

        let report = svc.enterprise_report("ent1", 0).await.unwrap();
        assert_eq!(report.wau, 2, "now + 6d ago");
        assert_eq!(report.mau, 3, "now + 6d + 10d ago; 40d falls outside");
    }

    /// Per-capita tokens divide by ACTIVE users in the window; a company with
    /// no activity reports 0 (there is no average to average).
    #[tokio::test]
    async fn avg_tokens_per_user_divides_by_active_users_and_survives_zero() {
        let svc = service().await;
        let now = now_ms();
        add_usage_event_at(&svc, "ent1", "alice", 100, now).await;
        add_usage_event_at(&svc, "ent1", "bob", 300, now).await;
        add_usage_event_at(&svc, "ent1", "bob", 100, now).await;

        let report = svc.enterprise_report("ent1", 0).await.unwrap();
        assert_eq!(report.avg_tokens_per_user, 250.0, "500 tokens / 2 users");

        let empty = svc.enterprise_report("ent_empty", 0).await.unwrap();
        assert_eq!(empty.avg_tokens_per_user, 0.0, "no active users → 0, not NaN");
        assert_eq!(empty.wau, 0);
        assert_eq!(empty.mau, 0);
    }

    /// Top10 ranks by window tokens DESC and truncates at 10 — a token-heavy
    /// single user must outrank a chatty but lightweight one.
    #[tokio::test]
    async fn top_users_are_token_ranked_and_truncated_to_ten() {
        let svc = service().await;
        let now = now_ms();
        for i in 0..12_i64 {
            // 12 users, tokens 100..1200; one user gets a second turn so the
            // ranking demonstrably aggregates, not just sorts rows.
            add_usage_event_at(&svc, "ent1", &format!("user{i}"), i * 100, now).await;
            if i == 11 {
                add_usage_event_at(&svc, "ent1", "user11", 50, now).await;
            }
        }

        let report = svc.enterprise_report("ent1", 0).await.unwrap();
        assert_eq!(report.top_users.len(), 10, "truncated to ten");
        assert_eq!(report.top_users[0].user_id, "user11");
        assert_eq!(report.top_users[0].total_tokens, 1100 + 50);
        assert_eq!(report.top_users[1].user_id, "user10");
        assert_eq!(report.top_users[9].user_id, "user2", "user0/user1 fall off the list");
        assert_eq!(
            report.top_users[0].estimated_cost_micros, 0,
            "raw inserts carry no cost"
        );
    }

    /// Hand-computable percentiles: 10 measured durations [100..1900 step 200]
    /// → P50 = sorted[floor(9*0.5)] = 900, P95 = sorted[floor(9*0.95)] = 1700.
    /// Same window also feeds the call counts and the success rate.
    #[tokio::test]
    async fn latency_percentiles_success_rate_and_counts_on_a_hand_computable_sample() {
        let svc = service().await;
        let now = now_ms();
        for i in 0..10_i64 {
            let error = if i < 2 { Some("boom") } else { None };
            add_llm_call_at(&svc, "ent1", Some(100 + 200 * i), error, now).await;
        }

        let report = svc.enterprise_report("ent1", 0).await.unwrap();
        assert_eq!(report.latency_p50, Some(900));
        assert_eq!(report.latency_p95, Some(1700));
        assert_eq!(report.llm_call_count, 10);
        assert_eq!(report.llm_error_count, 2);
        assert_eq!(report.tool_success_rate, Some(0.8), "8 of 10 completed without error");
    }

    /// The honesty gate: 9 measured calls is below [`MIN_LATENCY_SAMPLES`], so
    /// both percentiles publish as `null` — but the trend still reports what
    /// was measured that day, and the success rate still exists (it is a
    /// ratio, not a distribution).
    #[tokio::test]
    async fn latency_percentiles_are_null_below_ten_samples_but_the_rest_still_reports() {
        let svc = service().await;
        let now = now_ms();
        for i in 0..9_i64 {
            add_llm_call_at(&svc, "ent1", Some(100 + i), None, now).await;
        }

        let report = svc.enterprise_report("ent1", 0).await.unwrap();
        assert_eq!(report.latency_p50, None);
        assert_eq!(report.latency_p95, None);
        assert_eq!(report.tool_success_rate, Some(1.0));
        assert_eq!(report.llm_call_count, 9);
        assert_eq!(report.latency_trend.len(), 1);
        assert_eq!(report.latency_trend[0].samples, 9);
    }

    /// NULL durations (delegates, unmeasured attempts) are excluded from the
    /// percentile inputs — they are "not measured", not "measured at 0".
    #[tokio::test]
    async fn null_durations_do_not_count_as_zero_in_percentiles() {
        let svc = service().await;
        let now = now_ms();
        for i in 0..9_i64 {
            add_llm_call_at(&svc, "ent1", Some(100 + 200 * i), None, now).await;
        }
        // One delegate/unmeasured row on top: with it the table has 10 calls
        // but still only 9 measured durations, so the gate must stay shut.
        add_llm_call_at(&svc, "ent1", None, None, now).await;

        let report = svc.enterprise_report("ent1", 0).await.unwrap();
        assert_eq!(report.llm_call_count, 10);
        assert_eq!(report.latency_p50, None, "9 measured < 10 measured");
    }

    /// The trend buckets per UTC day and each day reports ITS OWN percentiles
    /// from exactly the samples it contains.
    #[tokio::test]
    async fn latency_trend_buckets_by_utc_day() {
        let svc = service().await;
        let now = now_ms();
        const DAY: i64 = 24 * 3600 * 1000;
        add_llm_call_at(&svc, "ent1", Some(100), None, now).await;
        add_llm_call_at(&svc, "ent1", Some(300), None, now).await;
        add_llm_call_at(&svc, "ent1", Some(1000), None, now - DAY).await;

        let report = svc.enterprise_report("ent1", 0).await.unwrap();
        assert_eq!(report.latency_trend.len(), 2, "two distinct days");
        let today = report.latency_trend.iter().find(|p| p.samples == 2).unwrap();
        assert_eq!(today.p50, 100, "sorted[0] of a 2-sample day");
        assert_eq!(today.p95, 100);
        let yesterday = report.latency_trend.iter().find(|p| p.samples == 1).unwrap();
        assert_eq!(yesterday.p50, 1000);
        assert_eq!(yesterday.p95, 1000);
        assert!(
            report
                .latency_trend
                .iter()
                .all(|p| p.day.len() == 10 && p.day.as_bytes()[4] == b'-'),
            "day keys are YYYY-MM-DD"
        );
    }

    /// A brand-new company (personal install, empty tables) must get a
    /// well-formed report — every percentile null, every count zero, no 500.
    #[tokio::test]
    async fn empty_tables_report_nulls_and_zeros_without_erroring() {
        let svc = service().await;

        let report = svc.enterprise_report("ent1", 0).await.unwrap();
        assert_eq!(report.wau, 0);
        assert_eq!(report.mau, 0);
        assert_eq!(report.avg_tokens_per_user, 0.0);
        assert_eq!(report.latency_p50, None);
        assert_eq!(report.latency_p95, None);
        assert!(report.latency_trend.is_empty());
        assert_eq!(report.tool_success_rate, None, "no calls → no rate");
        assert!(report.top_users.is_empty());
        assert_eq!(report.llm_call_count, 0);
        assert_eq!(report.llm_error_count, 0);
    }

    #[tokio::test]
    async fn list_usage_events_filters_and_paginates() {
        let svc = service().await;
        add_members(&svc, "ent1", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'alice' WHERE enterprise_id = 'ent1'")
            .execute(&svc.pool)
            .await
            .unwrap();
        for i in 0..3 {
            svc.record_turn(
                "alice",
                Some("c1"),
                Some("claude-opus-4-8"),
                None,
                Some(10 + i),
                Some(20),
            )
            .await
            .unwrap();
        }
        svc.record_turn("alice", Some("c1"), Some("gpt-4"), None, Some(5), Some(5))
            .await
            .unwrap();

        let all = svc.list_usage_events("ent1", 0, None, None, 50, 0).await.unwrap();
        assert_eq!(all.total, 4);
        assert_eq!(all.events.len(), 4);
        // Newest first.
        assert!(all.events[0].created_at >= all.events[3].created_at);

        let by_model = svc
            .list_usage_events("ent1", 0, None, Some("gpt-4"), 50, 0)
            .await
            .unwrap();
        assert_eq!(by_model.total, 1);
        assert_eq!(by_model.events[0].model.as_deref(), Some("gpt-4"));

        let page1 = svc.list_usage_events("ent1", 0, None, None, 2, 0).await.unwrap();
        let page2 = svc.list_usage_events("ent1", 0, None, None, 2, 2).await.unwrap();
        assert_eq!(
            page1.total, 4,
            "total reflects the whole filtered set, not just this page"
        );
        assert_eq!(page1.events.len(), 2);
        assert_eq!(page2.events.len(), 2);
        let page1_ids: std::collections::HashSet<_> = page1.events.iter().map(|e| e.id.clone()).collect();
        let page2_ids: std::collections::HashSet<_> = page2.events.iter().map(|e| e.id.clone()).collect();
        assert!(page1_ids.is_disjoint(&page2_ids), "pages must not overlap");
    }

    #[tokio::test]
    async fn list_usage_events_is_scoped_per_enterprise() {
        let svc = service().await;
        add_members(&svc, "ent1", 1).await;
        add_members(&svc, "ent2", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'alice' WHERE enterprise_id = 'ent1'")
            .execute(&svc.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'bob' WHERE enterprise_id = 'ent2'")
            .execute(&svc.pool)
            .await
            .unwrap();
        svc.record_turn("alice", Some("c1"), Some("claude-opus-4-8"), None, Some(10), Some(10))
            .await
            .unwrap();
        svc.record_turn("bob", Some("c2"), Some("claude-opus-4-8"), None, Some(10), Some(10))
            .await
            .unwrap();

        let ent1_events = svc.list_usage_events("ent1", 0, None, None, 50, 0).await.unwrap();
        assert_eq!(ent1_events.total, 1);
        assert_eq!(ent1_events.events[0].user_id, "alice");
    }

    #[tokio::test]
    async fn list_sessions_groups_by_conversation_and_lists_every_model_used() {
        let svc = service().await;
        add_members(&svc, "ent1", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'alice' WHERE enterprise_id = 'ent1'")
            .execute(&svc.pool)
            .await
            .unwrap();
        svc.record_turn("alice", Some("c1"), Some("claude-opus-4-8"), None, Some(100), Some(100))
            .await
            .unwrap();
        svc.record_turn("alice", Some("c1"), Some("gpt-4"), None, Some(50), Some(50))
            .await
            .unwrap();
        svc.record_turn("alice", Some("c2"), Some("claude-opus-4-8"), None, Some(10), Some(10))
            .await
            .unwrap();
        // No conversation_id: must be excluded from sessions, not merged into
        // a misleading "no conversation" bucket.
        svc.record_turn("alice", None, Some("claude-opus-4-8"), None, Some(999), Some(999))
            .await
            .unwrap();

        let page = svc.list_sessions("ent1", 0, 50, 0).await.unwrap();
        assert_eq!(page.total, 2);
        let c1 = page.sessions.iter().find(|s| s.conversation_id == "c1").unwrap();
        assert_eq!(c1.turn_count, 2);
        assert_eq!(c1.total_tokens, 300);
        assert_eq!(c1.user_id, "alice");
        let mut models = c1.models.clone();
        models.sort();
        assert_eq!(models, vec!["claude-opus-4-8".to_owned(), "gpt-4".to_owned()]);

        let c2 = page.sessions.iter().find(|s| s.conversation_id == "c2").unwrap();
        assert_eq!(c2.turn_count, 1);
    }

    /// dream conversations have no other way to surface a session cost (the
    /// ACP-only `/usage` snapshot structurally never fires for them — see
    /// `AgentInstance::get_usage`), so the frontend sums this directly.
    /// Personal/no-enterprise users must work too: `conversation_cost` is
    /// scoped by `user_id`, not by company membership.
    #[tokio::test]
    async fn conversation_cost_sums_every_turn_for_that_conversation() {
        let svc = service().await;
        svc.record_turn(
            "solo",
            Some("conv_x"),
            Some("claude-opus-4-8"),
            None,
            Some(100),
            Some(200),
        )
        .await
        .unwrap();
        svc.record_turn(
            "solo",
            Some("conv_x"),
            Some("claude-opus-4-8"),
            None,
            Some(50),
            Some(50),
        )
        .await
        .unwrap();
        // A different conversation for the same user must not bleed in.
        svc.record_turn(
            "solo",
            Some("conv_y"),
            Some("claude-opus-4-8"),
            None,
            Some(9_999),
            Some(9_999),
        )
        .await
        .unwrap();

        let cost = svc.conversation_cost("solo", "conv_x").await.unwrap();
        assert!(cost > 0);

        let single_turn_cost = svc.conversation_cost("solo", "conv_y").await.unwrap();
        // conv_y's lone turn has far more tokens than conv_x's two turns
        // combined; if the query were not scoping by conversation_id, conv_x's
        // total would exceed it instead of the other way around.
        assert!(single_turn_cost > cost);
    }

    #[tokio::test]
    async fn conversation_cost_is_zero_for_a_conversation_with_no_turns_yet() {
        let svc = service().await;
        assert_eq!(svc.conversation_cost("solo", "brand_new_conv").await.unwrap(), 0);
    }

    /// The dashboard has to name the models nothing priced, because a zero-cost
    /// media call consumes none of the spend cap — the cap quietly stops binding
    /// for that model, and the only fix is an admin entering a unit price. This
    /// is the visibility half of that (option 2), deliberately instead of
    /// inventing a fallback rate.
    #[tokio::test]
    async fn usage_summary_names_the_media_models_nothing_priced() {
        let svc = service().await;
        add_members(&svc, "ent1", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'alice' WHERE enterprise_id = 'ent1'")
            .execute(&svc.pool)
            .await
            .unwrap();

        let usage = |model: &'static str, price: Option<i64>| MediaUsage {
            user_id: "alice",
            kind: "image",
            model,
            count: 1,
            duration_seconds: 0,
            unit_price_micros: price,
            conversation_id: None,
        };

        // Nothing prices these two.
        svc.record_media_usage(usage("our-gateway-name-a", None)).await.unwrap();
        svc.record_media_usage(usage("our-gateway-name-b", None)).await.unwrap();
        // The built-in table prices this one…
        svc.record_media_usage(usage("gpt-image-2", None)).await.unwrap();
        // …and the admin priced this one themselves.
        svc.record_media_usage(usage("another-gateway-name", Some(50_000)))
            .await
            .unwrap();
        // A chat turn must not be mistaken for unpriced media.
        svc.record_turn("alice", Some("c1"), Some("some-chat-model"), None, Some(10), Some(10))
            .await
            .unwrap();

        let summary = svc.usage_summary("ent1", 0).await.unwrap();
        assert_eq!(summary.unpriced_media_calls, 2, "only the two nothing priced");
        let mut named = summary.unpriced_media_models.clone();
        named.sort();
        assert_eq!(named, vec!["our-gateway-name-a", "our-gateway-name-b"]);
    }

    /// A company that prices everything must see a clean dashboard — otherwise
    /// the warning becomes noise everyone learns to ignore.
    #[tokio::test]
    async fn a_fully_priced_company_sees_no_warning() {
        let svc = service().await;
        add_members(&svc, "ent1", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'alice' WHERE enterprise_id = 'ent1'")
            .execute(&svc.pool)
            .await
            .unwrap();
        svc.record_media_usage(MediaUsage {
            user_id: "alice",
            kind: "video",
            model: "seedance-2-0-fast",
            count: 1,
            duration_seconds: 5,
            unit_price_micros: None,
            conversation_id: None,
        })
        .await
        .unwrap();

        let summary = svc.usage_summary("ent1", 0).await.unwrap();
        assert_eq!(summary.unpriced_media_calls, 0);
        assert!(summary.unpriced_media_models.is_empty());
    }

    #[tokio::test]
    async fn manual_checkout_is_stubbed() {
        let svc = service().await;
        let result = svc.create_checkout("ent1", "team");
        assert_eq!(result.status, "manual");
        assert!(result.checkout_url.is_none());
    }

    #[tokio::test]
    async fn model_control_gates_send_by_allowlist_and_budget() {
        let svc = service().await;
        // Red line: no company → always allowed.
        assert!(svc.check_send_allowed("nobody", Some("gpt-4")).await.is_ok());

        add_members(&svc, "entX", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'zoe' WHERE enterprise_id = 'entX'")
            .execute(&svc.pool)
            .await
            .unwrap();

        // Allowlist: only claude-opus-4-8 permitted.
        svc.set_model_control("entX", None, &["claude-opus-4-8".to_owned()])
            .await
            .unwrap();
        assert!(svc.check_send_allowed("zoe", Some("claude-opus-4-8")).await.is_ok());
        assert_eq!(
            svc.check_send_allowed("zoe", Some("gpt-4")).await.unwrap_err().code(),
            "MODEL_NOT_ALLOWED"
        );
        // Unknown model (None) can't be checked → passes the allowlist.
        assert!(svc.check_send_allowed("zoe", None).await.is_ok());
        // Dedicated allowlist-only check (model-switch layer).
        assert!(svc.check_model_allowed("zoe", "claude-opus-4-8").await.is_ok());
        assert_eq!(
            svc.check_model_allowed("zoe", "gpt-4").await.unwrap_err().code(),
            "MODEL_NOT_ALLOWED"
        );
        assert!(svc.check_model_allowed("nobody", "anything").await.is_ok()); // personal red line

        // Spend cap: clear the allowlist, set a tiny cap, then overspend.
        svc.set_model_control("entX", Some(100), &[]).await.unwrap();
        assert!(svc.check_send_allowed("zoe", Some("gpt-4")).await.is_ok()); // under budget so far
        svc.record_turn("zoe", Some("c1"), Some("claude-opus-4-8"), None, Some(1000), Some(1000))
            .await
            .unwrap(); // ~90000 micros >> 100
        assert_eq!(
            svc.check_send_allowed("zoe", Some("claude-opus-4-8"))
                .await
                .unwrap_err()
                .code(),
            "BUDGET_EXCEEDED"
        );

        // The plan surfaces the cap + spend.
        let plan = svc.plan("entX").await.unwrap();
        assert_eq!(plan.cost_cap_micros, Some(100));
        assert!(plan.cost_used_micros >= 100);
    }

    async fn add_pending_member(svc: &BillingService, enterprise_id: &str, user_id: &str) {
        sqlx::query(
            "INSERT INTO one_enterprise_members (user_id, enterprise_id, role, seat_status, joined_at, updated_at) \
             VALUES (?, ?, 'member', 'pending', 0, 0)",
        )
        .bind(user_id)
        .bind(enterprise_id)
        .execute(&svc.pool)
        .await
        .unwrap();
    }

    /// ⚠️ T6-4, the regression this whole column exists to close. Before it,
    /// a member arriving over the seat cap got no row at all, so
    /// `resolve_enterprise_id` found nothing and treated them as personal —
    /// every gate here would have returned `Ok(())`. The company deliberately
    /// has NEITHER an allowlist NOR a spend cap configured (the worst case:
    /// the two checks below this point would pass literally everyone), so a
    /// green result here can only mean the seat check itself is doing its job.
    #[tokio::test]
    async fn a_pending_member_is_denied_even_with_no_allowlist_or_cap_configured() {
        let svc = service().await;
        add_pending_member(&svc, "entP", "waiting").await;

        assert_eq!(
            svc.check_send_allowed("waiting", Some("gpt-4"))
                .await
                .unwrap_err()
                .code(),
            "SEAT_LIMIT_EXCEEDED"
        );
        assert_eq!(
            svc.check_model_allowed("waiting", "gpt-4").await.unwrap_err().code(),
            "SEAT_LIMIT_EXCEEDED"
        );
        assert_eq!(
            svc.check_media_allowed("waiting", "seedance-2-0-fast")
                .await
                .unwrap_err()
                .code(),
            "SEAT_LIMIT_EXCEEDED"
        );

        // An ACTIVE member in the very same, still-unconfigured company passes
        // — the denial is about this one user's seat, not a company-wide lock.
        add_members(&svc, "entP", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'seated' WHERE enterprise_id = 'entP' AND user_id != 'waiting'")
            .execute(&svc.pool)
            .await
            .unwrap();
        assert!(svc.check_send_allowed("seated", Some("gpt-4")).await.is_ok());
    }

    /// `seat_used` feeds the "X / Y seats" dashboard number and `can_add_seat`;
    /// both must count only ACTIVE seats, or a pending row would either read as
    /// consumed capacity that was never actually granted, or (worse) make
    /// `can_add_seat` refuse forever since a pending row never goes away on its
    /// own.
    #[tokio::test]
    async fn seat_used_and_pending_are_reported_and_counted_separately() {
        let svc = service().await;
        force_tier(&svc, "entP", Tier::Free, None).await;
        sqlx::query("UPDATE one_enterprise_license SET seat_limit = 2 WHERE enterprise_id = 'entP'")
            .execute(&svc.pool)
            .await
            .unwrap();
        add_members(&svc, "entP", 2).await; // both default to 'active'
        add_pending_member(&svc, "entP", "waiting1").await;
        add_pending_member(&svc, "entP", "waiting2").await;

        let plan = svc.plan("entP").await.unwrap();
        assert_eq!(plan.seat_used, 2, "pending rows must not inflate the billed seat count");
        assert_eq!(plan.seat_pending, 2);
        assert_eq!(plan.seat_limit, Some(2));

        // At the cap with 2 pending on top — still full, not "4/2 so there's
        // negative room somehow".
        assert!(!svc.can_add_seat(Some("entP")).await.unwrap());
    }

    /// Billing is enterprise-scoped, so the guard must be too.
    ///
    /// The interesting case is `org_admin`: they administer ONE project group,
    /// but tier / seat cap / spend cap / model allowlist apply to the whole
    /// company. Letting them through would mean group A's admin can move the
    /// budget for every other group. `system_admin` is still accepted because
    /// that is the machine owner on a personal or single-server install, where
    /// no company row exists at all.
    #[tokio::test]
    async fn billing_admin_is_enterprise_scoped_not_project_group_scoped() {
        let svc = service().await;
        sqlx::raw_sql(
            "CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL, PRIMARY KEY (user_id, tenant_id));
             CREATE TABLE one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL);
             INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('group_admin', 't1', 'org_admin');
             INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('machine_owner', 't1', 'system_admin');
             INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('plain', 't1', 'member');
             INSERT INTO one_enterprise_members (user_id, enterprise_id, role, joined_at, updated_at) VALUES ('company_admin', 'entA', 'admin', 0, 0);",
        )
        .execute(&svc.pool)
        .await
        .unwrap();

        assert!(
            svc.is_billing_admin("company_admin").await.unwrap(),
            "a company admin owns the company's plan"
        );
        assert!(
            svc.is_billing_admin("machine_owner").await.unwrap(),
            "system_admin must keep working — personal installs have no company row"
        );
        assert!(
            !svc.is_billing_admin("group_admin").await.unwrap(),
            "a project-group admin must not be able to move the whole company's budget"
        );
        assert!(!svc.is_billing_admin("plain").await.unwrap());
    }
    /// Media generation reaches providers through the built-in MCP tool, which
    /// never passed through `SendGate` — so until the precheck existed, the
    /// priciest calls in the product ran outside the allowlist and the cap.
    #[tokio::test]
    async fn media_generation_obeys_the_same_allowlist_and_budget_as_chat() {
        let svc = service().await;
        add_members(&svc, "entM", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'mia' WHERE enterprise_id = 'entM'")
            .execute(&svc.pool)
            .await
            .unwrap();

        // Allowlist covers media models by the same rule as chat models.
        svc.set_model_control("entM", None, &["seedance-2-0-fast".to_owned()])
            .await
            .unwrap();
        assert!(svc.check_media_allowed("mia", "seedance-2-0-fast").await.is_ok());
        assert_eq!(
            svc.check_media_allowed("mia", "gpt-image-2").await.unwrap_err().code(),
            "MODEL_NOT_ALLOWED"
        );

        // Personal / no-company users are never gated — the standing red line.
        assert!(svc.check_media_allowed("nobody", "anything").await.is_ok());

        // Recorded media spend counts against the very same budget as chat, so
        // an expensive video cannot hide from the cap.
        svc.set_model_control("entM", Some(1_000), &[]).await.unwrap();
        assert!(svc.check_media_allowed("mia", "seedance-2-0-fast").await.is_ok());
        svc.record_media_usage(MediaUsage {
            user_id: "mia",
            kind: "video",
            model: "seedance-2-0-fast",
            count: 1,
            duration_seconds: 5,
            unit_price_micros: None,
            conversation_id: Some("conv_1"),
        })
        .await
        .unwrap();
        assert_eq!(
            svc.check_media_allowed("mia", "seedance-2-0-fast")
                .await
                .unwrap_err()
                .code(),
            "BUDGET_EXCEEDED"
        );

        // And it is visible in the dashboard rollup, not just the gate.
        let plan = svc.plan("entM").await.unwrap();
        assert!(plan.cost_used_micros >= 1_000);
    }

    /// The built-in rate table is a coarse illustration; a price the user
    /// entered for their own provider is the contract they are actually billed
    /// under, so it must win.
    #[tokio::test]
    async fn a_user_supplied_price_overrides_the_built_in_rate_table() {
        let svc = service().await;

        // Video: priced per second, so 4s at 2 USD-micros/s is 8.
        svc.record_media_usage(MediaUsage {
            user_id: "solo",
            kind: "video",
            model: "some-unknown-model",
            count: 1,
            duration_seconds: 4,
            unit_price_micros: Some(2),
            conversation_id: None,
        })
        .await
        .unwrap();
        let cost: i64 = sqlx::query_scalar(
            "SELECT estimated_cost_micros FROM one_usage_events WHERE user_id = 'solo' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(&svc.pool)
        .await
        .unwrap();
        // Without a price this model is unknown to the table and would cost 0.
        assert_eq!(cost, 8);

        // Images: priced per asset.
        svc.record_media_usage(MediaUsage {
            user_id: "solo2",
            kind: "image",
            model: "some-unknown-model",
            count: 3,
            duration_seconds: 0,
            unit_price_micros: Some(7),
            conversation_id: None,
        })
        .await
        .unwrap();
        let cost2: i64 =
            sqlx::query_scalar("SELECT estimated_cost_micros FROM one_usage_events WHERE user_id = 'solo2' LIMIT 1")
                .fetch_one(&svc.pool)
                .await
                .unwrap();
        assert_eq!(cost2, 21);

        // A zero / absent price falls back to the table rather than charging 0.
        svc.record_media_usage(MediaUsage {
            user_id: "solo3",
            kind: "image",
            model: "gpt-image-2",
            count: 1,
            duration_seconds: 0,
            unit_price_micros: Some(0),
            conversation_id: None,
        })
        .await
        .unwrap();
        let cost3: i64 =
            sqlx::query_scalar("SELECT estimated_cost_micros FROM one_usage_events WHERE user_id = 'solo3' LIMIT 1")
                .fetch_one(&svc.pool)
                .await
                .unwrap();
        assert_eq!(cost3, 40_000);
    }

    /// Media has no token counts; charging it at a token rate would report zero
    /// and let the most expensive calls sit invisibly under any cap.
    #[tokio::test]
    async fn media_usage_is_priced_even_though_it_has_no_tokens() {
        let svc = service().await;
        svc.record_media_usage(MediaUsage {
            user_id: "solo",
            kind: "image",
            model: "gpt-image-2",
            count: 3,
            duration_seconds: 0,
            unit_price_micros: None,
            conversation_id: None,
        })
        .await
        .unwrap();
        let cost: i64 = sqlx::query_scalar(
            "SELECT estimated_cost_micros FROM one_usage_events WHERE user_id = 'solo' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(&svc.pool)
        .await
        .unwrap();
        assert_eq!(cost, 3 * 40_000);

        // Token columns stay NULL: media is metered per asset, not per token.
        let tokens: Option<i64> =
            sqlx::query_scalar("SELECT total_tokens FROM one_usage_events WHERE user_id = 'solo' LIMIT 1")
                .fetch_one(&svc.pool)
                .await
                .unwrap();
        assert!(tokens.is_none());
    }

    /// Zero-cost media is not a cosmetic reporting issue: it consumes none of
    /// the spend cap, so the cap quietly stops binding for that model. Pinned
    /// because the built-in table matches on model *name* and a gateway with its
    /// own naming — the common case — misses every entry.
    #[tokio::test]
    async fn an_unrecognised_media_model_costs_nothing_against_the_cap() {
        let svc = service().await;
        svc.record_media_usage(MediaUsage {
            user_id: "solo",
            kind: "image",
            model: "our-gateways-own-name",
            count: 1,
            duration_seconds: 0,
            unit_price_micros: None,
            conversation_id: None,
        })
        .await
        .unwrap();
        let cost: i64 = sqlx::query_scalar(
            "SELECT estimated_cost_micros FROM one_usage_events WHERE user_id = 'solo' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(&svc.pool)
        .await
        .unwrap();
        assert_eq!(
            cost, 0,
            "if this ever becomes non-zero, a rate was invented — that is a pricing decision"
        );

        // …and the escape hatch that makes it countable is the user's own price.
        svc.record_media_usage(MediaUsage {
            user_id: "solo",
            kind: "image",
            model: "our-gateways-own-name",
            count: 2,
            duration_seconds: 0,
            unit_price_micros: Some(30_000),
            conversation_id: None,
        })
        .await
        .unwrap();
        let priced: i64 = sqlx::query_scalar(
            "SELECT estimated_cost_micros FROM one_usage_events WHERE user_id = 'solo' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(&svc.pool)
        .await
        .unwrap();
        assert_eq!(priced, 60_000);
    }

    /// A charge an admin cannot trace back to anywhere is a charge they cannot
    /// act on. Media started from the compose box writes no conversation
    /// message at all, so this column is the only trail it leaves — it used to
    /// be hard-coded NULL for every media row.
    #[tokio::test]
    async fn media_usage_is_attributed_to_its_conversation() {
        let svc = service().await;
        svc.record_media_usage(MediaUsage {
            user_id: "solo",
            kind: "video",
            model: "seedance-2-0-fast",
            count: 1,
            duration_seconds: 5,
            unit_price_micros: None,
            conversation_id: Some("conv_abc"),
        })
        .await
        .unwrap();
        let conversation: Option<String> = sqlx::query_scalar(
            "SELECT conversation_id FROM one_usage_events WHERE user_id = 'solo' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(&svc.pool)
        .await
        .unwrap();
        assert_eq!(conversation.as_deref(), Some("conv_abc"));

        // Still optional: a caller without one records the spend anyway rather
        // than dropping it, because the money was spent either way.
        svc.record_media_usage(MediaUsage {
            user_id: "solo2",
            kind: "image",
            model: "gpt-image-2",
            count: 1,
            duration_seconds: 0,
            unit_price_micros: None,
            conversation_id: None,
        })
        .await
        .unwrap();
        let none_conversation: Option<String> = sqlx::query_scalar(
            "SELECT conversation_id FROM one_usage_events WHERE user_id = 'solo2' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_one(&svc.pool)
        .await
        .unwrap();
        assert!(none_conversation.is_none());
    }

    /// Minimal `one_user_org` shape for T7 tests — this crate doesn't own the
    /// table (one-org does) so it isn't created by `run_one_billing_migrations`;
    /// tests exercising department resolution must stand up their own copy,
    /// same as `billing_admin_is_enterprise_scoped_not_project_group_scoped`
    /// already does for role resolution.
    async fn add_user_org(svc: &BillingService, user_id: &str, department_id: Option<&str>) {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL, department_id TEXT, PRIMARY KEY (user_id, tenant_id))",
        )
        .execute(&svc.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO one_user_org (user_id, tenant_id, role, department_id) VALUES (?, 't1', 'member', ?)")
            .bind(user_id)
            .bind(department_id)
            .execute(&svc.pool)
            .await
            .unwrap();
    }

    /// T7: `record_turn` and `record_media_usage` both stamp the user's
    /// CURRENT department at write time. A user with no `one_user_org` row
    /// (or no department) gets a NULL — never an error, since one-billing
    /// must keep working for personal/standalone installs that have no
    /// one-org table at all.
    #[tokio::test]
    async fn department_id_is_stamped_at_record_time() {
        let svc = service().await;
        add_user_org(&svc, "dana", Some("deptA")).await;

        svc.record_turn("dana", None, Some("gpt-4"), None, Some(10), Some(10))
            .await
            .unwrap();
        svc.record_media_usage(MediaUsage {
            user_id: "dana",
            kind: "image",
            model: "gpt-image-2",
            count: 1,
            duration_seconds: 0,
            unit_price_micros: None,
            conversation_id: None,
        })
        .await
        .unwrap();

        let depts: Vec<Option<String>> =
            sqlx::query_scalar("SELECT department_id FROM one_usage_events WHERE user_id = 'dana' ORDER BY created_at")
                .fetch_all(&svc.pool)
                .await
                .unwrap();
        assert_eq!(depts, vec![Some("deptA".to_owned()), Some("deptA".to_owned())]);

        // No one_user_org row at all → NULL, not an error.
        svc.record_turn("no_org_row", None, Some("gpt-4"), None, Some(10), Some(10))
            .await
            .unwrap();
        let none_dept: Option<String> =
            sqlx::query_scalar("SELECT department_id FROM one_usage_events WHERE user_id = 'no_org_row' LIMIT 1")
                .fetch_one(&svc.pool)
                .await
                .unwrap();
        assert!(none_dept.is_none());
    }

    /// The department cap is a tighter constraint layered UNDER the
    /// company-wide one: it must bind even when no company cap is configured
    /// at all, and it must leave members of other departments (and members
    /// with no department) untouched.
    #[tokio::test]
    async fn department_budget_gates_sends_independently_of_company_wide_budget() {
        let svc = service().await;
        add_members(&svc, "entD", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'dana' WHERE enterprise_id = 'entD'")
            .execute(&svc.pool)
            .await
            .unwrap();
        add_user_org(&svc, "dana", Some("deptA")).await;
        add_user_org(&svc, "gio", None).await; // same company, no department

        // No company-wide cap anywhere; only a department cap.
        svc.set_department_budget("entD", "deptA", Some(100)).await.unwrap();
        assert!(svc.check_send_allowed("dana", Some("gpt-4")).await.is_ok());

        svc.record_turn(
            "dana",
            Some("c1"),
            Some("claude-opus-4-8"),
            None,
            Some(1000),
            Some(1000),
        )
        .await
        .unwrap(); // ~90000 micros >> 100
        assert_eq!(
            svc.check_send_allowed("dana", Some("gpt-4")).await.unwrap_err().code(),
            "DEPARTMENT_BUDGET_EXCEEDED"
        );
        // check_media_allowed delegates to check_send_allowed, so media is gated too.
        assert_eq!(
            svc.check_media_allowed("dana", "gpt-4").await.unwrap_err().code(),
            "DEPARTMENT_BUDGET_EXCEEDED"
        );

        // A member with no department is never subject to a department cap.
        assert!(svc.check_send_allowed("gio", Some("gpt-4")).await.is_ok());
    }

    #[tokio::test]
    async fn list_department_budgets_reports_cap_and_usage() {
        let svc = service().await;
        add_user_org(&svc, "dana", Some("deptA")).await;
        svc.set_department_budget("entD", "deptA", Some(500)).await.unwrap();
        svc.record_turn(
            "dana",
            Some("c1"),
            Some("claude-opus-4-8"),
            None,
            Some(1000),
            Some(1000),
        )
        .await
        .unwrap();

        let budgets = svc.list_department_budgets("entD").await.unwrap();
        assert_eq!(budgets.len(), 1);
        assert_eq!(budgets[0].department_id, "deptA");
        assert_eq!(budgets[0].cost_cap_micros, Some(500));
        assert!(budgets[0].cost_used_micros > 0);

        // Clearing the cap keeps the row (still reportable) but drops the cap.
        svc.set_department_budget("entD", "deptA", None).await.unwrap();
        let cleared = svc.list_department_budgets("entD").await.unwrap();
        assert_eq!(cleared[0].cost_cap_micros, None);
    }

    /// Write-time denormalization, not a live join: past spend must stay
    /// attributed to the department a user was in AT THE TIME, or reassigning
    /// someone would retroactively move last month's spend to their new
    /// department's cap.
    #[tokio::test]
    async fn reassigning_a_users_department_does_not_reshuffle_past_spend() {
        let svc = service().await;
        add_user_org(&svc, "dana", Some("deptA")).await;
        svc.record_turn(
            "dana",
            Some("c1"),
            Some("claude-opus-4-8"),
            None,
            Some(1000),
            Some(1000),
        )
        .await
        .unwrap();

        // Move dana to deptB going forward.
        sqlx::query("UPDATE one_user_org SET department_id = 'deptB' WHERE user_id = 'dana'")
            .execute(&svc.pool)
            .await
            .unwrap();
        svc.record_turn(
            "dana",
            Some("c2"),
            Some("claude-opus-4-8"),
            None,
            Some(1000),
            Some(1000),
        )
        .await
        .unwrap();

        let depts: Vec<Option<String>> =
            sqlx::query_scalar("SELECT department_id FROM one_usage_events WHERE user_id = 'dana' ORDER BY created_at")
                .fetch_all(&svc.pool)
                .await
                .unwrap();
        assert_eq!(depts, vec![Some("deptA".to_owned()), Some("deptB".to_owned())]);
    }

    /// T8: enterprise-scoped only — a personal/no-company user's generation
    /// must never create a ledger row. Same red line every other governance
    /// surface in this crate honors.
    #[tokio::test]
    async fn record_media_asset_is_a_noop_for_personal_users() {
        let svc = service().await;
        svc.record_media_asset(
            "nobody",
            "image",
            Some("gpt-image-2"),
            "/tmp/img.png",
            Some("a cat"),
            None,
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_media_assets")
            .fetch_one(&svc.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    /// Prompt retention defaults to OFF and is enforced server-side: the
    /// caller can send whatever it wants, only the company's own opt-in
    /// decides whether it lands in the column.
    #[tokio::test]
    async fn media_asset_prompt_is_retained_only_after_company_opts_in() {
        let svc = service().await;
        add_members(&svc, "entL", 1).await;
        sqlx::query("UPDATE one_enterprise_members SET user_id = 'lee' WHERE enterprise_id = 'entL'")
            .execute(&svc.pool)
            .await
            .unwrap();
        add_user_org(&svc, "lee", Some("deptL")).await;

        // Default: prompt is dropped even though the caller sent one.
        svc.record_media_asset(
            "lee",
            "image",
            Some("gpt-image-2"),
            "/w/a.png",
            Some("a red fox"),
            Some("c1"),
        )
        .await
        .unwrap();
        let prompt_before: Option<String> =
            sqlx::query_scalar("SELECT prompt FROM one_media_assets WHERE file_path = '/w/a.png'")
                .fetch_one(&svc.pool)
                .await
                .unwrap();
        assert!(prompt_before.is_none());

        // Opt in, then the same caller behavior actually retains it.
        svc.set_media_ledger_retain_prompts("entL", true).await.unwrap();
        assert!(svc.media_ledger_retain_prompts("entL").await.unwrap());
        svc.record_media_asset(
            "lee",
            "image",
            Some("gpt-image-2"),
            "/w/b.png",
            Some("a blue fox"),
            Some("c1"),
        )
        .await
        .unwrap();
        let prompt_after: Option<String> =
            sqlx::query_scalar("SELECT prompt FROM one_media_assets WHERE file_path = '/w/b.png'")
                .fetch_one(&svc.pool)
                .await
                .unwrap();
        assert_eq!(prompt_after.as_deref(), Some("a blue fox"));

        // Department was resolved and attached, same as T7's usage rows.
        let department: Option<String> =
            sqlx::query_scalar("SELECT department_id FROM one_media_assets WHERE file_path = '/w/b.png'")
                .fetch_one(&svc.pool)
                .await
                .unwrap();
        assert_eq!(department.as_deref(), Some("deptL"));
    }

    #[tokio::test]
    async fn list_media_assets_filters_by_kind_model_user_since_and_prompt() {
        let svc = service().await;
        svc.set_media_ledger_retain_prompts("entS", true).await.unwrap();
        // `record_media_asset` requires a real enterprise membership row to
        // attribute to (T7/T8 both no-op for personal users) — seeding rows
        // directly here keeps the test focused on `list_media_assets`'s own
        // filter logic rather than membership setup.
        let now = dream_core_common::now_ms();
        for (i, (user, kind, model, path, prompt)) in [
            ("ann", "image", "gpt-image-2", "/w/1.png", "a cat in a hat"),
            ("ann", "video", "seedance-2-0-fast", "/w/2.mp4", "a cat running"),
            ("bo", "image", "flux-pro", "/w/3.png", "a dog"),
        ]
        .into_iter()
        .enumerate()
        {
            sqlx::query(
                "INSERT INTO one_media_assets (id, user_id, enterprise_id, department_id, conversation_id, kind, model, file_path, prompt, created_at) \
                 VALUES (?, ?, 'entS', NULL, NULL, ?, ?, ?, ?, ?)",
            )
            .bind(format!("media_{i}"))
            .bind(user)
            .bind(kind)
            .bind(model)
            .bind(path)
            .bind(prompt)
            .bind(now + i as i64)
            .execute(&svc.pool)
            .await
            .unwrap();
        }

        let by_kind = svc
            .list_media_assets(
                "entS",
                MediaAssetFilters {
                    kind: Some("video"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].file_path, "/w/2.mp4");

        let by_user = svc
            .list_media_assets(
                "entS",
                MediaAssetFilters {
                    user_id: Some("ann"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(by_user.len(), 2);

        let by_prompt = svc
            .list_media_assets(
                "entS",
                MediaAssetFilters {
                    prompt_contains: Some("cat"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(by_prompt.len(), 2);

        let by_since = svc
            .list_media_assets(
                "entS",
                MediaAssetFilters {
                    since: Some(now + 2),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(by_since.len(), 1);
        assert_eq!(by_since[0].file_path, "/w/3.png");

        // A company that never opted into retention has NULL prompts, so a
        // prompt search finds nothing — no special-casing, just how NULL LIKE
        // behaves.
        let unretained = svc
            .list_media_assets(
                "entU",
                MediaAssetFilters {
                    prompt_contains: Some("cat"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(unretained.is_empty());
    }

    /// `activate_license` needs a key signed by the vendor's (deliberately
    /// offline, never-committed) private key, so it can't run end to end in a
    /// unit test — this exercises `active_license`'s own read/deserialize
    /// path directly against a row shaped the way `activate_license` writes
    /// one (billing_006's E4 columns included).
    #[tokio::test]
    async fn active_license_reads_back_e4_quotas_and_modules() {
        let svc = service().await;
        sqlx::query(
            "INSERT INTO one_license_activation \
                 (license_id, enterprise_id, customer, tier, seats, expires_at, issued_at, activated_at, activated_by, \
                  tenant_cap, agent_node_cap, cpu_cores_cap, memory_mb_cap, modules, serial, app_id, file_name) \
             VALUES ('lic1', 'ent1', 'Acme', 'enterprise', 50, NULL, 0, 0, 'admin1', \
                     10, 20, 64, 131072, '[{\"module\":\"/admin/*\",\"startsAt\":null,\"expiresAt\":null}]', \
                     'SN-0001', 'one-work', 'acme.lic')",
        )
        .execute(&svc.pool)
        .await
        .unwrap();

        let info = svc.active_license("ent1").await.unwrap().unwrap();
        assert_eq!(info.tenant_cap, Some(10));
        assert_eq!(info.agent_node_cap, Some(20));
        assert_eq!(info.cpu_cores_cap, Some(64));
        assert_eq!(info.memory_mb_cap, Some(131_072));
        assert_eq!(info.modules.len(), 1);
        assert_eq!(info.modules[0].module, "/admin/*");
        assert_eq!(info.serial.as_deref(), Some("SN-0001"));
        assert_eq!(info.app_id.as_deref(), Some("one-work"));
        assert_eq!(info.file_name.as_deref(), Some("acme.lic"));
    }

    /// A row written before billing_006 has no `modules` value to read —
    /// covered by the column's own `NOT NULL DEFAULT '[]'`, but confirmed
    /// here in case a future edit weakens that default: this must still
    /// resolve to "no restriction", not an error swallowing the whole license.
    #[tokio::test]
    async fn active_license_tolerates_a_pre_e4_row_with_no_quota_columns() {
        let svc = service().await;
        sqlx::query(
            "INSERT INTO one_license_activation \
                 (license_id, enterprise_id, customer, tier, issued_at, activated_at, activated_by) \
             VALUES ('lic1', 'ent1', 'Acme', 'enterprise', 0, 0, 'admin1')",
        )
        .execute(&svc.pool)
        .await
        .unwrap();

        let info = svc.active_license("ent1").await.unwrap().unwrap();
        assert_eq!(info.tenant_cap, None);
        assert!(info.modules.is_empty());
    }

    // ---- P2-5: per-model-call LLM trace (`one_llm_calls`) ----

    /// Map one user onto a company, so `record_llm_call`'s tenancy resolution
    /// finds them (same fixture pattern the usage-events tests use).
    async fn seed_llm_member(svc: &BillingService, enterprise_id: &str, user_id: &str) {
        sqlx::query(
            "INSERT INTO one_enterprise_members (user_id, enterprise_id, role, joined_at, updated_at) \
             VALUES (?, ?, 'member', 0, 0)",
        )
        .bind(user_id)
        .bind(enterprise_id)
        .execute(&svc.pool)
        .await
        .unwrap();
    }

    /// A successful call with the common fields filled; tests override via
    /// struct update on the returned value where they care.
    fn llm_call(user_id: &str, model: &str) -> NewLlmCall {
        NewLlmCall {
            user_id: user_id.to_owned(),
            conversation_id: Some("c1".to_owned()),
            model: Some(model.to_owned()),
            provider: Some("acp".to_owned()),
            tool_name: None,
            input_tokens: 10,
            output_tokens: 20,
            duration_ms: Some(120),
            error: None,
        }
    }

    /// Record → list round trip: every column survives, newest first, and the
    /// user / model / since filters each narrow the page on their own.
    #[tokio::test]
    async fn llm_call_roundtrip_with_user_model_and_since_filters() {
        let svc = service().await;
        seed_llm_member(&svc, "ent1", "alice").await;
        svc.record_llm_call(llm_call("alice", "claude-opus-4-8")).await.unwrap();
        svc.record_llm_call(llm_call("alice", "claude-opus-4-8")).await.unwrap();
        svc.record_llm_call(llm_call("alice", "gpt-4")).await.unwrap();

        let all = svc.list_llm_calls("ent1", 0, None, None, 50, 0).await.unwrap();
        assert_eq!(all.total, 3);
        assert_eq!(all.calls.len(), 3);
        let first = &all.calls[0];
        assert_eq!(first.model.as_deref(), Some("gpt-4"), "newest first");
        assert_eq!(first.user_id, "alice");
        assert_eq!(first.conversation_id.as_deref(), Some("c1"));
        assert_eq!(first.provider.as_deref(), Some("acp"));
        assert_eq!((first.input_tokens, first.output_tokens), (10, 20));
        assert_eq!(first.duration_ms, Some(120));
        assert!(first.error.is_none());

        let by_model = svc.list_llm_calls("ent1", 0, None, Some("gpt-4"), 50, 0).await.unwrap();
        assert_eq!(by_model.total, 1);
        assert_eq!(by_model.calls[0].model.as_deref(), Some("gpt-4"));

        let by_user = svc.list_llm_calls("ent1", 0, Some("alice"), None, 50, 0).await.unwrap();
        assert_eq!(by_user.total, 3);

        // `since` excludes everything stamped before it: push one row into the
        // far past and confirm it drops out of a "recent" query. The cutoff
        // backs off a minute so the two just-written rows (stamped at record
        // time, a few ms before now) are safely inside the window.
        sqlx::query("UPDATE one_llm_calls SET created_at = 1 WHERE model = 'gpt-4'")
            .execute(&svc.pool)
            .await
            .unwrap();
        let recent = svc
            .list_llm_calls("ent1", dream_core_common::now_ms() - 60_000, None, None, 50, 0)
            .await
            .unwrap();
        assert_eq!(recent.total, 2, "the pre-since row must be excluded");
    }

    /// Same pagination contract as `list_usage_events`: `total` reflects the
    /// whole filtered set, pages don't overlap.
    #[tokio::test]
    async fn llm_calls_paginate_like_usage_events() {
        let svc = service().await;
        seed_llm_member(&svc, "ent1", "alice").await;
        for _ in 0..4 {
            svc.record_llm_call(llm_call("alice", "claude-opus-4-8")).await.unwrap();
        }

        let page1 = svc.list_llm_calls("ent1", 0, None, None, 2, 0).await.unwrap();
        let page2 = svc.list_llm_calls("ent1", 0, None, None, 2, 2).await.unwrap();
        assert_eq!(page1.total, 4);
        assert_eq!(page1.calls.len(), 2);
        assert_eq!(page2.calls.len(), 2);
        let page1_ids: std::collections::HashSet<_> = page1.calls.iter().map(|c| c.id.clone()).collect();
        let page2_ids: std::collections::HashSet<_> = page2.calls.iter().map(|c| c.id.clone()).collect();
        assert!(page1_ids.is_disjoint(&page2_ids), "pages must not overlap");
    }

    /// Retention purge is enterprise-scoped and removes only rows strictly
    /// before the cutoff, reporting how many it deleted.
    #[tokio::test]
    async fn purge_llm_calls_removes_only_rows_before_the_window() {
        let svc = service().await;
        seed_llm_member(&svc, "ent1", "alice").await;
        seed_llm_member(&svc, "ent2", "bob").await;
        svc.record_llm_call(llm_call("alice", "claude-opus-4-8")).await.unwrap();
        svc.record_llm_call(llm_call("alice", "claude-opus-4-8")).await.unwrap();
        svc.record_llm_call(llm_call("bob", "claude-opus-4-8")).await.unwrap();

        let now = dream_core_common::now_ms();
        // Age deterministically one ent1 row (its oldest) and ent2's only row,
        // so the purge has a stale row to find in each tenant.
        for ent in ["ent1", "ent2"] {
            sqlx::query(
                "UPDATE one_llm_calls SET created_at = 1 \
                 WHERE id = (SELECT id FROM one_llm_calls WHERE enterprise_id = ? ORDER BY created_at LIMIT 1)",
            )
            .bind(ent)
            .execute(&svc.pool)
            .await
            .unwrap();
        }

        // Default-window cutoff — exactly what the endpoint computes when
        // `beforeMs` is omitted.
        let cutoff = now - LLM_CALL_RETENTION_DAYS * 24 * 3600 * 1000;
        let deleted = svc.purge_llm_calls_older_than("ent1", cutoff).await.unwrap();
        assert_eq!(deleted, 1, "exactly ent1's one stale row");

        let stale_ent1: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM one_llm_calls WHERE enterprise_id = 'ent1' AND created_at = 1")
                .fetch_one(&svc.pool)
                .await
                .unwrap();
        assert_eq!(stale_ent1, 0);
        // The fresh ent1 row survives the purge.
        let fresh_ent1: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_llm_calls WHERE enterprise_id = 'ent1'")
            .fetch_one(&svc.pool)
            .await
            .unwrap();
        assert_eq!(fresh_ent1, 1);
        // ent2's stale row is a different tenant's history — untouched.
        let ent2_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_llm_calls WHERE enterprise_id = 'ent2'")
            .fetch_one(&svc.pool)
            .await
            .unwrap();
        assert_eq!(ent2_left, 1, "a purge of ent1 must never touch ent2");
    }

    /// Failed calls (error non-NULL) are first-class rows — a retry storm is
    /// exactly what this trace exists to expose, so they must list like
    /// successful ones, carrying the reason and their own duration.
    #[tokio::test]
    async fn failed_llm_calls_are_queryable_like_successful_ones() {
        let svc = service().await;
        seed_llm_member(&svc, "ent1", "alice").await;
        svc.record_llm_call(llm_call("alice", "claude-opus-4-8")).await.unwrap();
        svc.record_llm_call(NewLlmCall {
            error: Some("429 rate limited".to_owned()),
            duration_ms: Some(35),
            input_tokens: 0,
            output_tokens: 0,
            ..llm_call("alice", "claude-opus-4-8")
        })
        .await
        .unwrap();

        let page = svc.list_llm_calls("ent1", 0, None, None, 50, 0).await.unwrap();
        assert_eq!(page.total, 2, "the failed call is listed beside the good one");
        let failed = &page.calls[0];
        assert_eq!(failed.error.as_deref(), Some("429 rate limited"));
        assert_eq!(failed.duration_ms, Some(35));
        assert_eq!(failed.input_tokens, 0, "a failed call spent no output tokens");
        // And the success filter direction: the good row is still clean.
        assert!(page.calls[1].error.is_none());
    }

    /// Red line: a personal / no-company user records NOTHING. Unlike
    /// `one_usage_events` (which keeps NULL-enterprise rows for personal
    /// users), the per-call trace is enterprise-scoped NOT NULL by design —
    /// it is a governed-deployment diagnostic, and there is no admin to show
    /// it to on a personal install.
    #[tokio::test]
    async fn llm_calls_are_not_recorded_for_personal_users() {
        let svc = service().await;
        svc.record_llm_call(llm_call("nobody", "claude-opus-4-8"))
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_llm_calls")
            .fetch_one(&svc.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
