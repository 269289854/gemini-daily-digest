//! Read-only MCP tool handlers (`McpAccessMode::ReadOnly`).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use digest_core::constants::DEFAULT_PAGE_SIZE;
use digest_core::domain::{McpAccessMode, McpToolName};
use digest_core::error::CoreError;
use serde_json::{json, Value};

use crate::client::DigestApiClient;
use crate::redact::sanitize_mcp_output;
use crate::tools::{
    api_path, optional_bool, optional_non_empty_string, parse_args_object, parse_date_str,
    parse_pagination, parse_uuid_str, required_non_empty_string,
};
use crate::{McpDispatcher, McpToolHandler};

pub(crate) fn register(dispatcher: &mut McpDispatcher, client: &Arc<DigestApiClient>) {
    dispatcher.register(Arc::new(GetDailyTool {
        client: Arc::clone(client),
    }));
    dispatcher.register(Arc::new(ListDatesTool {
        client: Arc::clone(client),
    }));
    dispatcher.register(Arc::new(ListSourcesTool {
        client: Arc::clone(client),
    }));
    dispatcher.register(Arc::new(GetSourceDigestsTool {
        client: Arc::clone(client),
    }));
    dispatcher.register(Arc::new(SearchItemsTool {
        client: Arc::clone(client),
    }));
    dispatcher.register(Arc::new(GetBookmarksTool {
        client: Arc::clone(client),
    }));
    dispatcher.register(Arc::new(GetRunsStatsTool {
        client: Arc::clone(client),
    }));
}

struct GetDailyTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for GetDailyTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestGetDaily
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let format =
            optional_non_empty_string(&map, "format")?.unwrap_or_else(|| "json".to_owned());
        if !matches!(format.as_str(), "json" | "markdown") {
            return Err(CoreError::InvalidInput(
                "format must be 'json' or 'markdown'".to_owned(),
            ));
        }

        let resolved_date = match optional_non_empty_string(&map, "date")? {
            Some(raw) => parse_date_str(&raw, "date")?,
            None => {
                let latest = self
                    .client
                    .get_json(
                        &api_path("/digests"),
                        &[("page", "1".to_owned()), ("pageSize", "1".to_owned())],
                    )
                    .await?;
                let latest_date = latest
                    .get("items")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(|item| item.get("digestDate"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| CoreError::NotFound("no daily digests available".to_owned()))?;
                parse_date_str(latest_date, "digestDate")?
            }
        };

        if format == "markdown" {
            let markdown = self
                .client
                .get_text(&api_path(&format!("/digests/{resolved_date}/markdown")))
                .await?;
            return Ok(sanitize_mcp_output(json!({
                "digestDate": resolved_date,
                "format": "markdown",
                "markdown": markdown,
            })));
        }

        let detail = self
            .client
            .get_json(&api_path(&format!("/digests/{resolved_date}")), &[])
            .await?;
        Ok(sanitize_mcp_output(detail))
    }
}

struct ListDatesTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for ListDatesTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestListDates
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let (page, page_size) = parse_pagination(&map, DEFAULT_PAGE_SIZE)?;
        let payload = self
            .client
            .get_json(
                &api_path("/digests"),
                &[
                    ("page", page.to_string()),
                    ("pageSize", page_size.to_string()),
                ],
            )
            .await?;
        Ok(sanitize_mcp_output(payload))
    }
}

struct ListSourcesTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for ListSourcesTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestListSources
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let _ = parse_args_object(args)?;
        let payload = self
            .client
            .get_json(&api_path("/settings/sources"), &[])
            .await?;
        let wrapped = match payload {
            Value::Array(sources) => json!({ "sources": sources }),
            other => other,
        };
        Ok(sanitize_mcp_output(wrapped))
    }
}

