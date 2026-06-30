use super::ranged::build_ranged;
use super::{AppState, AuthKey};
use crate::error::AppError;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use axum::{Json, Router};
use std::time::Duration;

/// 내부 API 라우터(인증 필요). 파일 CRUD.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/files/{bucket}/{*key}",
            put(put_file).get(get_file).head(head_file).delete(delete_file),
        )
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

    fn test_app() -> (Router, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
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
        let cfg = Config::from_env(|k| match k {
            "FILES_DATA_DIR" => Some("/tmp".into()),
            "FILES_KEYS_PATH" => Some("/tmp/keys.json".into()),
            _ => None,
        })
        .unwrap();
        let state = AppState {
            store: Store::new(d.path().to_path_buf()),
            keys: Arc::new(KeyRegistry::from_keys(vec![writer, reader])),
            cap: Capacity::with_free_fn(0, || Ok(u64::MAX)),
            cfg: Arc::new(cfg),
        };
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

    async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
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
