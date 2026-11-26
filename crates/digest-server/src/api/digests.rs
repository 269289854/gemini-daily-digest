//! Daily digest listing, detail lookup, and Markdown export endpoints.

use axum::extract::{Path, State};
use axum::Json;
use digest_core::domain::{AlertPriority, ProcessedItem, RenderInput};
use digest_core::error::CoreError;
use digest_core::renderer::MarkdownDigestRenderer;
use digest_core::traits::SummarizedDigest;
use http::header::CONTENT_TYPE;
use http::StatusCode;
use serde::Deserialize;

use crate::api::error::{parse_date, ApiError, ApiQuery, RequestId};
use crate::auth::AuthenticatedUser;
use crate::constants::CONTENT_TYPE_MARKDOWN_UTF8;
use crate::repo::{DailyDigestDetail, DailyDigestSummary, PageRequest, PagedResult};
use crate::state::AppState;

/// Shared pagination query parameters (`page`, `pageSize`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginationQuery {
    pub page: Option<usize>,
    #[serde(alias = "page_size")]
    pub page_size: Option<usize>,
}

/// `GET /api/v1/digests` — paginated daily digest summaries.
pub async fn list_digests(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    ApiQuery(query): ApiQuery<PaginationQuery>,
) -> Result<Json<PagedResult<DailyDigestSummary>>, ApiError> {
    let page = PageRequest::new(query.page, query.page_size)
        .map_err(|err| ApiError::new(&req_id.0, err))?;
    let result = state
        .repos
        .digests
        .list_daily_digests(user.id, page)
        .await
        .map_err(|err| ApiError::new(req_id.0, err))?;

    Ok(Json(result))
}

/// `GET /api/v1/digests/{date}` — full daily digest detail for `YYYY-MM-DD`.
pub async fn get_digest_by_date(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(date): Path<String>,
) -> Result<Json<DailyDigestDetail>, ApiError> {
    let parsed_date = parse_date(&date, "date").map_err(|err| ApiError::new(&req_id.0, err))?;
    let detail = state
        .repos
        .digests
        .get_daily_digest_by_date(user.id, parsed_date)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?
        .ok_or_else(|| {
            ApiError::new(
                req_id.0,
                CoreError::NotFound(format!("daily digest not found for {parsed_date}")),
            )
        })?;

    Ok(Json(detail))
}

/// `GET /api/v1/digests/{date}/markdown` — raw Markdown export (`text/markdown; charset=utf-8`).
pub async fn get_digest_markdown(
    State(state): State<AppState>,
    req_id: RequestId,
    AuthenticatedUser(user): AuthenticatedUser,
    Path(date): Path<String>,
) -> Result<(StatusCode, [(http::HeaderName, &'static str); 1], String), ApiError> {
    let parsed_date = parse_date(&date, "date").map_err(|err| ApiError::new(&req_id.0, err))?;
    let detail = state
        .repos
        .digests
        .get_daily_digest_by_date(user.id, parsed_date)
        .await
        .map_err(|err| ApiError::new(&req_id.0, err))?
        .ok_or_else(|| {
            ApiError::new(
                req_id.0,
                CoreError::NotFound(format!("daily digest not found for {parsed_date}")),
            )
        })?;

    let markdown = render_detail_as_markdown(detail, user.preferred_locale);
    Ok((
        StatusCode::OK,
        [(CONTENT_TYPE, CONTENT_TYPE_MARKDOWN_UTF8)],
        markdown,
    ))
}

fn render_detail_as_markdown(detail: DailyDigestDetail, locale: String) -> String {
    let all_items: Vec<ProcessedItem> = detail
        .sections
        .iter()
        .flat_map(|section| section.items.iter().cloned())
        .collect();
    let highlighted_items: Vec<ProcessedItem> = all_items
        .iter()
        .filter(|item| item.match_result.priority != AlertPriority::Normal)
        .cloned()
        .collect();

    let priority = if highlighted_items
        .iter()
        .any(|item| item.match_result.priority == AlertPriority::Urgent)
    {
        AlertPriority::Urgent
    } else if !highlighted_items.is_empty() {
        AlertPriority::High
    } else {
        AlertPriority::Normal
    };

    let input = RenderInput {
        digest_date: detail.digest_date,
        digest: SummarizedDigest {
            title: detail.title,
            overview_markdown: detail.overview_markdown,
            prompt_tokens: u32::try_from(detail.prompt_tokens.max(0)).unwrap_or(0),
            completion_tokens: u32::try_from(detail.completion_tokens.max(0)).unwrap_or(0),
            model_used: detail.model_used,
            degraded_fallback: detail.degraded_fallback,
        },
        sections: detail.sections,
        highlighted_items,
        all_items,
        priority,
        locale,
    };

    MarkdownDigestRenderer::new().render_markdown(&input)
}
