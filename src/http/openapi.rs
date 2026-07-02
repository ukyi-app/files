//! 코드-우선 OpenAPI 스펙(utoipa) — 수기 openapi.yaml을 대체한다. 코드가 곧 계약이라 드리프트 불가.
//! 내부 리스너(:8080)의 `/openapi.json`으로 스펙만 서빙. 인터랙티브 UI(Scalar/Swagger 등)는
//! API origin에 CDN 서드파티 JS를 로드하는 공급망 리스크(codex HIGH)라 미서빙 — 소비자 로컬 도구로 렌더.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

/// API 에러 응답 바디 `{"error": "<code>"}` — 안정적 에러 코드.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    /// 안정적 에러 코드(not_found·unauthorized·forbidden·too_large·insufficient_storage 등)
    pub error: String,
}

/// Bearer 보안 스킴을 components에 주입(핸들러 `security(("bearer" = []))` 참조).
struct SecurityAddon;
impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi
            .components
            .get_or_insert_with(utoipa::openapi::Components::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}

/// files API의 OpenAPI 스펙(코드 파생).
#[derive(OpenApi)]
#[openapi(
    info(
        title = "files — 홈랩 공용 파일 스토어",
        version = "0.1.0",
        description = "content-addressed blob 스토어의 **내부 API**(:8080, `/api/*`, Bearer 키) 계약. \
                       이 스펙은 내부 리스너에 서빙되므로 내부 표면만 기술한다 — 공개 다운로드 표면(:8081, \
                       `files.ukyi.app/{bucket}/{key}`, 무인증)은 별도 origin의 단순 GET이라 스펙 밖(2리스너 origin 혼동 방지)."
    ),
    paths(
        crate::http::internal::files::put_file,
        crate::http::internal::files::get_file,
        crate::http::internal::files::head_file,
        crate::http::internal::files::delete_file,
        crate::http::internal::files::list_files,
        crate::http::internal::buckets::put_bucket,
        crate::http::internal::buckets::get_buckets,
        crate::http::internal::health::healthz,
        crate::http::internal::health::readyz,
    ),
    components(schemas(
        crate::meta::ObjectMeta,
        crate::meta::BucketMeta,
        crate::meta::Visibility,
        crate::http::internal::files::ObjectEntry,
        crate::http::internal::buckets::BucketEntry,
        crate::http::internal::buckets::CreateBucket,
        ErrorResponse,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "files", description = "파일 CRUD (내부, 키 필요)"),
        (name = "buckets", description = "버킷 생성/목록 (admin)"),
        (name = "health", description = "liveness/readiness"),
    ),
)]
pub struct ApiDoc;
