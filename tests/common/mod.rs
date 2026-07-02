#![allow(dead_code)] // 각 테스트 바이너리가 common을 독립 컴파일 → 일부만 사용

use axum::body::Body;
use axum::http::{header, Request};
use files::config::Config;
use files::http::{self, AppState};
use sha2::{Digest, Sha256};

pub fn hex_sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let b = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&b).unwrap()
}

/// keys.json 문자열을 받아 tempdir 기반 내부 라우터를 만든다(contract/openapi 공용 패턴).
pub fn internal_app(keys_json: &str) -> (axum::Router, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    let keys_path = d.path().join("keys.json");
    std::fs::write(&keys_path, keys_json).unwrap();
    let dd = d.path().join("data");
    let cfg = Config::from_env(|k| match k {
        "FILES_DATA_DIR" => Some(dd.to_string_lossy().to_string()),
        "FILES_KEYS_PATH" => Some(keys_path.to_string_lossy().to_string()),
        _ => None,
    })
    .unwrap();
    let state: AppState = http::build_state(cfg).unwrap();
    (http::internal::router(state), d)
}

pub fn bearer(method: &str, uri: &str, token: &str, ct: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from(body.to_string()))
        .unwrap()
}
