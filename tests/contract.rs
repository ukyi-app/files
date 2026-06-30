//! OpenAPI 계약 테스트 — 테스트 서버 응답 형태가 openapi.yaml 스키마와 일치하는지 검증.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use files::config::Config;
use files::http::{self, AppState};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

fn sha(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn schema_required(doc: &serde_json::Value, name: &str) -> Vec<String> {
    let mut v: Vec<String> = doc["components"]["schemas"][name]["required"]
        .as_array()
        .unwrap_or_else(|| panic!("schema {name} required missing"))
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

fn json_keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let b = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&b).unwrap()
}

fn app() -> (axum::Router, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    let keys_path = d.path().join("keys.json");
    let keys = format!(
        r#"[{{"id":"w","sha256":"{}","service":"page","writeBuckets":["skills"],"readBuckets":["skills"]}},{{"id":"a","sha256":"{}","service":"ops","admin":true}}]"#,
        sha("writer"),
        sha("admin")
    );
    std::fs::write(&keys_path, keys).unwrap();
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

fn bearer(method: &str, uri: &str, token: &str, ct: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn responses_match_openapi_schema() {
    let doc: serde_json::Value =
        serde_yaml::from_str(&std::fs::read_to_string("openapi.yaml").unwrap()).unwrap();
    let (app, _d) = app();

    // PUT object → ObjectMeta
    let res = app
        .clone()
        .oneshot(bearer("PUT", "/api/files/skills/c.txt", "writer", "text/plain", "data"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(json_keys(&body_json(res).await), schema_required(&doc, "ObjectMeta"));

    // GET missing → Error
    let res = app
        .clone()
        .oneshot(bearer("GET", "/api/files/skills/missing", "writer", "text/plain", ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_keys(&body_json(res).await), schema_required(&doc, "Error"));

    // PUT bucket(admin) → BucketMeta
    let res = app
        .clone()
        .oneshot(bearer(
            "PUT",
            "/api/buckets/skills",
            "admin",
            "application/json",
            r#"{"visibility":"public"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(json_keys(&body_json(res).await), schema_required(&doc, "BucketMeta"));

    // GET list → array of ObjectEntry
    let res = app
        .oneshot(bearer("GET", "/api/files/skills", "writer", "text/plain", ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let j = body_json(res).await;
    let arr = j.as_array().unwrap();
    assert!(!arr.is_empty());
    assert_eq!(json_keys(&arr[0]), schema_required(&doc, "ObjectEntry"));
}
