//! one-devops error type; wire shape matches upstream `ErrorResponse`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use dream_core_api_types::ErrorResponse;

#[derive(Debug, thiserror::Error)]
pub enum DevopsError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// This machine (identified by the client-reported `x-dream-machine-id`
    /// header) has been blocked by an administrator in the runtime-node
    /// roster. Distinct from `Forbidden` so the client can tell "this device
    /// was blocked" apart from other refusals and react accordingly (purge
    /// local team resources) rather than treating it as a generic error.
    #[error("Machine blocked: {0}")]
    MachineBlocked(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl DevopsError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "NOT_FOUND",
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::MachineBlocked(_) => "MACHINE_BLOCKED",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::MachineBlocked(_) => StatusCode::FORBIDDEN,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for DevopsError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "one-devops internal error");
        }
        (status, Json(ErrorResponse::new(self.to_string(), self.code()))).into_response()
    }
}

impl From<sqlx::Error> for DevopsError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(format!("database error: {e}"))
    }
}
