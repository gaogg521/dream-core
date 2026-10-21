//! Router state for one-employee routes.

use std::sync::Arc;

use crate::service::EmployeeService;
use crate::tenant::{DEFAULT_TENANT, TenantResolver};

#[async_trait::async_trait]
pub trait EmployeeAssistantDependencies: Send + Sync {
    async fn export_batch(
        &self,
        references: &[(String, String, Option<String>)],
    ) -> Result<Vec<dream_core_api_types::employee_sync::TeamAssistantSnapshot>, String> {
        let mut snapshots = Vec::with_capacity(references.len());
        for (owner, id, agent_override) in references {
            snapshots.push(self.export(owner, id, agent_override.as_deref()).await?);
        }
        Ok(snapshots)
    }
    async fn export(
        &self,
        owner: &str,
        id: &str,
        agent_override: Option<&str>,
    ) -> Result<dream_core_api_types::employee_sync::TeamAssistantSnapshot, String>;
    async fn materialize(
        &self,
        user: &str,
        scope: &str,
        snapshots: &[dream_core_api_types::employee_sync::TeamAssistantSnapshot],
    ) -> Result<std::collections::HashMap<String, String>, String>;
    async fn prune(&self, user: &str, scope: &str, retained: &[String]) -> Result<(), String>;
}

#[derive(Clone)]
pub struct OneEmployeeRouterState {
    pub service: Arc<EmployeeService>,
    /// Optional so personal-edition deployments and unit tests can build state
    /// without one-org. Absent → every user is in the `default` tenant.
    pub tenant_resolver: Option<Arc<dyn TenantResolver>>,
    pub assistant_dependencies: Option<Arc<dyn EmployeeAssistantDependencies>>,
}

impl OneEmployeeRouterState {
    pub fn new(service: Arc<EmployeeService>) -> Self {
        Self {
            service,
            tenant_resolver: None,
            assistant_dependencies: None,
        }
    }

    pub fn with_assistant_dependencies(mut self, dependencies: Arc<dyn EmployeeAssistantDependencies>) -> Self {
        self.assistant_dependencies = Some(dependencies);
        self
    }

    /// Wire the tenant resolver so employees can be shared within a tenant.
    pub fn with_tenant_resolver(mut self, resolver: Arc<dyn TenantResolver>) -> Self {
        self.tenant_resolver = Some(resolver);
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
