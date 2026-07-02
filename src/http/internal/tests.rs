use super::*;
use crate::auth::{ApiKey, KeyRegistry};
use crate::capacity::Capacity;
use crate::config::Config;
use crate::store::Store;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tower::ServiceExt;

fn sha(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn state_with_data_dir(data_dir: &str) -> AppState {
    let writer = ApiKey {
        id: "w".into(),
        sha256: sha("writer"),
        service: "page".into(),
        write_buckets: vec!["skills".into()],
        read_buckets: vec!["skills".into()],
        admin: false,
    };
    let reader = ApiKey {
        id: "r".into(),
        sha256: sha("reader"),
        service: "ro".into(),
        write_buckets: vec![],
        read_buckets: vec!["skills".into()],
        admin: false,
    };
    let admin = ApiKey {
        id: "a".into(),
        sha256: sha("admin"),
        service: "ops".into(),
        write_buckets: vec![],
        read_buckets: vec![],
        admin: true,
    };
    let dd = data_dir.to_string();
    let cfg = Config::from_env(move |k| match k {
        "FILES_DATA_DIR" => Some(dd.clone()),
        "FILES_KEYS_PATH" => Some("/tmp/keys.json".into()),
        "FILES_MIN_FREE_BYTES" => Some("0".into()),
        _ => None,
    })
    .unwrap();
    AppState {
        store: Store::new(std::path::PathBuf::from(data_dir)),
        keys: Arc::new(KeyRegistry::from_keys(vec![writer, reader, admin])),
        cap: Capacity::with_free_fn(0, || Ok(u64::MAX)),
        cfg: Arc::new(cfg),
    }
}

fn test_app() -> (Router, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    let state = state_with_data_dir(&d.path().to_string_lossy());
    (router(state), d)
}

fn req(method: &str, uri: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn req_json(method: &str, uri: &str, token: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .unwrap()
}

fn req_get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn req_plain(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn create_bucket_admin_then_list() {
    let (app, _d) = test_app();
    let res = app
        .clone()
        .oneshot(req_json("PUT", "/api/buckets/photos", "admin", r#"{"visibility":"public"}"#))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let res = app.oneshot(req_get("/api/buckets", "admin")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let s = String::from_utf8(body_bytes(res).await).unwrap();
    assert!(s.contains("\"photos\""), "buckets list: {s}");
    assert!(s.contains("\"public\""));
}

#[tokio::test]
async fn create_bucket_non_admin_403() {
    let (app, _d) = test_app();
    let res = app
        .oneshot(req_json("PUT", "/api/buckets/photos", "writer", r#"{"visibility":"public"}"#))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_reserved_bucket_400() {
    let (app, _d) = test_app();
    let res = app
        .oneshot(req_json("PUT", "/api/buckets/api", "admin", r#"{"visibility":"public"}"#))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_files_returns_entries() {
    let (app, _d) = test_app();
    app.clone()
        .oneshot(req("PUT", "/api/files/skills/object?key=one.txt", "writer", "a"))
        .await
        .unwrap();
    app.clone()
        .oneshot(req("PUT", "/api/files/skills/object?key=two.txt", "writer", "bb"))
        .await
        .unwrap();
    let res = app.oneshot(req_get("/api/files/skills", "reader")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let s = String::from_utf8(body_bytes(res).await).unwrap();
    assert!(s.contains("one.txt") && s.contains("two.txt"), "list: {s}");
    assert!(s.contains("contentType")); // camelCase flatten
}

#[tokio::test]
async fn healthz_ok() {
    let (app, _d) = test_app();
    let res = app.oneshot(req_plain("/healthz")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn readyz_ok_when_writable() {
    let (app, _d) = test_app();
    let res = app.oneshot(req_plain("/readyz")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn readyz_503_when_unwritable() {
    let app = router(state_with_data_dir("/nonexistent-files-data-xyz/sub"));
    let res = app.oneshot(req_plain("/readyz")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn put_creates_201_then_get_roundtrip() {
    let (app, _d) = test_app();
    let res = app
        .clone()
        .oneshot(req("PUT", "/api/files/skills/object?key=a/b.txt", "writer", "hello"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let res = app
        .oneshot(req("GET", "/api/files/skills/object?key=a/b.txt", "reader", ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_bytes(res).await, b"hello");
}

#[tokio::test]
async fn get_missing_404() {
    let (app, _d) = test_app();
    let res = app
        .oneshot(req("GET", "/api/files/skills/object?key=missing.txt", "reader", ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_without_write_scope_403() {
    let (app, _d) = test_app();
    let res = app
        .oneshot(req("PUT", "/api/files/skills/object?key=x.txt", "reader", "hi"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_then_get_404() {
    let (app, _d) = test_app();
    app.clone()
        .oneshot(req("PUT", "/api/files/skills/object?key=d.txt", "writer", "bye"))
        .await
        .unwrap();
    let res = app
        .clone()
        .oneshot(req("DELETE", "/api/files/skills/object?key=d.txt", "writer", ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = app
        .oneshot(req("GET", "/api/files/skills/object?key=d.txt", "reader", ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_returns_metadata_headers() {
    let (app, _d) = test_app();
    app.clone()
        .oneshot(req("PUT", "/api/files/skills/object?key=h.txt", "writer", "12345"))
        .await
        .unwrap();
    let res = app
        .oneshot(req("HEAD", "/api/files/skills/object?key=h.txt", "reader", ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_LENGTH).unwrap().to_str().unwrap(),
        "5"
    );
    assert!(res.headers().get(header::ETAG).is_some());
    assert!(body_bytes(res).await.is_empty());
}
