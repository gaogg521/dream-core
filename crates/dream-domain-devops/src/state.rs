//! Router state for one-devops routes.

use std::sync::Arc;

use dream_domain_employee::{DEFAULT_TENANT, EmployeeService, TenantResolver};

use crate::proxy_usage::ProxyUsageRecorder;
use crate::service::DevopsService;

#[derive(Clone)]
pub struct OneDevopsRouterState {
    pub service: Arc<DevopsService>,
    /// Optional so unit tests can build state without the employee runtime.
    /// The dispatch endpoint returns a clear error when it is absent.
    pub employee: Option<Arc<EmployeeService>>,
    /// Optional tenant resolver (A1 L3): lets dispatch/breakdown reach a
    /// team-shared employee owned by another same-tenant member. Absent →
    /// callers resolve to the `default` tenant (personal edition).
    pub tenant_resolver: Option<Arc<dyn TenantResolver>>,
    /// Optional usage sink for the model proxy (P1-2): each proxied response
    /// is teed through a usage parser and completed calls are handed here.
    /// `None` in personal builds — the tap stays a pure pass-through, matching
    /// the rest of the accounting plane.
    pub usage_recorder: Option<Arc<dyn ProxyUsageRecorder>>,
}

impl OneDevopsRouterState {
    pub fn new(service: Arc<DevopsService>) -> Self {
        Self {
            service,
            employee: None,
            tenant_resolver: None,
            usage_recorder: None,
        }
    }

    /// Wire the employee runtime so requirements can be dispatched to digital
    /// employees. Called by the app router after the EmployeeService is built.
    pub fn with_employee(mut self, employee: Arc<EmployeeService>) -> Self {
        self.employee = Some(employee);
        self
    }

    /// Wire the tenant resolver so dispatch/breakdown can use team-shared
    /// employees.
    pub fn with_tenant_resolver(mut self, resolver: Arc<dyn TenantResolver>) -> Self {
        self.tenant_resolver = Some(resolver);
        self
    }

    /// Wire the billing-plane usage sink so model-proxy calls are metered
    /// (P1-2). Called by the app router under the `enterprise` feature only.
    pub fn with_proxy_usage_recorder(mut self, recorder: Arc<dyn ProxyUsageRecorder>) -> Self {
        self.usage_recorder = Some(recorder);
        self
    }

    /// Resolve the caller's tenant, falling back to the personal `default`
    /// tenant when no resolver is wired.
    pub async fn tenant_of(&self, user_id: &str) -> String {
        match &self.tenant_resolver {
            Some(resolver) => resolver.tenant_of(user_id).await,
            None => DEFAULT_TENANT.to_owned(),
        }
    }
}
