//! Full-text item search endpoint (`GET /api/v1/items`).

use axum::extract::State;
use axum::Json;
use digest_core::error::CoreError;
use serde::Deserialize;

use crate::api::error::{parse_date, parse_uuid, ApiError, ApiQuery, RequestId};
use crate::auth::AuthenticatedUser;
use crate::repo::{ItemSearchQuery, ItemView, PageRequest, PagedResult};
use crate::state::AppState;

/// Query parameters for `GET /api/v1/items`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItemsQuery {
    pub q: Option<String>,
    #[serde(alias = "source_id")]
    pub source_id: Option<String>,
    #[serde(alias = "date_from")]
    pub date_from: Option<String>,
    #[serde(alias = "date_to")]
    pub date_to: Option<String>,
    pub keyword: Option<String>,
    pub bookmarked: Option<bool>,
    pub page: Option<usize>,
    #[serde(alias = "page_size")]
    pub page_size: Option<usize>,
}

/// `GET /api/v1/items` — full-text and metadata search isolated to the user's subscriptions.
pub async fn search_items(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiQuery(raw): ApiQuery<SearchItemsQuery>,
) -> Result<Json<PagedResult<ItemView>>, ApiError> {
    let page =
        PageRequest::new(raw.page, raw.page_size).map_err(|err| ApiError::new(&req_id.0, err))?;

    let parsed_source_id = match raw
        .source_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(val) => {
            Some(parse_uuid(val, "sourceId").map_err(|err| ApiError::new(&req_id.0, err))?)
        }
        None => None,
    };

    let resolved_source_id = match parsed_source_id {
        Some(sid) => {
            if let Some(sub) = state
                .repos
                .sources
                .get_user_source(user.id, sid)
                .await
                .map_err(|err| ApiError::new(&req_id.0, err))?
            {
                Some(sub.canonical_source_id)
            } else {
                Some(sid)
            }
        }
        None => None,
    };

    let date_from = match raw
        .date_from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(val) => {
            Some(parse_date(val, "dateFrom").map_err(|err| ApiError::new(&req_id.0, err))?)
        }
        None => None,
    };

    let date_to = match raw
        .date_to
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(val) => Some(parse_date(val, "dateTo").map_err(|err| ApiError::new(&req_id.0, err))?),
        None => None,
    };

    if let (Some(from), Some(to)) = (date_from, date_to) {
        if from > to {
            return Err(ApiError::new(
                req_id.0,
                CoreError::InvalidInput("dateFrom must be <= dateTo".to_owned()),
            ));
        }
    }

    let q = raw.q.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let keyword = raw
        .keyword
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());

    let query = ItemSearchQuery {
        q,
        source_id: resolved_source_id,
        date_from,
        date_to,
        keyword,
        bookmarked: raw.bookmarked,
        page,
    };

    let result = state
        .repos
        .sources
        .search_user_items(user.id, query)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(result))
}