struct GetSourceDigestsTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for GetSourceDigestsTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestGetSourceDigests
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let raw_source_id = required_non_empty_string(&map, "sourceId")?;
        let source_id = parse_uuid_str(&raw_source_id, "sourceId")?;
        let (page, page_size) = parse_pagination(&map, DEFAULT_PAGE_SIZE)?;
        let include_items = optional_bool(&map, "includeItems")?.unwrap_or(true);

        let query = [
            ("page", page.to_string()),
            ("pageSize", page_size.to_string()),
        ];
        let digests_path = api_path(&format!("/sources/{source_id}/digests"));

        if include_items {
            let items_path = api_path(&format!("/sources/{source_id}/items"));
            let (digests, items) = tokio::try_join!(
                self.client.get_json(&digests_path, &query),
                self.client.get_json(&items_path, &query)
            )?;
            Ok(sanitize_mcp_output(json!({
                "sourceId": source_id,
                "digests": digests,
                "items": items,
            })))
        } else {
            let digests = self.client.get_json(&digests_path, &query).await?;
            Ok(sanitize_mcp_output(json!({
                "sourceId": source_id,
                "digests": digests,
            })))
        }
    }
}

struct SearchItemsTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for SearchItemsTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestSearchItems
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let (page, page_size) = parse_pagination(&map, DEFAULT_PAGE_SIZE)?;

        let mut query: Vec<(&str, String)> = vec![
            ("page", page.to_string()),
            ("pageSize", page_size.to_string()),
        ];

        if let Some(q) = optional_non_empty_string(&map, "q")? {
            query.push(("q", q));
        }
        if let Some(raw_sid) = optional_non_empty_string(&map, "sourceId")? {
            let sid = parse_uuid_str(&raw_sid, "sourceId")?;
            query.push(("sourceId", sid.to_string()));
        }

        let date_from = optional_non_empty_string(&map, "dateFrom")?
            .map(|raw| parse_date_str(&raw, "dateFrom"))
            .transpose()?;
        let date_to = optional_non_empty_string(&map, "dateTo")?
            .map(|raw| parse_date_str(&raw, "dateTo"))
            .transpose()?;

        if let (Some(from_str), Some(to_str)) = (date_from.as_deref(), date_to.as_deref()) {
            let from_d = NaiveDate::parse_from_str(from_str, "%Y-%m-%d")
                .map_err(|_| CoreError::InvalidInput("invalid dateFrom".to_owned()))?;
            let to_d = NaiveDate::parse_from_str(to_str, "%Y-%m-%d")
                .map_err(|_| CoreError::InvalidInput("invalid dateTo".to_owned()))?;
            if from_d > to_d {
                return Err(CoreError::InvalidInput(
                    "dateFrom must be <= dateTo".to_owned(),
                ));
            }
        }

        if let Some(from) = date_from {
            query.push(("dateFrom", from));
        }
        if let Some(to) = date_to {
            query.push(("dateTo", to));
        }
        if let Some(keyword) = optional_non_empty_string(&map, "keyword")? {
            query.push(("keyword", keyword));
        }
        if let Some(bookmarked) = optional_bool(&map, "bookmarked")? {
            query.push(("bookmarked", bookmarked.to_string()));
        }

        let payload = self.client.get_json(&api_path("/items"), &query).await?;
        Ok(sanitize_mcp_output(payload))
    }
}

struct GetBookmarksTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for GetBookmarksTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestGetBookmarks
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let (page, page_size) = parse_pagination(&map, DEFAULT_PAGE_SIZE)?;
        let payload = self
            .client
            .get_json(
                &api_path("/items"),
                &[
                    ("bookmarked", "true".to_owned()),
                    ("page", page.to_string()),
                    ("pageSize", page_size.to_string()),
                ],
            )
            .await?;
        Ok(sanitize_mcp_output(payload))
    }
}

struct GetRunsStatsTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for GetRunsStatsTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestGetRunsStats
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let (page, page_size) = parse_pagination(&map, DEFAULT_PAGE_SIZE)?;
        let runs_query = [
            ("page", page.to_string()),
            ("pageSize", page_size.to_string()),
        ];

        let stats_path = api_path("/runs/stats");
        let runs_path = api_path("/runs");
        let (stats, recent_runs) = tokio::try_join!(
            self.client.get_json(&stats_path, &[]),
            self.client.get_json(&runs_path, &runs_query)
        )?;

        Ok(sanitize_mcp_output(json!({
            "stats": stats,
            "recentRuns": recent_runs,
        })))
    }
}
