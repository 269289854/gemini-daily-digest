//! User bookmark endpoints (`POST /api/v1/bookmarks`, `DELETE /api/v1/bookmarks/{itemId}`).

use axum::extract::{Path, State};
use axum::Json;
use http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::{parse_uuid, ApiError, ApiJson, RequestId};
use crate::auth::AuthenticatedUser;
use crate::repo::BookmarkView;
use crate::state::AppState;

/// Request body for `POST /api/v1/bookmarks`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertBookmarkRequest {
    #[serde(alias = "item_id")]
    pub item_id: Uuid,
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /api/v1/bookmarks` — create or update a bookmark on an item.
pub async fn upsert_bookmark(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiJson(body): ApiJson<UpsertBookmarkRequest>,
) -> Result<Json<BookmarkView>, ApiError> {
    let note = body
        .note
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty());

    let view = state
        .repos
        .sources
        .upsert_bookmark(user.id, body.item_id, note)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(view))
}

/// `DELETE /api/v1/bookmarks/{itemId}` — remove a user's bookmark.
pub async fn delete_bookmark(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(item_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let parsed_item_id =
        parse_uuid(&item_id, "itemId").map_err(|err| ApiError::new(&req_id.0, err))?;

    state
        .repos
        .sources
        .delete_bookmark(user.id, parsed_item_id)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(StatusCode::NO_CONTENT)
}
