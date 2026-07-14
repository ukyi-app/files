#![allow(dead_code)] // 각 테스트 바이너리가 common을 독립 컴파일 → 일부만 사용

use axum::body::Body;
use axum::http::{header, Request};
use files::config::Config;
use files::http::{self, AppState};
use files::layout::Layout;
use files::store::Store;
use sha2::{Digest, Sha256};

pub fn hex_sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// `.objects`를 만들고 `(tempdir, Store, Layout)`을 준다 — **F-14 통합 증인의 공용 무대**
/// (`e2e`의 W4·W7·W9·W10c·W17 · `reconcile_vanishing_entries`의 W13-E/G/T).
///
/// ⚠ tempdir을 **돌려준다** — 호출부가 `let (_d, ..)`로 붙잡고 있어야 한다(드롭되면 무대가 사라진다).
pub fn f14_store() -> (tempfile::TempDir, Store, Layout) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let l = Layout::new(root.clone());
    std::fs::create_dir_all(l.objects_dir()).unwrap();
    (d, Store::new(root), l)
}

pub async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let b = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
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
