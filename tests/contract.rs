//! OpenAPI 계약 테스트 — 테스트 서버 응답 형태가 **코드 생성 스펙**(utoipa ApiDoc)과 일치하는지 검증.
//! (수기 openapi.yaml 제거 후 코드-우선. 스키마는 응답 타입에서 파생되므로 이 테스트는 핸들러
//!  #[utoipa::path] 어노테이션이 실제 반환 형태와 일치함을 지킨다.)

use axum::http::StatusCode;
use tower::ServiceExt;
use utoipa::OpenApi;

mod common;
use common::{bearer, body_json, hex_sha, internal_app};

/// 스키마의 required 필드 전부 수집. utoipa는 `#[serde(flatten)]`을 allOf(+$ref)로 표현하므로
/// 직접 `.required`뿐 아니라 allOf 멤버($ref는 해석)까지 재귀 수집한다.
fn schema_required(doc: &serde_json::Value, name: &str) -> Vec<String> {
    let mut v = collect_required(doc, &doc["components"]["schemas"][name]);
    v.sort();
    v.dedup();
    v
}

fn collect_required(doc: &serde_json::Value, schema: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(req) = schema["required"].as_array() {
        out.extend(req.iter().filter_map(|x| x.as_str().map(String::from)));
    }
    if let Some(all) = schema["allOf"].as_array() {
        for m in all {
            if let Some(r) = m["$ref"].as_str() {
                let n = r.rsplit('/').next().unwrap();
                out.extend(collect_required(doc, &doc["components"]["schemas"][n]));
            } else {
                out.extend(collect_required(doc, m));
            }
        }
    }
    out
}

fn json_keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[tokio::test]
async fn responses_match_openapi_schema() {
    let doc: serde_json::Value =
        serde_json::to_value(files::http::openapi::ApiDoc::openapi()).unwrap();
    let keys = format!(
        r#"[{{"id":"w","sha256":"{}","service":"page","writeBuckets":["skills"],"readBuckets":["skills"]}},{{"id":"a","sha256":"{}","service":"ops","admin":true}}]"#,
        hex_sha(b"writer"),
        hex_sha(b"admin")
    );
    let (app, _d) = internal_app(&keys);

    // PUT object → ObjectMeta
    let res = app
        .clone()
        .oneshot(bearer("PUT", "/api/files/skills/object?key=c.txt", "writer", "text/plain", "data"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(json_keys(&body_json(res).await), schema_required(&doc, "ObjectMeta"));

    // GET missing → Error
    let res = app
        .clone()
        .oneshot(bearer("GET", "/api/files/skills/object?key=missing", "writer", "text/plain", ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_keys(&body_json(res).await), schema_required(&doc, "ErrorResponse"));

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
