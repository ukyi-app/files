use crate::auth::{ApiKey, KeyRegistry};
use crate::capacity::Capacity;
use crate::config::Config;
use crate::error::AppError;
use crate::store::Store;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub keys: Arc<KeyRegistry>,
    pub cap: Capacity,
    pub cfg: Arc<Config>,
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{ApiKey, KeyRegistry};
    use crate::capacity::Capacity;
    use crate::config::Config;
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let key = ApiKey {
            id: "k".into(),
            sha256: hex::encode(Sha256::digest(b"good-token")),
            service: "svc".into(),
            write_buckets: vec![],
            read_buckets: vec![],
            admin: false,
        };
        let cfg = Config::from_env(|k| match k {
            "FILES_DATA_DIR" => Some("/tmp".into()),
            "FILES_KEYS_PATH" => Some("/tmp/keys.json".into()),
            _ => None,
        })
        .unwrap();
        AppState {
            store: Store::new(std::env::temp_dir()),
            keys: Arc::new(KeyRegistry::from_keys(vec![key])),
            cap: Capacity::with_free_fn(0, || Ok(u64::MAX)),
            cfg: Arc::new(cfg),
        }
    }

    async fn protected(_auth: AuthKey) -> &'static str {
        "ok"
    }

    fn app() -> Router {
        Router::new()
            .route("/p", get(protected))
            .with_state(test_state())
    }

    async fn status_of(req: Request<Body>) -> StatusCode {
        app().oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn missing_bearer_is_401() {
        let req = Request::builder().uri("/p").body(Body::empty()).unwrap();
        assert_eq!(status_of(req).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bad_bearer_is_401() {
        let req = Request::builder()
            .uri("/p")
            .header("authorization", "Bearer nope")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn good_bearer_is_200() {
        let req = Request::builder()
            .uri("/p")
            .header("authorization", "Bearer good-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::OK);
    }
}
