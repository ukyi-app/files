use super::openapi::ErrorResponse;
use super::ranged::build_ranged;
use super::{AppState, AuthKey};
use crate::capacity::free_bytes;
use crate::error::AppError;
use crate::meta::{BucketMeta, ObjectMeta, Visibility};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use std::time::Duration;
use utoipa::OpenApi;

/// raw 바이너리 업로드 바디 스키마 — OpenAPI `{type: string, format: binary}`(application/octet-stream).
/// blob 스토어라 업로드는 텍스트가 아닌 바이너리(생성 클라이언트가 UTF-8로 손상시키지 않게).
#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
pub(crate) struct OctetStreamBody(#[allow(dead_code)] Vec<u8>);

/// 객체 키 쿼리 파라미터(`?key=<key>`). 슬래시 포함 중첩 키(dir/sub/file.bin)를 허용한다 —
/// OpenAPI 단일 path 세그먼트로 표현 불가한 catch-all을 쿼리로 정합화(생성 클라이언트 호환).
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct KeyQuery {
    /// 객체 키. `/`로 구분된 세그먼트이며 각 세그먼트는 `[A-Za-z0-9_-]`로 시작하고 이후
    /// `[A-Za-z0-9._-]`만 허용(`.`으로 시작하는 숨김·`.`/`..`·제어문자 불가). 쿼리 값은 `%2F`로
    /// 인코딩하거나 슬래시를 그대로 둬도 된다. 머신-리더블 `pattern`은 세그먼트 문법만 모델링하는
    /// **필요조건**이며, 예약 접미사(`.meta.json`·`.bucket.json`) 배제를 포함한 최종 검증은 서버
    /// `valid_key`(src/path.rs)가 담당한다 — 위반 시 400(invalid_key/reserved_suffix). 접미사 배제를
    /// pattern에 넣지 않는 것은 lookahead를 Go·Rust regex 등 일부 생성기가 미지원해 검증기 컴파일이
    /// 깨질 수 있기 때문(서버를 진실원으로 둔다).
    #[param(
        min_length = 1,
        max_length = 1024,
        pattern = r"^[A-Za-z0-9_-][A-Za-z0-9._-]*(/[A-Za-z0-9_-][A-Za-z0-9._-]*)*$",
        example = "dir/sub/file.tar.gz"
    )]
    pub key: String,
}

/// 내부 API 라우터(파일 CRUD + 버킷 + 헬스 + OpenAPI 문서). 헬스/문서 외 라우트는 인증 필요.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route(
            "/api/files/{bucket}/object",
            put(put_file).get(get_file).head(head_file).delete(delete_file),
        )
        .route("/api/files/{bucket}", get(list_files))
        .route("/api/buckets/{bucket}", put(put_bucket))
        .route("/api/buckets", get(get_buckets))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state);
    // 코드-우선 OpenAPI: /openapi.json(스펙)만 서빙(무상태 — 스펙은 비밀 아님).
    // 인터랙티브 UI(Scalar/Swagger/Redoc 등)는 의도적으로 미서빙 — 전부 API origin에 CDN unpinned
    // 서드파티 JS를 로드해, try-it으로 Bearer 키 입력 시 공급망 침해가 토큰 탈취 경로가 된다(codex HIGH).
    // 스펙 렌더는 소비자 로컬 도구(VS Code OpenAPI·redocly·Scalar 데스크톱)로 /openapi.json을 열면 된다.
    let docs = Router::new().route(
        "/openapi.json",
        get(|| async { Json(super::openapi::ApiDoc::openapi()) }),
    );
    api.merge(docs)
}

