//! Read-write MCP tool handlers (`McpAccessMode::ReadWrite`).

use std::sync::Arc;

use async_trait::async_trait;
use digest_core::domain::{AlertPriority, McpAccessMode, McpToolName, SourceScope, SourceType};
use digest_core::error::CoreError;
use serde_json::{json, Map, Value};

use crate::client::DigestApiClient;
use crate::desktop::{DesktopNavigationTarget, DesktopViewController};
use crate::redact::sanitize_mcp_output;
use crate::tools::{
    api_path, optional_bool, optional_non_empty_string, parse_args_object, parse_date_str,
    parse_enum_str, parse_uuid_str, required_bool, required_non_empty_string,
};
use crate::{McpDispatcher, McpToolHandler};

pub(crate) fn register(
    dispatcher: &mut McpDispatcher,
    client: Arc<DigestApiClient>,
    desktop: Arc<dyn DesktopViewController>,
) {
    dispatcher.register(Arc::new(TriggerRunTool {
        client: Arc::clone(&client),
    }));
    dispatcher.register(Arc::new(AddSourceTool {
        client: Arc::clone(&client),
    }));
    dispatcher.register(Arc::new(ToggleSourceTool {
        client: Arc::clone(&client),
    }));
    dispatcher.register(Arc::new(BookmarkItemTool {
        client: Arc::clone(&client),
    }));
    dispatcher.register(Arc::new(UpdateScheduleTool {
        client: Arc::clone(&client),
    }));
    dispatcher.register(Arc::new(AddWatchlistRuleTool { client }));
    dispatcher.register(Arc::new(OpenDesktopViewTool { desktop }));
}

struct TriggerRunTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for TriggerRunTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestTriggerRun
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadWrite
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let digest_date = optional_non_empty_string(&map, "digestDate")?
            .map(|raw| parse_date_str(&raw, "digestDate"))
            .transpose()?;
        let force_refresh = optional_bool(&map, "forceRefresh")?.unwrap_or(false);
        let notify = optional_bool(&map, "notify")?.unwrap_or(false);

        let mut body = Map::new();
        if let Some(date) = digest_date {
            body.insert("digestDate".to_owned(), Value::String(date));
        }
        body.insert("forceRefresh".to_owned(), Value::Bool(force_refresh));
        body.insert("notify".to_owned(), Value::Bool(notify));

        let payload = self
            .client
            .post_json(&api_path("/runs/trigger"), Some(&Value::Object(body)))
            .await?;
        Ok(sanitize_mcp_output(payload))
    }
}

struct AddSourceTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for AddSourceTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestAddSource
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadWrite
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        if map.contains_key("credentials") {
            return Err(CoreError::InvalidInput(
                "MCP tools cannot accept or manage source credentials".to_owned(),
            ));
        }

        let source_type_raw = required_non_empty_string(&map, "sourceType")?;
        let source_type: SourceType = parse_enum_str(&source_type_raw)?;
        let handle_or_url = required_non_empty_string(&map, "handleOrUrl")?;
        let enabled = optional_bool(&map, "enabled")?.unwrap_or(true);

        let mut body = Map::new();
        body.insert(
            "sourceType".to_owned(),
            Value::String(source_type.as_str().to_owned()),
        );
        body.insert("handleOrUrl".to_owned(), Value::String(handle_or_url));
        body.insert("enabled".to_owned(), Value::Bool(enabled));

        if let Some(scope_raw) = optional_non_empty_string(&map, "scope")? {
            let scope: SourceScope = parse_enum_str(&scope_raw)?;
            body.insert("scope".to_owned(), Value::String(scope.as_str().to_owned()));
        }
        if let Some(canonical_key) = optional_non_empty_string(&map, "canonicalKey")? {
            body.insert("canonicalKey".to_owned(), Value::String(canonical_key));
        }
        if let Some(custom_name) = optional_non_empty_string(&map, "customName")? {
            body.insert("customName".to_owned(), Value::String(custom_name));
        }
        if let Some(default_name) = optional_non_empty_string(&map, "defaultName")? {
            body.insert("defaultName".to_owned(), Value::String(default_name));
        }
        if let Some(category) = optional_non_empty_string(&map, "category")? {
            body.insert("category".to_owned(), Value::String(category));
        }

        let payload = self
            .client
            .post_json(&api_path("/settings/sources"), Some(&Value::Object(body)))
            .await?;
        Ok(sanitize_mcp_output(payload))
    }
}

