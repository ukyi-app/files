use crate::error::AppError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if let AppError::Internal(ref e) = self {
            tracing::error!(error = %e, "internal error");
        }
        (status, Json(serde_json::json!({ "error": self.code() }))).into_response()
    }
}
