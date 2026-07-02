//! 코드-우선 OpenAPI 서빙 계약 — 내부 리스너가 생성된 스펙(/openapi.json)만 서빙하고
//! 인터랙티브 UI(/docs)는 서빙하지 않는지(공급망 벡터 차단 결정 잠금).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{body_json, internal_app};

#[tokio::test]
async fn serves_generated_openapi_spec_unauthenticated() {
    let (app, _d) = internal_app(r#"[{"id":"a","sha256":"00","service":"ops","admin":true}]"#);
    // /openapi.json은 인증 없이 서빙(문서는 비밀 아님)
    let res = app
        .oneshot(Request::builder().uri("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let doc = body_json(res).await;
    assert!(
        doc["openapi"].as_str().unwrap_or("").starts_with("3."),
        "openapi 버전 필드: {doc}"
    );
    // 코드에서 파생된 경로/스키마가 실제로 담겨 있어야 한다
    assert!(
        doc["paths"]["/api/files/{bucket}/object"]["put"].is_object(),
        "put_file 경로 누락"
    );
    assert!(
        doc["components"]["schemas"]["ObjectMeta"].is_object(),
        "ObjectMeta 스키마 누락"
    );
}

/// 코드-우선 스펙이 실제 동작을 정확히 기술하는지(삭제된 yaml 회귀 방지):
/// ① 업로드 바디 = 바이너리(텍스트 아님) ② 스펙은 **내부 전용** — 공개 경로(다른 origin :8081)는 스펙 밖.
#[tokio::test]
async fn spec_binary_upload_and_internal_only() {
    let (app, _d) = internal_app(r#"[{"id":"a","sha256":"00","service":"ops","admin":true}]"#);
    let res = app
        .oneshot(Request::builder().uri("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let doc = body_json(res).await;

    // ① PUT 업로드 = */* + format:binary (임의 Content-Type 허용·보존 — 다운로드 동적 타입과 대칭)
    let put_rb = &doc["paths"]["/api/files/{bucket}/object"]["put"]["requestBody"]["content"];
    assert_eq!(
        put_rb["*/*"]["schema"]["format"].as_str(),
        Some("binary"),
        "PUT 업로드 바디는 */* + format:binary 여야(임의 Content-Type 보존): {put_rb}"
    );

    // finding6 해소: 키는 query 파라미터(슬래시 포함 중첩 키 OpenAPI 정합 — path 세그먼트 아님)
    let put_params = doc["paths"]["/api/files/{bucket}/object"]["put"]["parameters"]
        .as_array()
        .expect("put parameters");
    let key_param = put_params
        .iter()
        .find(|p| p["name"] == "key")
        .expect("key 파라미터 존재");
    assert_eq!(
        key_param["in"].as_str(),
        Some("query"),
        "키는 query 파라미터여야(중첩 키 정합): {key_param}"
    );

    // ② 내부 전용 — /openapi.json은 내부 리스너 서빙이라 상대경로가 내부 origin으로 해석된다.
    //    공개 다운로드/카탈로그(별도 origin :8081)를 여기 담으면 클라이언트가 잘못된 origin을 가리키므로 스펙 밖.
    assert!(doc["paths"]["/api/files/{bucket}/object"].is_object(), "내부 경로 있어야");
    assert!(
        doc["paths"]["/{bucket}/{key}"].is_null(),
        "공개 다운로드 경로는 스펙 밖이어야(2리스너 origin 혼동 방지): {}",
        doc["paths"]
    );
    assert!(doc["paths"]["/"].is_null(), "공개 카탈로그 경로는 스펙 밖이어야");
}

/// codex pass5 finding1/2 회귀 방지: SDK 제거 후 스펙이 유일 계약이므로 다운로드(GET)가
/// ① 바이너리 바디(200·206 octet-stream+format:binary) ② Range 헤더(ETag·Accept-Ranges·
/// Content-Length·Content-Range)를 선언해야 생성기가 void로 모델링하지 않는다. 또한 key
/// 파라미터가 런타임 문법(min/max/pattern)을 계약으로 노출해야 client drift를 막는다.
#[tokio::test]
async fn spec_download_declares_binary_range_and_key_grammar() {
    let (app, _d) = internal_app(r#"[{"id":"a","sha256":"00","service":"ops","admin":true}]"#);
    let res = app
        .oneshot(Request::builder().uri("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let doc = body_json(res).await;
    let get = &doc["paths"]["/api/files/{bucket}/object"]["get"];

    // ① 200·206 바디 = 바이너리(format:binary). 미디어 타입은 저장 타입이 동적이라 `*/*`로 문서화
    //    (octet-stream 고정이면 text/plain·image/png 다운로드가 미문서 타입이 되는 드리프트 — pass6 finding1).
    for status in ["200", "206"] {
        let schema = &get["responses"][status]["content"]["*/*"]["schema"];
        assert_eq!(
            schema["format"].as_str(),
            Some("binary"),
            "{status} 응답은 `*/*` 바이너리 바디여야: {}",
            get["responses"][status]
        );
    }

    // ② Range 다운로드 헤더 문서화: 206 Content-Range, 200 ETag/Accept-Ranges/Content-Length
    assert!(
        get["responses"]["206"]["headers"]["Content-Range"].is_object(),
        "206 Content-Range 헤더 누락: {}",
        get["responses"]["206"]
    );
    for hname in ["ETag", "Accept-Ranges", "Content-Length"] {
        assert!(
            get["responses"]["200"]["headers"][hname].is_object(),
            "200 {hname} 헤더 누락: {}",
            get["responses"]["200"]
        );
    }
    // 206도 실제 런타임처럼 Last-Modified·Cache-Control·Vary를 문서화(pass6 finding1)
    for hname in ["Last-Modified", "Cache-Control", "Vary"] {
        assert!(
            get["responses"]["206"]["headers"][hname].is_object(),
            "206 {hname} 헤더 누락: {}",
            get["responses"]["206"]
        );
    }

    // key 파라미터 문법 계약(런타임 valid_key와 정합: 1..=1024, 세그먼트 문법 pattern)
    let key_param = get["parameters"]
        .as_array()
        .expect("get parameters")
        .iter()
        .find(|p| p["name"] == "key")
        .expect("key 파라미터 존재");
    let schema = &key_param["schema"];
    assert_eq!(schema["maxLength"].as_i64(), Some(1024), "key maxLength=1024: {key_param}");
    assert_eq!(schema["minLength"].as_i64(), Some(1), "key minLength=1: {key_param}");
    assert!(schema["pattern"].as_str().is_some(), "key pattern 누락: {key_param}");
}

/// pass7 finding2(+일관성 완결): 객체 4연산이 실제 반환하는 에러 코드(400 invalid_key·reserved_suffix,
/// 401 인증 실패, 403 스코프 없음)를 전부 스펙에 문서화했는지. SDK 제거로 스펙이 유일 계약이라
/// 에러 경로도 계약에 있어야 생성 클라이언트가 모델링한다.
#[tokio::test]
async fn spec_object_ops_document_error_codes() {
    let (app, _d) = internal_app(r#"[{"id":"a","sha256":"00","service":"ops","admin":true}]"#);
    let res = app
        .oneshot(Request::builder().uri("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let doc = body_json(res).await;
    let ops = &doc["paths"]["/api/files/{bucket}/object"];
    for method in ["put", "get", "head", "delete"] {
        for code in ["400", "401", "403"] {
            let r = &ops[method]["responses"][code];
            assert!(r.is_object(), "{method} {code} 응답 미문서화: {}", ops[method]["responses"]);
            let schema_ref = r["content"]["application/json"]["schema"]["$ref"]
                .as_str()
                .unwrap_or("");
            assert!(
                schema_ref.ends_with("ErrorResponse"),
                "{method} {code} 바디는 ErrorResponse여야: {r}"
            );
        }
    }

    // 버킷 연산도 인증 에러(401/403) 문서화 — 전 인증 엔드포인트 일관. put_bucket은 400(예약명)도.
    let put_bucket = &doc["paths"]["/api/buckets/{bucket}"]["put"]["responses"];
    for code in ["400", "401", "403"] {
        assert!(put_bucket[code].is_object(), "put_bucket {code} 미문서화: {put_bucket}");
    }
    let get_buckets = &doc["paths"]["/api/buckets"]["get"]["responses"];
    for code in ["401", "403"] {
        assert!(get_buckets[code].is_object(), "get_buckets {code} 미문서화: {get_buckets}");
    }
}

/// 보안 결정 잠금: 인터랙티브 UI(/docs)는 서빙하지 않는다 — 어떤 OpenAPI UI(Scalar/Swagger/Redoc)든
/// API origin에 CDN 서드파티 JS를 로드해 try-it Bearer 키 탈취 공급망 벡터가 되기 때문(codex 리뷰 HIGH).
/// CDN-로드 UI를 다시 붙이면 이 테스트가 깨져 경고한다. 스펙 렌더는 소비자 로컬 도구로.
#[tokio::test]
async fn does_not_serve_interactive_docs_ui() {
    let (app, _d) = internal_app(r#"[{"id":"a","sha256":"00","service":"ops","admin":true}]"#);
    let res = app
        .oneshot(Request::builder().uri("/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
