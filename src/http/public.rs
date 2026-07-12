use super::ranged::build_ranged;
use super::AppState;
use crate::error::AppError;
use crate::meta::Visibility;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, Response};
use axum::routing::{any, get};
use axum::Router;

/// 공개 라우터가 catch-all 앞에서 가려야 하는 예약 경로 패턴(axum 0.8 문법) —
/// `layout::RESERVED_BUCKETS`의 각 예약명에 대한 "그림자 라우트"다.
///
/// **모양의 비대칭이 곧 관측 행동이다. 균일하게 펴지 마라**:
/// - `api`는 서브트리(`/api/{*rest}`) — `/api/*`가 전 메서드에서 404.
/// - `healthz`·`readyz`는 정확 일치 — 하위 경로(`/healthz/foo`)에는 예약 라우트가
///   없어 catch-all `/{bucket}/{*key}`(GET 전용)에 매칭되고, 비-GET은 axum이
///   405 + `Allow: GET,HEAD`를 낸다. 여기에 `/healthz/{*rest}`를 추가하면
///   그 405가 404로 바뀐다(= 행위 파손).
///
/// 이름만 담은 `RESERVED_BUCKETS`에서 이 목록을 파생할 수 없는 이유이기도 하다.
/// 두 목록의 정합성(양방향)은 `every_reserved_bucket_has_a_shadow_route`와
/// `every_shadow_route_names_a_reserved_bucket`이, 위 405 비대칭은
/// `reserved_route_shape_asymmetry_is_load_bearing`이 지킨다.
const RESERVED_ROUTES: &[&str] = &["/api/{*rest}", "/healthz", "/readyz"];

