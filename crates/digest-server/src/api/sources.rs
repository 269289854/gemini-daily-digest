//! Per-source digest timeline and raw collected items endpoints.

use axum::extract::{Path, State};
use axum::Json;
use digest_core::error::CoreError;

use crate::api::digests::PaginationQuery;
use crate::api::error::{parse_uuid, ApiError, ApiQuery, RequestId};
use crate::auth::AuthenticatedUser;
use crate::repo::{ItemView, PageRequest, PagedResult, SourceDailyDigestView};
use crate::state::AppState;

/// `GET /api/v1/sources/{id}/digests` — independent daily digest timeline for one subscribed source.
pub async fn list_source_digests(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<PaginationQuery>,
) -> Result<Json<PagedResult<SourceDailyDigestView>>, ApiError> {
    let source_or_sub_id =
        parse_uuid(&id, "sourceId").map_err(|err| ApiError::new(&req_id.0, err))?;
    let page = PageRequest::new(query.page, query.page_size)
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let source = state
        .repos
        .sources
        .get_user_source(user.id, source_or_sub_id)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?
        .ok_or_else(|| {
            ApiError::new(
                &req_id.0,
                CoreError::NotFound(format!("subscribed source {source_or_sub_id} not found")),
            )
        })?;

    let result = state
        .repos
        .digests
        .list_source_daily_digests(user.id, source.canonical_source_id, page)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(result))
}

/// `GET /api/v1/sources/{id}/items` — paginated raw collected items for one subscribed source.
pub async fn list_source_items(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<PaginationQuery>,
) -> Result<Json<PagedResult<ItemView>>, ApiError> {
    let source_or_sub_id =
        parse_uuid(&id, "sourceId").map_err(|err| ApiError::new(&req_id.0, err))?;
    let page = PageRequest::new(query.page, query.page_size)
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let source = state
        .repos
        .sources
        .get_user_source(user.id, source_or_sub_id)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?
        .ok_or_else(|| {
            ApiError::new(
                &req_id.0,
                CoreError::NotFound(format!("subscribed source {source_or_sub_id} not found")),
            )
        })?;

    let result = state
        .repos
        .sources
        .list_source_items(user.id, source.canonical_source_id, page)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(result))
}
