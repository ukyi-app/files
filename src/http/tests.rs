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

#[test]
fn build_state_creates_objects_dir_and_loads_keys() {
    let d = tempfile::tempdir().unwrap();
    let keys_path = d.path().join("keys.json");
    std::fs::write(&keys_path, r#"[{"id":"k","sha256":"00","service":"s"}]"#).unwrap();
    let data_dir = d.path().join("data");
    let cfg = Config::from_env(|k| match k {
        "FILES_DATA_DIR" => Some(data_dir.to_string_lossy().to_string()),
        "FILES_KEYS_PATH" => Some(keys_path.to_string_lossy().to_string()),
        _ => None,
    })
    .unwrap();
    let _state = build_state(cfg).unwrap();
    assert!(data_dir.join(".objects").is_dir());
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
