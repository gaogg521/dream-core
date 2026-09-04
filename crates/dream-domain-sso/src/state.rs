//! Router state for one-sso routes.

use std::sync::Arc;

use crate::enterprise::{CompanyAdminCheck, DirectorySink, EnterpriseSync};
use crate::org_hooks::OrgAutoJoin;
use crate::service::SsoService;

#[derive(Clone)]
pub struct OneSsoRouterState {
    pub service: Arc<SsoService>,
    /// Syncs the SSO user's company + membership into the enterprise-org domain
    /// (one-enterprise), when the app layer wires one in. `None` — personal
    /// edition, WebUI-only builds, unit tests — means SSO login authenticates
    /// and nothing else, exactly as before the enterprise dimension existed.
    pub enterprise_sync: Option<Arc<dyn EnterpriseSync>>,
    /// Lets `RequireSsoAdmin` accept a company administrator (Direction B: SSO
    /// config is a company-level policy). `None` falls back to the project-group
    /// `one_user_org` admin check — personal / standalone behaviour is unchanged.
    pub company_admin_check: Option<Arc<dyn CompanyAdminCheck>>,
    /// Auto-joins a project group by email domain policy (P2-4 onboarding).
    /// `None` — personal edition, unit tests — means SSO login never auto-joins
    /// a project group; membership stays invite-code-only, exactly as before.
    pub org_auto_join: Option<Arc<dyn OrgAutoJoin>>,
    /// Where a directory pull is stored (T6). `None` — personal edition, tests,
    /// or any build without the enterprise dimension — means directory sync has
    /// nowhere to write and therefore never runs.
    pub directory_sink: Option<Arc<dyn DirectorySink>>,
    /// 登录二次认证（MFA · TOTP）服务。None —— 单机/测试组装未接 —— SSO
    /// 回调与 LDAP 登录不做第二步，管理端点返回 503。
    pub mfa: Option<Arc<dream_core_auth::mfa::MfaService>>,
}

impl OneSsoRouterState {
    pub fn new(service: Arc<SsoService>) -> Self {
        Self {
            service,
            enterprise_sync: None,
            company_admin_check: None,
            org_auto_join: None,
            directory_sink: None,
            mfa: None,
        }
    }

    /// 挂上 MFA 服务（登录闸 + 管理端点）。见 `dream_core_auth::mfa`。
    pub fn with_mfa(mut self, mfa: Arc<dream_core_auth::mfa::MfaService>) -> Self {
        self.mfa = Some(mfa);
        self
    }

    pub fn with_enterprise_sync(mut self, sync: Arc<dyn EnterpriseSync>) -> Self {
        self.enterprise_sync = Some(sync);
        self
    }

    pub fn with_company_admin_check(mut self, check: Arc<dyn CompanyAdminCheck>) -> Self {
        self.company_admin_check = Some(check);
        self
    }

    pub fn with_org_auto_join(mut self, hook: Arc<dyn OrgAutoJoin>) -> Self {
        self.org_auto_join = Some(hook);
        self
    }

    pub fn with_directory_sink(mut self, sink: Arc<dyn DirectorySink>) -> Self {
        self.directory_sink = Some(sink);
        self
    }
}