/// 공개 라우터(인증 없음). 쓰기 `/api` 핸들러 부재 = 표면 분리를 라우터 자체로 강제.
///
/// catch-all 앞에 `RESERVED_ROUTES`를 등록하되, **wire 효과는 항목마다 다르다**(발견 P4-2):
/// - `/api/{*rest}`(서브트리)는 실효가 있다 — 이게 없으면 `/api/x/y`가 catch-all
///   (GET 전용)에 걸려 비-GET이 405가 된다. 있으면 `any()`가 삼켜 전 메서드 404.
/// - `/healthz`·`/readyz`(정확 일치)는 wire 레벨에서 **axum fallback 404와 구별 불가능한
///   no-op**이다(둘 다 빈 바디 404). 예약을 라우터에 명시적으로 남겨 드리프트 가드가
///   물릴 곳을 주는 선언적 역할이지, 무언가를 새로 막지는 않는다.
pub fn router(state: AppState) -> Router {
    let mut r = Router::new().route("/", get(catalog));
    for pattern in RESERVED_ROUTES {
        r = r.route(pattern, any(not_found));
    }
    r.route("/{bucket}/{*key}", get(public_download))
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

// 공개 다운로드는 별도 origin(:8081)의 단순 GET — OpenAPI 스펙(내부 전용) 밖.
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

    /// 두-목록 드리프트 방지(정방향): `layout::RESERVED_BUCKETS`의 **모든** 예약명은
    /// 공개 라우터에 그림자 라우트(`/{name}` 정확 일치 **또는** `/{name}/…` 서브트리)를
    /// 가져야 한다.
    ///
    /// **이 테스트가 지키는 것**: 예약명이 공개 라우터에서 *인지되고 있다*는 것 —
    /// 즉 `layout`에 새 예약명을 추가하면서 `RESERVED_ROUTES`를 빼먹는 드리프트를
    /// 조용한 버그가 아니라 이 실패로 만든다.
    ///
    /// **지키지 않는 것 — "누수 차단"이 아니다**:
    /// - 예약명은 단일 세그먼트라 애초에 catch-all `/{bucket}/{*key}`(2세그먼트 이상 요구)에
    ///   도달할 수 없다. 라우트가 없으면 그냥 axum fallback 404다.
    /// - 이 테스트가 허용하는 "정확 일치" 그림자(`/healthz`)는 하위 경로
    ///   `/healthz/foo`가 catch-all → `public_download`에 도달하는 것을 **막지 못한다**.
    ///   그건 지금도 일어나는 현행 행동이고(GET이면 404 JSON), 의도된 것이다.
    ///
    /// 그림자의 **모양**(정확 일치 vs 서브트리)은 하위 경로의 비-GET 응답(404 vs 405)을
    /// 결정하는 관측 행동이므로 기계가 파생하지 않고 사람이 고른다 —
    /// 그 선택은 `reserved_route_shape_asymmetry_is_load_bearing`이 핀한다.
    #[test]
    fn every_reserved_bucket_has_a_shadow_route() {
        for name in crate::layout::RESERVED_BUCKETS {
            let exact = format!("/{name}");
            let subtree = format!("/{name}/");
            let shadowed = RESERVED_ROUTES
                .iter()
                .any(|r| **r == *exact || r.starts_with(&subtree));
            assert!(
                shadowed,
                "예약 버킷 {name:?}에 공개 라우터 그림자 라우트가 없다 \
                 (등록된 라우트: {RESERVED_ROUTES:?}). \
                 catch-all `/{{bucket}}/{{*key}}`가 가로채므로, public.rs의 RESERVED_ROUTES에 \
                 \"/{name}\"(정확 일치) 또는 \"/{name}/{{*rest}}\"(서브트리)를 추가하라 — \
                 둘 중 무엇이냐가 하위 경로의 비-GET 응답(404 vs 405)을 결정한다."
            );
        }
    }

    /// 두-목록 드리프트 방지(역방향): `RESERVED_ROUTES`의 **모든** 항목은 어떤 예약
    /// 버킷명에 대응해야 한다.
    ///
    /// 예약되지 않은 이름의 그림자 라우트가 섞이면(예: 누가 `/metrics/{*rest}`만 추가),
    /// 사용자가 정당하게 만들 수 있는 `metrics` 버킷의 공개 다운로드가 catch-all보다
    /// 먼저 그 라우트에 가로채여 **영구히 404**가 된다. 정방향 테스트는 이걸 못 잡는다.
    #[test]
    fn every_shadow_route_names_a_reserved_bucket() {
        for route in RESERVED_ROUTES {
            let first = route.trim_start_matches('/').split('/').next().unwrap_or("");
            assert!(
                crate::layout::RESERVED_BUCKETS.contains(&first),
                "그림자 라우트 {route:?}의 첫 세그먼트 {first:?}가 예약 버킷이 아니다 \
                 (layout::RESERVED_BUCKETS: {:?}). 이 라우트는 catch-all `/{{bucket}}/{{*key}}` \
                 앞에 등록되므로, 사용자가 만들 수 있는 동명 버킷의 공개 다운로드를 \
                 영구히 404로 만든다. layout에 예약명을 추가하든지, 이 라우트를 지워라.",
                crate::layout::RESERVED_BUCKETS
            );
        }
    }

    /// **예약 라우트의 모양이 곧 관측 행동이다** — 이 비대칭을 핀한다.
    /// "라우트를 균일하게 정리"하는 리팩터가 스위트를 초록으로 통과하면 안 된다.
    ///
    /// | 셀 | 현행 | 왜 |
    /// |---|---|---|
    /// | 비-GET `/healthz/foo`·`/readyz/foo` | 405 + `Allow: GET,HEAD` | 정확 일치 그림자뿐이라 하위 경로엔 예약 라우트가 **없고**, catch-all `/{bucket}/{*key}`(GET 전용)가 잡아 axum이 405를 낸다 |
    /// | GET `/healthz/foo` | 404 JSON `{"error":"not_found"}` | catch-all → `public_download` → 예약명 버킷 부재 |
    /// | 비-GET `/api/x/y` | 404 빈 바디 | 서브트리 그림자 `any(not_found)`가 전 메서드를 삼킴 |
    ///
    /// `RESERVED_ROUTES`에 `/healthz/{*rest}`를 추가하면 첫 줄의 405가 404로 바뀐다
    /// (= 행위 파손). 그 뮤턴트를 죽이는 것이 이 테스트의 존재 이유다.
    ///
    /// `OPTIONS`는 **의도적 제외**(빠뜨린 게 아니다 — 채워 넣지 마라): 공개 origin에
    /// `CorsLayer`가 도입되면 OPTIONS는 정당하게 처리되어 달라지는데, 그건 이 테스트가
    /// 지키는 성질(예약 라우트 **모양**의 비대칭)과 무관한 정상 변경이다. 무관한 이유로
    /// 깨지는 단언은 다음 사람에게 "테스트를 약화시켜라"를 학습시킨다.
    /// PUT/POST/DELETE만으로 위 뮤턴트 킬은 그대로 성립한다.
    #[tokio::test]
    async fn reserved_route_shape_asymmetry_is_load_bearing() {
        let (app, _d) = pub_app().await;
        const NON_GET: [&str; 3] = ["PUT", "POST", "DELETE"];

        // 정확 일치 그림자(healthz·readyz): 하위 경로의 비-GET → 405 + Allow: GET,HEAD
        for name in ["healthz", "readyz"] {
            for method in NON_GET {
                let uri = format!("/{name}/foo");
                let req = Request::builder()
                    .method(method)
                    .uri(&uri)
                    .body(Body::empty())
                    .unwrap();
                let res = app.clone().oneshot(req).await.unwrap();
                assert_eq!(
                    res.status(),
                    StatusCode::METHOD_NOT_ALLOWED,
                    "{method} {uri}는 catch-all(GET 전용)에 걸려 405여야 함 — \
                     RESERVED_ROUTES에 /{name}/{{*rest}}를 추가하면 404로 파손된다"
                );
                assert_eq!(
                    res.headers().get(header::ALLOW).map(|v| v.to_str().unwrap()),
                    Some("GET,HEAD"),
                    "{method} {uri}의 Allow 헤더"
                );
            }
            // 같은 경로의 GET은 catch-all 핸들러까지 도달 → 404 JSON(빈 바디가 아님)
            let uri = format!("/{name}/foo");
            let res = app.clone().oneshot(get(&uri)).await.unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "GET {uri}");
            assert_eq!(body_str(res).await, r#"{"error":"not_found"}"#, "GET {uri} 바디");
        }

        // 대조군 — 서브트리 그림자(api): 같은 메서드가 405가 아니라 404(빈 바디)
        for method in NON_GET {
            let req = Request::builder()
                .method(method)
                .uri("/api/x/y")
                .body(Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::NOT_FOUND,
                "{method} /api/x/y는 서브트리 그림자가 삼켜 404여야 함"
            );
            assert!(
                res.headers().get(header::ALLOW).is_none(),
                "{method} /api/x/y는 405가 아니므로 Allow 헤더가 없어야 함"
            );
            assert_eq!(body_str(res).await, "", "{method} /api/x/y는 빈 바디 404");
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