struct ToggleSourceTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for ToggleSourceTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestToggleSource
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadWrite
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let raw_source_id = required_non_empty_string(&map, "sourceId")?;
        let source_id = parse_uuid_str(&raw_source_id, "sourceId")?;
        let enabled = required_bool(&map, "enabled")?;

        let body = json!({ "enabled": enabled });
        let payload = self
            .client
            .put_json(&api_path(&format!("/settings/sources/{source_id}")), &body)
            .await?;
        Ok(sanitize_mcp_output(payload))
    }
}

struct BookmarkItemTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for BookmarkItemTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestBookmarkItem
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadWrite
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let raw_item_id = required_non_empty_string(&map, "itemId")?;
        let item_id = parse_uuid_str(&raw_item_id, "itemId")?;
        let bookmarked = optional_bool(&map, "bookmarked")?.unwrap_or(true);
        let note = optional_non_empty_string(&map, "note")?;

        if bookmarked {
            let mut body = Map::new();
            body.insert("itemId".to_owned(), Value::String(item_id.to_string()));
            if let Some(note_text) = note {
                body.insert("note".to_owned(), Value::String(note_text));
            }
            let payload = self
                .client
                .post_json(&api_path("/bookmarks"), Some(&Value::Object(body)))
                .await?;
            Ok(sanitize_mcp_output(payload))
        } else {
            self.client
                .delete_empty(&api_path(&format!("/bookmarks/{item_id}")))
                .await?;
            Ok(json!({
                "itemId": item_id,
                "bookmarked": false,
            }))
        }
    }
}

struct UpdateScheduleTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for UpdateScheduleTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestUpdateSchedule
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadWrite
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;

        let current = self
            .client
            .get_json(&api_path("/settings/schedule"), &[])
            .await?;

        let cur_timezone = current
            .get("timezone")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::Upstream("schedule response missing timezone".to_owned()))?;
        let cur_daily_time = current
            .get("dailyTime")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::Upstream("schedule response missing dailyTime".to_owned()))?;
        let cur_email_enabled = current
            .get("emailEnabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                CoreError::Upstream("schedule response missing emailEnabled".to_owned())
            })?;
        let cur_send_when_empty = current
            .get("sendWhenEmpty")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                CoreError::Upstream("schedule response missing sendWhenEmpty".to_owned())
            })?;
        let cur_alert_scan_enabled = current
            .get("alertScanEnabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                CoreError::Upstream("schedule response missing alertScanEnabled".to_owned())
            })?;
        let cur_alert_interval = current
            .get("alertIntervalMinutes")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                CoreError::Upstream("schedule response missing alertIntervalMinutes".to_owned())
            })?;

        let timezone =
            optional_non_empty_string(&map, "timezone")?.unwrap_or_else(|| cur_timezone.to_owned());
        let daily_time = optional_non_empty_string(&map, "dailyTime")?
            .unwrap_or_else(|| cur_daily_time.to_owned());
        validate_daily_time(&daily_time)?;

        let email_enabled = optional_bool(&map, "emailEnabled")?.unwrap_or(cur_email_enabled);
        let send_when_empty = optional_bool(&map, "sendWhenEmpty")?.unwrap_or(cur_send_when_empty);
        let alert_scan_enabled =
            optional_bool(&map, "alertScanEnabled")?.unwrap_or(cur_alert_scan_enabled);

        let alert_interval_minutes = match map.get("alertIntervalMinutes") {
            None | Some(Value::Null) => cur_alert_interval,
            Some(Value::Number(n)) => {
                let val = n.as_i64().ok_or_else(|| {
                    CoreError::InvalidInput("alertIntervalMinutes must be an integer".to_owned())
                })?;
                if val <= 0 {
                    return Err(CoreError::InvalidInput(
                        "alertIntervalMinutes must be >= 1".to_owned(),
                    ));
                }
                val
            }
            Some(_) => {
                return Err(CoreError::InvalidInput(
                    "alertIntervalMinutes must be an integer".to_owned(),
                ));
            }
        };

        let merged_body = json!({
            "timezone": timezone,
            "dailyTime": daily_time,
            "emailEnabled": email_enabled,
            "sendWhenEmpty": send_when_empty,
            "alertScanEnabled": alert_scan_enabled,
            "alertIntervalMinutes": alert_interval_minutes,
        });

        let payload = self
            .client
            .put_json(&api_path("/settings/schedule"), &merged_body)
            .await?;
        Ok(sanitize_mcp_output(payload))
    }
}

