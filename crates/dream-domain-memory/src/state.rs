//! Router state for one-memory routes.

use std::sync::Arc;

use crate::service::MemoryService;

#[derive(Clone)]
pub struct OneMemoryRouterState {
    pub service: Arc<MemoryService>,
}

impl OneMemoryRouterState {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self { service }
    }
}
