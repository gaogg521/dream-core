//! Router state for one-workflow routes.

use std::sync::Arc;

use crate::service::WorkflowService;

#[derive(Clone)]
pub struct OneWorkflowRouterState {
    pub service: Arc<WorkflowService>,
}

impl OneWorkflowRouterState {
    pub fn new(service: Arc<WorkflowService>) -> Self {
        Self { service }
    }
}