fn validate_daily_time(raw: &str) -> Result<(), CoreError> {
    let trimmed = raw.trim();
    let (hh, mm) = trimmed
        .split_once(':')
        .ok_or_else(|| CoreError::InvalidInput("dailyTime must be in HH:MM format".to_owned()))?;
    if hh.len() != 2 || mm.len() != 2 {
        return Err(CoreError::InvalidInput(
            "dailyTime must be in HH:MM format".to_owned(),
        ));
    }
    let hour: u8 = hh
        .parse()
        .map_err(|_| CoreError::InvalidInput("dailyTime hour must be numeric".to_owned()))?;
    let minute: u8 = mm
        .parse()
        .map_err(|_| CoreError::InvalidInput("dailyTime minute must be numeric".to_owned()))?;
    if hour > 23 || minute > 59 {
        return Err(CoreError::InvalidInput(
            "dailyTime must be between 00:00 and 23:59".to_owned(),
        ));
    }
    Ok(())
}

struct AddWatchlistRuleTool {
    client: Arc<DigestApiClient>,
}

#[async_trait]
impl McpToolHandler for AddWatchlistRuleTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestAddWatchlistRule
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadWrite
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let map = parse_args_object(args)?;
        let name = required_non_empty_string(&map, "name")?;

        let keywords = parse_string_array(&map, "keywords", true)?;
        let exclude_keywords = match map.get("excludeKeywords") {
            None | Some(Value::Null) => Vec::new(),
            Some(_) => parse_string_array(&map, "excludeKeywords", false)?,
        };

        let priority = match optional_non_empty_string(&map, "priority")? {
            Some(raw) => parse_enum_str(&raw)?,
            None => AlertPriority::Normal,
        };
        let notify_immediately = optional_bool(&map, "notifyImmediately")?.unwrap_or(false);
        let enabled = optional_bool(&map, "enabled")?.unwrap_or(true);

        let body = json!({
            "name": name,
            "keywords": keywords,
            "excludeKeywords": exclude_keywords,
            "priority": priority,
            "notifyImmediately": notify_immediately,
            "enabled": enabled,
        });

        let payload = self
            .client
            .post_json(&api_path("/settings/watchlist"), Some(&body))
            .await?;
        Ok(sanitize_mcp_output(payload))
    }
}

fn parse_string_array(
    map: &Map<String, Value>,
    key: &str,
    require_non_empty: bool,
) -> Result<Vec<String>, CoreError> {
    let raw = map
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::InvalidInput(format!("'{key}' must be an array of strings")))?;

    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        let s = item
            .as_str()
            .ok_or_else(|| CoreError::InvalidInput(format!("'{key}' elements must be strings")))?;
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_owned());
        }
    }

    if require_non_empty && out.is_empty() {
        return Err(CoreError::InvalidInput(format!(
            "'{key}' must contain at least one non-empty string"
        )));
    }
    Ok(out)
}

struct OpenDesktopViewTool {
    desktop: Arc<dyn DesktopViewController>,
}

#[async_trait]
impl McpToolHandler for OpenDesktopViewTool {
    fn name(&self) -> McpToolName {
        McpToolName::DigestOpenDesktopView
    }

    fn required_access(&self) -> McpAccessMode {
        McpAccessMode::ReadWrite
    }

    async fn execute(&self, args: Value) -> Result<Value, CoreError> {
        let target = DesktopNavigationTarget::from_json(args)?;
        let res = self.desktop.open_view(target).await?;
        Ok(sanitize_mcp_output(res))
    }
}