#[utoipa::path(
    put, path = "/api/files/{bucket}/object", tag = "files",
    params(
        ("bucket" = String, Path, description = "버킷명"),
        KeyQuery,
    ),
    request_body(content = inline(OctetStreamBody), description = "raw 바이너리 바디 스트리밍(멀티파트 아님). Content-Type 헤더로 지정한 미디어 타입이 그대로 저장되어 다운로드 시 반환된다(다운로드 응답과 대칭).", content_type = "*/*"),
    responses(
        (status = 201, description = "생성됨", body = ObjectMeta),
        (status = 400, description = "잘못된 키/요청(invalid_key·reserved_suffix·upload_timeout)", body = ErrorResponse),
        (status = 401, description = "인증 실패", body = ErrorResponse),
        (status = 403, description = "쓰기 스코프 없음", body = ErrorResponse),
        (status = 413, description = "크기 초과", body = ErrorResponse),
        (status = 507, description = "저장공간 부족", body = ErrorResponse),
    ),
    security(("bearer" = [])),
)]
pub(crate) async fn put_file(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path(bucket): Path<String>,
    Query(KeyQuery { key: obj_key }): Query<KeyQuery>,
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

#[utoipa::path(
    get, path = "/api/files/{bucket}/object", tag = "files",
    params(
        ("bucket" = String, Path),
        KeyQuery,
        ("Range" = Option<String>, Header, description = "바이트 Range(예: bytes=0-1023)"),
        ("If-None-Match" = Option<String>, Header, description = "강한 ETag 조건부"),
    ),
    responses(
        // 바디는 항상 바이너리 바이트지만 응답 Content-Type은 저장된 meta.content_type(동적)이라
        // 미디어 타입을 `*/*`로 문서화(octet-stream 고정 아님) — 생성 클라이언트가 실제 타입으로 디스패치 가능.
        (status = 200, description = "본문(바이너리 스트리밍, Content-Type=저장 타입)", body = inline(OctetStreamBody), content_type = "*/*",
            headers(
                ("ETag" = String, description = "강한 ETag(\"<sha256>\")"),
                ("Accept-Ranges" = String, description = "bytes"),
                ("Content-Length" = String, description = "본문 바이트 수"),
                ("Content-Type" = String, description = "저장된 콘텐츠 타입(업로드 시 지정값)"),
                ("Last-Modified" = String, description = "업로드 시각"),
                ("Cache-Control" = String, description = "no-store, private(중간 캐시 금지)"),
                ("Vary" = String, description = "Authorization"),
            )),
        (status = 206, description = "부분 본문(Range, Content-Type=저장 타입)", body = inline(OctetStreamBody), content_type = "*/*",
            headers(
                ("Content-Range" = String, description = "bytes <start>-<end>/<total>"),
                ("ETag" = String, description = "강한 ETag"),
                ("Accept-Ranges" = String, description = "bytes"),
                ("Content-Length" = String, description = "부분 바이트 수"),
                ("Content-Type" = String, description = "저장된 콘텐츠 타입"),
                ("Last-Modified" = String, description = "업로드 시각"),
                ("Cache-Control" = String, description = "no-store, private"),
                ("Vary" = String, description = "Authorization"),
            )),
        (status = 304, description = "Not Modified(본문 없음, ETag만)"),
        (status = 416, description = "Range 불충족",
            headers(("Content-Range" = String, description = "bytes */<total>"))),
        (status = 400, description = "잘못된 키(invalid_key·reserved_suffix)", body = ErrorResponse),
        (status = 401, description = "인증 실패", body = ErrorResponse),
        (status = 403, description = "읽기 스코프 없음", body = ErrorResponse),
        (status = 404, body = ErrorResponse),
    ),
    security(("bearer" = [])),
)]
pub(crate) async fn get_file(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path(bucket): Path<String>,
    Query(KeyQuery { key: obj_key }): Query<KeyQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if !key.can_read(&bucket) {
        return Err(AppError::Forbidden);
    }
    let (meta, file) = st.store.open(&bucket, &obj_key).await?;
    let mut resp = build_ranged(&headers, &meta, file).await;
    // 인증된 내부 객체 읽기 — 객체 식별자가 ?key= 쿼리에 있어 프록시가 쿼리를 무시/정규화하면
    // 오배달·접근 로그 키 노출 위험(codex). 중간 캐시 금지 + Authorization별 분리를 명시.
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store, private"));
    h.insert(header::VARY, HeaderValue::from_static("Authorization"));
    Ok(resp)
}

