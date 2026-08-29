//! one-workflow error type. Mirrors one-platform's code/status mapping so
//! clients handle workflow errors the same way as other governance errors.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use dream_core_api_types::ErrorResponse;

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("Not currently in an enterprise")]
    NotInEnterprise,

    #[error("{0}")]
    Forbidden(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("{0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl WorkflowError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotInEnterprise => "NOT_IN_ENTERPRISE",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

impl IntoResponse for WorkflowError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "one-workflow internal error");
        }
        (status, Json(ErrorResponse::new(self.to_string(), self.code()))).into_response()
    }
}

impl From<sqlx::Error> for WorkflowError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(e.to_string())
    }
}
