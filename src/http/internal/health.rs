use crate::capacity::free_bytes;
use crate::http::AppState;
use axum::extract::State;
use axum::http::StatusCode;

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
