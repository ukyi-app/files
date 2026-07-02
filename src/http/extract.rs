use super::AppState;
use crate::auth::ApiKey;
use crate::error::AppError;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

/// `Authorization: Bearer <token>` 추출 + 인증. 실패 시 401.
pub struct AuthKey(pub ApiKey);

impl FromRequestParts<AppState> for AuthKey {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;
        let key = state
            .keys
            .authenticate(token)
            .ok_or(AppError::Unauthorized)?;
        Ok(AuthKey(key.clone()))
    }
}
