//! Execution run trigger, run history, metrics stats, and delivery resend endpoints.

use axum::extract::{Path, State};
use axum::Json;

use crate::api::digests::PaginationQuery;
use crate::api::error::{parse_uuid, ApiError, ApiQuery, OptionalApiJson, RequestId};
use crate::auth::AuthenticatedUser;
use crate::repo::{DeliveryLogRecord, DigestRunView, PageRequest, PagedResult, RunStatsView};
use crate::services::TriggerRunRequest;
use crate::state::AppState;

/// `POST /api/v1/runs/trigger` — triggers an immediate fetch + summarize (+ optional notify) run.
pub async fn trigger_run(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    OptionalApiJson(body): OptionalApiJson<TriggerRunRequest>,
) -> Result<Json<DigestRunView>, ApiError> {
    let view = state
        .executor
        .trigger_manual_run(user.id, body)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(view))
}

/// `GET /api/v1/runs` — lists paginated execution runs with child delivery logs.
pub async fn list_runs(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiQuery(query): ApiQuery<PaginationQuery>,
) -> Result<Json<PagedResult<DigestRunView>>, ApiError> {
    let page = PageRequest::new(query.page, query.page_size)
        .map_err(|err| ApiError::new(&req_id.0, err))?;

    let runs = state
        .repos
        .runs
        .list_runs_with_deliveries(user.id, page)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(runs))
}

/// `GET /api/v1/runs/stats` — returns token usage, source health, and delivery success metrics.
pub async fn get_run_stats(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
) -> Result<Json<RunStatsView>, ApiError> {
    let stats = state
        .repos
        .runs
        .get_run_stats(user.id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(stats))
}

/// `POST /api/v1/deliveries/{id}/resend` — re-dispatches a delivery log.
pub async fn resend_delivery(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(id): Path<String>,
) -> Result<Json<DeliveryLogRecord>, ApiError> {
    let delivery_id = parse_uuid(&id, "deliveryId").map_err(|err| ApiError::new(&req_id.0, err))?;

    let record = state
        .executor
        .resend_delivery(user.id, delivery_id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(record))
}
