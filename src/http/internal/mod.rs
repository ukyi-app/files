use super::AppState;
use axum::routing::{get, put};
use axum::{Json, Router};
use utoipa::OpenApi;

// ⚠️ pub(crate) 필수(Phase C 소견#1): openapi.rs의 paths(...)가 sibling 모듈에서
// crate::http::internal::files::* 등 경로를 직접 참조한다. `mod files;`(private)이면
// 핸들러가 pub(crate)여도 경로가 `files` 컴포넌트에서 막혀 컴파일 실패한다.
// 아이템 자체는 pub(crate) 유지 → 외부 크레이트 API는 넓히지 않는다.
pub(crate) mod buckets;
pub(crate) mod files;
pub(crate) mod health;
#[cfg(test)]
mod tests;

/// 내부 API 라우터(파일 CRUD + 버킷 + 헬스 + OpenAPI 문서). 헬스/문서 외 라우트는 인증 필요.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route(
            "/api/files/{bucket}/object",
            put(files::put_file)
                .get(files::get_file)
                .head(files::head_file)
                .delete(files::delete_file),
        )
        .route("/api/files/{bucket}", get(files::list_files))
        .route("/api/buckets/{bucket}", put(buckets::put_bucket))
        .route("/api/buckets", get(buckets::get_buckets))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
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
