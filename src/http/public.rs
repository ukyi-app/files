use super::ranged::build_ranged;
use super::AppState;
use crate::error::AppError;
use crate::meta::Visibility;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, Response};
use axum::routing::{any, get};
use axum::Router;

/// 공개 라우터(인증 없음). 쓰기 `/api` 핸들러 부재 = 표면 분리를 라우터 자체로 강제.
/// catch-all 앞에서 `/api/*`·`/healthz`·`/readyz`를 명시 404(발견 P4-2).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(catalog))
        .route("/api/{*rest}", any(not_found))
        .route("/healthz", any(not_found))
        .route("/readyz", any(not_found))
        .route("/{bucket}/{*key}", get(public_download))
        .with_state(state)
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn public_download(
    State(st): State<AppState>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // public 버킷만 서빙(internal/없음/예약명 모두 404 — 존재 비노출)
    match st.store.get_bucket(&bucket).await {
        Ok(bm) if bm.visibility == Visibility::Public => {}
        _ => return Err(AppError::NotFound),
    }
    let (meta, file) = st
        .store
        .open(&bucket, &key)
        .await
        .map_err(|_| AppError::NotFound)?;
    let mut resp = build_ranged(&headers, &meta, file).await;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
}

/// public 버킷 목록 카탈로그(사용자 콘텐츠 텍스트 이스케이프).
async fn catalog(State(st): State<AppState>) -> Result<Html<String>, AppError> {
    let base = &st.cfg.public_base_url;
    let mut html = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>files</title></head><body><h1>files</h1>",
    );
    for (name, bm) in st.store.list_buckets().await? {
        if bm.visibility != Visibility::Public {
            continue;
        }
        html.push_str(&format!("<h2>{}</h2><ul>", escape(&name)));
        for (key, _meta) in st.store.list(&name).await? {
            let url = format!("{base}/{name}/{key}");
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                escape(&url),
                escape(&key)
            ));
        }
        html.push_str("</ul>");
    }
    html.push_str("</body></html>");
    Ok(Html(html))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::KeyRegistry;
    use crate::capacity::Capacity;
    use crate::config::Config;
    use crate::meta::{BucketMeta, Visibility};
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn pub_app() -> (Router, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path().to_path_buf());
        let now = crate::clock::now_rfc3339();
        store
            .put_bucket(
                "downloads",
                &BucketMeta {
                    visibility: Visibility::Public,
                    owner: "o".into(),
                    created_at: now.clone(),
                },
            )
            .await
            .unwrap();
        store
            .put("downloads", "file.txt", "text/plain", "u", b"public data".to_vec())
            .await
            .unwrap();
        store
            .put_bucket(
                "secret",
                &BucketMeta {
                    visibility: Visibility::Internal,
                    owner: "o".into(),
                    created_at: now,
                },
            )
            .await
            .unwrap();
        store
            .put("secret", "hidden.txt", "text/plain", "u", b"nope".to_vec())
            .await
            .unwrap();
        let cfg = Config::from_env(|k| match k {
            "FILES_DATA_DIR" => Some("/tmp".into()),
            "FILES_KEYS_PATH" => Some("/tmp/keys.json".into()),
            _ => None,
        })
        .unwrap();
        let state = AppState {
            store,
            keys: Arc::new(KeyRegistry::from_keys(vec![])),
            cap: Capacity::with_free_fn(0, || Ok(u64::MAX)),
            cfg: Arc::new(cfg),
        };
        (router(state), d)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    async fn body_str(resp: axum::response::Response) -> String {
        let b = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn public_download_200_with_security_headers() {
        let (app, _d) = pub_app().await;
        let res = app.oneshot(get("/downloads/file.txt")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get(header::CONTENT_DISPOSITION).unwrap().to_str().unwrap(),
            "attachment"
        );
        assert_eq!(
            res.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap().to_str().unwrap(),
            "nosniff"
        );
        assert_eq!(body_str(res).await, "public data");
    }

    #[tokio::test]
    async fn internal_bucket_not_served_publicly_404() {
        let (app, _d) = pub_app().await;
        let res = app.oneshot(get("/secret/hidden.txt")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn missing_object_404() {
        let (app, _d) = pub_app().await;
        let res = app.oneshot(get("/downloads/nope.txt")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn public_api_path_404() {
        let (app, _d) = pub_app().await;
        let res = app.oneshot(get("/api/files/downloads/file.txt")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn no_method_reaches_api_surface_on_public() {
        let (app, _d) = pub_app().await;
        for (method, uri) in [
            ("PUT", "/api/files/downloads/x.txt"),
            ("DELETE", "/api/files/downloads/file.txt"),
            ("POST", "/api/buckets/downloads"),
            ("GET", "/api/buckets"),
            ("GET", "/api/files/downloads"),
        ] {
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::NOT_FOUND,
                "{method} {uri}는 공개 표면에서 404여야 함"
            );
        }
        // 쓰기가 도달하지 않았음을 확인: 기존 객체 그대로
        let res = app.oneshot(get("/downloads/file.txt")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_str(res).await, "public data");
    }

    #[tokio::test]
    async fn reserved_bucket_names_cannot_be_created() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path().to_path_buf());
        for reserved in ["api", "healthz", "readyz"] {
            let r = store
                .put_bucket(
                    reserved,
                    &BucketMeta {
                        visibility: Visibility::Public,
                        owner: "o".into(),
                        created_at: crate::clock::now_rfc3339(),
                    },
                )
                .await;
            assert!(
                matches!(r, Err(crate::error::AppError::BadRequest(_))),
                "{reserved}는 예약되어 생성 거부되어야 함"
            );
        }
    }

    #[tokio::test]
    async fn catalog_lists_public_only() {
        let (app, _d) = pub_app().await;
        let res = app.oneshot(get("/")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = body_str(res).await;
        assert!(html.contains("downloads") && html.contains("file.txt"), "catalog: {html}");
        assert!(!html.contains("secret") && !html.contains("hidden"), "internal leaked: {html}");
    }
}
