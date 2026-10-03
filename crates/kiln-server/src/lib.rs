pub mod auth;
pub mod enrollment;
pub mod gateway;
pub mod host;
mod journal;
mod ssh;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use kiln_api::ApiError;

pub struct Failure(StatusCode, ApiError);

impl Failure {
    pub fn new(status: StatusCode, code: &str, message: &str) -> Self {
        Self(
            status,
            ApiError {
                code: code.into(),
                message: message.into(),
            },
        )
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        eprintln!("host service error: {error}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Host operation unavailable; consult the server log.",
        )
    }

    pub fn missing() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Resource not found in this workspace.",
        )
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}
