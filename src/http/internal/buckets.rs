use crate::error::AppError;
use crate::http::openapi::ErrorResponse;
use crate::http::{AppState, AuthKey};
use crate::meta::{BucketMeta, Visibility};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

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
