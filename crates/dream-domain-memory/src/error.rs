//! one-memory error type. Mirrors one-platform's code/status mapping so
//! clients handle memory errors the same way as other governance errors.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use dream_core_api_types::ErrorResponse;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Not currently in an enterprise")]
    NotInEnterprise,

    #[error("{0}")]
    Forbidden(String),

    /// C1-2 fix: this machine (self-reported `x-dream-machine-id`) has been
    /// blocked in the runtime-node roster. Distinct from `Forbidden` so the
    /// client can tell "this device was blocked" apart from other refusals.
    #[error("Machine blocked: {0}")]
    MachineBlocked(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("{0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl MemoryError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotInEnterprise => "NOT_IN_ENTERPRISE",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::MachineBlocked(_) => "MACHINE_BLOCKED",
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Forbidden(_) | Self::MachineBlocked(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

impl IntoResponse for MemoryError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "one-memory internal error");
        }
        (status, Json(ErrorResponse::new(self.to_string(), self.code()))).into_response()
    }
}

impl From<sqlx::Error> for MemoryError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(e.to_string())
    }
}
