use super::ranged::build_ranged;
use super::{AppState, AuthKey};
use crate::capacity::free_bytes;
use crate::error::AppError;
use crate::meta::{BucketMeta, ObjectMeta, Visibility};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use std::time::Duration;

/// 내부 API 라우터(파일 CRUD + 버킷 + 헬스). 헬스 외 라우트는 인증 필요.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/files/{bucket}/{*key}",
            put(put_file).get(get_file).head(head_file).delete(delete_file),
        )
        .route("/api/files/{bucket}", get(list_files))
        .route("/api/buckets/{bucket}", put(put_bucket))
        .route("/api/buckets", get(get_buckets))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
}

async fn put_file(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path((bucket, obj_key)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, AppError> {
    if !key.can_write(&bucket) {
        return Err(AppError::Forbidden);
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let max = st.cfg.max_file_bytes;
    if content_length.is_some_and(|cl| cl > max) {
        return Err(AppError::TooLarge);
    }
    // 예약: Content-Length(없으면 max)를 max로 캡. RAII로 완료/실패 시 해제.
    let _res = st.cap.reserve(content_length.unwrap_or(max).min(max))?;

    let stream = body.into_data_stream();
    let fut = st
        .store
        .put_stream(&bucket, &obj_key, &content_type, &key.service, stream, max);
    // 업로드 바디 타임아웃(< gc_grace). 초과 시 중단 — 잔여 temp는 reconciliation이 grace 후 정리(P3-3)
    let meta = match tokio::time::timeout(Duration::from_secs(st.cfg.upload_timeout_secs), fut).await
    {
        Ok(r) => r?,
        Err(_) => return Err(AppError::BadRequest("upload_timeout")),
    };
    Ok((StatusCode::CREATED, Json(meta)).into_response())
}

async fn get_file(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path((bucket, obj_key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if !key.can_read(&bucket) {
        return Err(AppError::Forbidden);
    }
    let (meta, file) = st.store.open(&bucket, &obj_key).await?;
    Ok(build_ranged(&headers, &meta, file).await)
}

async fn head_file(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path((bucket, obj_key)): Path<(String, String)>,
) -> Result<Response, AppError> {
    if !key.can_read(&bucket) {
        return Err(AppError::Forbidden);
    }
    let meta = st.store.head(&bucket, &obj_key).await?;
    let mut resp = Response::new(Body::empty());
    let h = resp.headers_mut();
    h.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", meta.sha256)).unwrap(),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.size));
    if let Ok(v) = HeaderValue::from_str(&meta.content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    Ok(resp)
}

async fn delete_file(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path((bucket, obj_key)): Path<(String, String)>,
) -> Result<Response, AppError> {
    if !key.can_write(&bucket) {
        return Err(AppError::Forbidden);
    }
    st.store.delete(&bucket, &obj_key).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(serde::Deserialize)]
struct CreateBucket {
    visibility: Visibility,
}

async fn put_bucket(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path(bucket): Path<String>,
    Json(body): Json<CreateBucket>,
) -> Result<Response, AppError> {
    if !key.admin {
        return Err(AppError::Forbidden);
    }
    let bm = BucketMeta {
        visibility: body.visibility,
        owner: key.service.clone(),
        created_at: crate::clock::now_rfc3339(),
    };
    st.store.put_bucket(&bucket, &bm).await?;
    Ok((StatusCode::CREATED, Json(bm)).into_response())
}

#[derive(serde::Serialize)]
struct BucketEntry {
    bucket: String,
    #[serde(flatten)]
    meta: BucketMeta,
}

async fn get_buckets(State(st): State<AppState>, AuthKey(key): AuthKey) -> Result<Response, AppError> {
    if !key.admin {
        return Err(AppError::Forbidden);
    }
    let entries: Vec<BucketEntry> = st
        .store
        .list_buckets()
        .await?
        .into_iter()
        .map(|(bucket, meta)| BucketEntry { bucket, meta })
        .collect();
    Ok(Json(entries).into_response())
}

#[derive(serde::Serialize)]
struct ObjectEntry {
    key: String,
    #[serde(flatten)]
    meta: ObjectMeta,
}

async fn list_files(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path(bucket): Path<String>,
) -> Result<Response, AppError> {
    if !key.can_read(&bucket) {
        return Err(AppError::Forbidden);
    }
    let entries: Vec<ObjectEntry> = st
        .store
        .list(&bucket)
        .await?
        .into_iter()
        .map(|(key, meta)| ObjectEntry { key, meta })
        .collect();
    Ok(Json(entries).into_response())
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// `/data` 쓰기 가능 + free-space ≥ min_free 확인.
async fn readyz(State(st): State<AppState>) -> StatusCode {
    let dir = &st.cfg.data_dir;
    let probe = dir.join(".readyz-probe");
    let writable = tokio::fs::write(&probe, b"ok").await.is_ok();
    let _ = tokio::fs::remove_file(&probe).await;
    let free_ok = free_bytes(dir)
        .map(|f| f >= st.cfg.min_free_bytes)
        .unwrap_or(false);
    if writable && free_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
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
            .oneshot(req("PUT", "/api/files/skills/one.txt", "writer", "a"))
            .await
            .unwrap();
        app.clone()
            .oneshot(req("PUT", "/api/files/skills/two.txt", "writer", "bb"))
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
            .oneshot(req("PUT", "/api/files/skills/a/b.txt", "writer", "hello"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);

        let res = app
            .oneshot(req("GET", "/api/files/skills/a/b.txt", "reader", ""))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_bytes(res).await, b"hello");
    }

    #[tokio::test]
    async fn get_missing_404() {
        let (app, _d) = test_app();
        let res = app
            .oneshot(req("GET", "/api/files/skills/missing.txt", "reader", ""))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn put_without_write_scope_403() {
        let (app, _d) = test_app();
        let res = app
            .oneshot(req("PUT", "/api/files/skills/x.txt", "reader", "hi"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_then_get_404() {
        let (app, _d) = test_app();
        app.clone()
            .oneshot(req("PUT", "/api/files/skills/d.txt", "writer", "bye"))
            .await
            .unwrap();
        let res = app
            .clone()
            .oneshot(req("DELETE", "/api/files/skills/d.txt", "writer", ""))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let res = app
            .oneshot(req("GET", "/api/files/skills/d.txt", "reader", ""))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn head_returns_metadata_headers() {
        let (app, _d) = test_app();
        app.clone()
            .oneshot(req("PUT", "/api/files/skills/h.txt", "writer", "12345"))
            .await
            .unwrap();
        let res = app
            .oneshot(req("HEAD", "/api/files/skills/h.txt", "reader", ""))
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
}