#[utoipa::path(
    head, path = "/api/files/{bucket}/object", tag = "files",
    params(("bucket" = String, Path), KeyQuery),
    responses(
        (status = 200, description = "메타데이터 헤더만(본문 없음)",
            headers(
                ("ETag" = String, description = "강한 ETag(\"<sha256>\")"),
                ("Accept-Ranges" = String, description = "bytes"),
                ("Content-Length" = String, description = "본문 바이트 수"),
                ("Content-Type" = String, description = "저장된 콘텐츠 타입"),
                ("Cache-Control" = String, description = "no-store, private"),
                ("Vary" = String, description = "Authorization"),
            )),
        (status = 400, description = "잘못된 키(invalid_key·reserved_suffix)", body = ErrorResponse),
        (status = 401, description = "인증 실패", body = ErrorResponse),
        (status = 403, description = "읽기 스코프 없음", body = ErrorResponse),
        (status = 404, body = ErrorResponse),
    ),
    security(("bearer" = [])),
)]
pub(crate) async fn head_file(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path(bucket): Path<String>,
    Query(KeyQuery { key: obj_key }): Query<KeyQuery>,
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
    // 인증된 내부 읽기 — GET과 동일 캐시/분리 정책(codex finding3)
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store, private"));
    h.insert(header::VARY, HeaderValue::from_static("Authorization"));
    Ok(resp)
}

#[utoipa::path(
    delete, path = "/api/files/{bucket}/object", tag = "files",
    params(("bucket" = String, Path), KeyQuery),
    responses(
        (status = 204, description = "삭제됨(멱등)"),
        (status = 400, description = "잘못된 키(invalid_key·reserved_suffix)", body = ErrorResponse),
        (status = 401, description = "인증 실패", body = ErrorResponse),
        (status = 403, description = "쓰기 스코프 없음", body = ErrorResponse),
    ),
    security(("bearer" = [])),
)]
pub(crate) async fn delete_file(
    State(st): State<AppState>,
    AuthKey(key): AuthKey,
    Path(bucket): Path<String>,
    Query(KeyQuery { key: obj_key }): Query<KeyQuery>,
) -> Result<Response, AppError> {
    if !key.can_write(&bucket) {
        return Err(AppError::Forbidden);
    }
    st.store.delete(&bucket, &obj_key).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub(crate) struct CreateBucket {
    visibility: Visibility,
}

#[utoipa::path(
    put, path = "/api/buckets/{bucket}", tag = "buckets",
    params(("bucket" = String, Path)),
    request_body = CreateBucket,
    responses(
        (status = 201, description = "버킷 생성/가시성 설정", body = BucketMeta),
        (status = 400, description = "예약 버킷명", body = ErrorResponse),
        (status = 401, description = "인증 실패", body = ErrorResponse),
        (status = 403, description = "admin 아님", body = ErrorResponse),
    ),
    security(("bearer" = [])),
)]
pub(crate) async fn put_bucket(
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

#[derive(serde::Serialize, utoipa::ToSchema)]
pub(crate) struct BucketEntry {
    bucket: String,
    #[serde(flatten)]
    meta: BucketMeta,
}

#[utoipa::path(
    get, path = "/api/buckets", tag = "buckets",
    responses(
        (status = 200, description = "버킷 목록", body = Vec<BucketEntry>),
        (status = 401, description = "인증 실패", body = ErrorResponse),
        (status = 403, body = ErrorResponse),
    ),
    security(("bearer" = [])),
)]
pub(crate) async fn get_buckets(State(st): State<AppState>, AuthKey(key): AuthKey) -> Result<Response, AppError> {
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

#[derive(serde::Serialize, utoipa::ToSchema)]
pub(crate) struct ObjectEntry {
    key: String,
    #[serde(flatten)]
    meta: ObjectMeta,
}

#[utoipa::path(
    get, path = "/api/files/{bucket}", tag = "files",
    params(("bucket" = String, Path)),
    responses(
        (status = 200, description = "버킷 내 객체 목록(non-servable 제외)", body = Vec<ObjectEntry>),
        (status = 403, body = ErrorResponse),
    ),
    security(("bearer" = [])),
)]
pub(crate) async fn list_files(
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

#[utoipa::path(get, path = "/healthz", tag = "health", responses((status = 200, description = "liveness")))]
pub(crate) async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// `/data` 쓰기 가능 + free-space ≥ min_free 확인.
#[utoipa::path(
    get, path = "/readyz", tag = "health",
    responses((status = 200, description = "/data 쓰기가능 + free-space 충족"), (status = 503, description = "저하")),
)]
pub(crate) async fn readyz(State(st): State<AppState>) -> StatusCode {
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
}
