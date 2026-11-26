//! MCP tool registry, JSON Schema descriptors, and shared argument validators.

pub mod read_tools;
pub mod write_tools;

use std::str::FromStr;
use std::sync::Arc;

use chrono::NaiveDate;
use digest_core::constants::{API_PREFIX, DEFAULT_MAX_PAGE_SIZE, DEFAULT_PAGE_SIZE};
use digest_core::domain::{AlertPriority, McpAccessMode, McpToolName, SourceScope, SourceType};
use digest_core::error::CoreError;
use serde::Serialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::client::DigestApiClient;
use crate::desktop::{DesktopViewController, ALLOWED_SETTINGS_TABS};
use crate::McpDispatcher;

/// MCP tool behavior annotations (`readOnlyHint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolAnnotations {
    pub read_only_hint: bool,
}

/// Standard MCP `tools/list` tool descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    pub name: McpToolName,
    pub description: &'static str,
    pub input_schema: Value,
    pub annotations: McpToolAnnotations,
}

/// Extended tool descriptor exposed on `/control/tools` including required access mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlToolDescriptor {
    pub name: McpToolName,
    pub description: &'static str,
    pub required_access: McpAccessMode,
    pub input_schema: Value,
    pub annotations: McpToolAnnotations,
}

/// Registers all 14 `McpToolName` handlers onto `dispatcher`.
pub fn register_all_tools(
    dispatcher: &mut McpDispatcher,
    client: Arc<DigestApiClient>,
    desktop: Arc<dyn DesktopViewController>,
) {
    read_tools::register(dispatcher, &client);
    write_tools::register(dispatcher, client, desktop);
}

/// Convenience constructor returning a fully populated `McpDispatcher`.
#[must_use]
pub fn build_default_dispatcher(
    client: Arc<DigestApiClient>,
    desktop: Arc<dyn DesktopViewController>,
) -> McpDispatcher {
    let mut dispatcher = McpDispatcher::default();
    register_all_tools(&mut dispatcher, client, desktop);
    dispatcher
}

/// Returns standard MCP tool descriptors for all 14 tools.
#[must_use]
pub fn all_tool_descriptors() -> Vec<McpToolDescriptor> {
    all_control_tool_descriptors()
        .into_iter()
        .map(|item| McpToolDescriptor {
            name: item.name,
            description: item.description,
            input_schema: item.input_schema,
            annotations: item.annotations,
        })
        .collect()
}

/// Returns tool descriptors with `requiredAccess` metadata for `/control/tools`.
#[must_use]
pub fn all_control_tool_descriptors() -> Vec<ControlToolDescriptor> {
    vec![
        descriptor(
            McpToolName::DigestGetDaily,
            McpAccessMode::ReadOnly,
            "Fetch full daily digest for a given YYYY-MM-DD date (or latest digest if date is omitted).",
            json!({
                "type": "object",
                "properties": {
                    "date": { "type": "string", "description": "Optional digest date in YYYY-MM-DD format." },
                    "format": { "type": "string", "enum": ["json", "markdown"], "default": "json" }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestListDates,
            McpAccessMode::ReadOnly,
            "List available daily digest dates with summary counts.",
            json!({
                "type": "object",
                "properties": {
                    "page": { "type": "integer", "minimum": 1, "default": 1 },
                    "pageSize": { "type": "integer", "minimum": 1, "maximum": DEFAULT_MAX_PAGE_SIZE, "default": DEFAULT_PAGE_SIZE }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestListSources,
            McpAccessMode::ReadOnly,
            "List subscribed canonical sources/accounts and their fetch health status (credentials redacted).",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestGetSourceDigests,
            McpAccessMode::ReadOnly,
            "Get independent daily summaries and raw collected items for a subscribed source/account.",
            json!({
                "type": "object",
                "required": ["sourceId"],
                "properties": {
                    "sourceId": { "type": "string", "format": "uuid" },
                    "page": { "type": "integer", "minimum": 1, "default": 1 },
                    "pageSize": { "type": "integer", "minimum": 1, "maximum": DEFAULT_MAX_PAGE_SIZE, "default": DEFAULT_PAGE_SIZE },
                    "includeItems": { "type": "boolean", "default": true }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestSearchItems,
            McpAccessMode::ReadOnly,
            "Full-text and metadata search across collected items.",
            json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string" },
                    "sourceId": { "type": "string", "format": "uuid" },
                    "dateFrom": { "type": "string", "description": "Start date (YYYY-MM-DD)." },
                    "dateTo": { "type": "string", "description": "End date (YYYY-MM-DD)." },
                    "keyword": { "type": "string" },
                    "bookmarked": { "type": "boolean" },
                    "page": { "type": "integer", "minimum": 1, "default": 1 },
                    "pageSize": { "type": "integer", "minimum": 1, "maximum": DEFAULT_MAX_PAGE_SIZE, "default": DEFAULT_PAGE_SIZE }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestGetBookmarks,
            McpAccessMode::ReadOnly,
            "List bookmarked items and user notes.",
            json!({
                "type": "object",
                "properties": {
                    "page": { "type": "integer", "minimum": 1, "default": 1 },
                    "pageSize": { "type": "integer", "minimum": 1, "maximum": DEFAULT_MAX_PAGE_SIZE, "default": DEFAULT_PAGE_SIZE }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestGetRunsStats,
            McpAccessMode::ReadOnly,
            "Get execution run history, source health stats, delivery metrics, and token consumption.",
            json!({
                "type": "object",
                "properties": {
                    "page": { "type": "integer", "minimum": 1, "default": 1 },
                    "pageSize": { "type": "integer", "minimum": 1, "maximum": DEFAULT_MAX_PAGE_SIZE, "default": DEFAULT_PAGE_SIZE }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestTriggerRun,
            McpAccessMode::ReadWrite,
            "Trigger an immediate fetch + AI summarization (+ optional notification) run.",
            json!({
                "type": "object",
                "properties": {
                    "digestDate": { "type": "string", "description": "Optional target date (YYYY-MM-DD)." },
                    "forceRefresh": { "type": "boolean", "default": false },
                    "notify": { "type": "boolean", "default": false }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestAddSource,
            McpAccessMode::ReadWrite,
            "Subscribe to a canonical source/account (credentials are forbidden via MCP).",
            json!({
                "type": "object",
                "required": ["sourceType", "handleOrUrl"],
                "properties": {
                    "sourceType": {
                        "type": "string",
                        "enum": SourceType::all().iter().map(|v| v.as_str()).collect::<Vec<_>>()
                    },
                    "handleOrUrl": { "type": "string", "minLength": 1 },
                    "scope": {
                        "type": "string",
                        "enum": SourceScope::all().iter().map(|v| v.as_str()).collect::<Vec<_>>()
                    },
                    "canonicalKey": { "type": "string" },
                    "customName": { "type": "string" },
                    "defaultName": { "type": "string" },
                    "category": { "type": "string" },
                    "enabled": { "type": "boolean", "default": true }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestToggleSource,
            McpAccessMode::ReadWrite,
            "Enable or pause a subscribed source/account.",
            json!({
                "type": "object",
                "required": ["sourceId", "enabled"],
                "properties": {
                    "sourceId": { "type": "string", "format": "uuid" },
                    "enabled": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestBookmarkItem,
            McpAccessMode::ReadWrite,
            "Bookmark an item with optional note, or remove an existing bookmark.",
            json!({
                "type": "object",
                "required": ["itemId"],
                "properties": {
                    "itemId": { "type": "string", "format": "uuid" },
                    "bookmarked": { "type": "boolean", "default": true },
                    "note": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestUpdateSchedule,
            McpAccessMode::ReadWrite,
            "Update daily digest schedule time, timezone, email switch, or alert scan interval.",
            json!({
                "type": "object",
                "properties": {
                    "timezone": { "type": "string" },
                    "dailyTime": { "type": "string", "description": "24-hour HH:MM time." },
                    "emailEnabled": { "type": "boolean" },
                    "sendWhenEmpty": { "type": "boolean" },
                    "alertScanEnabled": { "type": "boolean" },
                    "alertIntervalMinutes": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestAddWatchlistRule,
            McpAccessMode::ReadWrite,
            "Create a keyword highlight and immediate alert watchlist rule.",
            json!({
                "type": "object",
                "required": ["name", "keywords"],
                "properties": {
                    "name": { "type": "string", "minLength": 1 },
                    "keywords": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "type": "string", "minLength": 1 }
                    },
                    "excludeKeywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "default": []
                    },
                    "priority": {
                        "type": "string",
                        "enum": AlertPriority::all().iter().map(|v| v.as_str()).collect::<Vec<_>>(),
                        "default": AlertPriority::Normal.as_str()
                    },
                    "notifyImmediately": { "type": "boolean", "default": false },
                    "enabled": { "type": "boolean", "default": true }
                },
                "additionalProperties": false
            }),
        ),
        descriptor(
            McpToolName::DigestOpenDesktopView,
            McpAccessMode::ReadWrite,
            "Focus the Desktop window and navigate to a specific digest, source, search, runs, or settings view.",
            json!({
                "type": "object",
                "required": ["view"],
                "properties": {
                    "view": {
                        "type": "string",
                        "enum": ["daily", "sources", "history", "runs", "settings"]
                    },
                    "date": { "type": "string", "description": "Optional YYYY-MM-DD date for daily view." },
                    "sourceId": { "type": "string", "format": "uuid" },
                    "q": { "type": "string" },
                    "bookmarked": { "type": "boolean" },
                    "tab": { "type": "string", "enum": ALLOWED_SETTINGS_TABS }
                },
                "additionalProperties": false
            }),
        ),
    ]
}

fn descriptor(
    name: McpToolName,
    required_access: McpAccessMode,
    description: &'static str,
    input_schema: Value,
) -> ControlToolDescriptor {
    ControlToolDescriptor {
        name,
        description,
        required_access,
        input_schema,
        annotations: McpToolAnnotations {
            read_only_hint: required_access == McpAccessMode::ReadOnly,
        },
    }
}

pub(crate) fn api_path(subroute: &str) -> String {
    format!("{API_PREFIX}{subroute}")
}

pub(crate) fn parse_args_object(args: Value) -> Result<Map<String, Value>, CoreError> {
    match args {
        Value::Null => Ok(Map::new()),
        Value::Object(map) => Ok(map),
        _ => Err(CoreError::InvalidInput(
            "MCP tool arguments must be a JSON object".to_owned(),
        )),
    }
}

pub(crate) fn optional_non_empty_string(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, CoreError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(_) => Err(CoreError::InvalidInput(format!("'{key}' must be a string"))),
    }
}

pub(crate) fn required_non_empty_string(
    map: &Map<String, Value>,
    key: &str,
) -> Result<String, CoreError> {
    optional_non_empty_string(map, key)?.ok_or_else(|| {
        CoreError::InvalidInput(format!("'{key}' is required and must not be empty"))
    })
}

pub(crate) fn optional_bool(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, CoreError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(val)) => Ok(Some(*val)),
        Some(_) => Err(CoreError::InvalidInput(format!(
            "'{key}' must be a boolean"
        ))),
    }
}

pub(crate) fn required_bool(map: &Map<String, Value>, key: &str) -> Result<bool, CoreError> {
    optional_bool(map, key)?.ok_or_else(|| CoreError::InvalidInput(format!("'{key}' is required")))
}

pub(crate) fn parse_date_str(raw: &str, field: &str) -> Result<String, CoreError> {
    let trimmed = raw.trim();
    if trimmed.len() != 10 || NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").is_err() {
        return Err(CoreError::InvalidInput(format!(
            "{field} must be in strict YYYY-MM-DD format"
        )));
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn parse_uuid_str(raw: &str, field: &str) -> Result<Uuid, CoreError> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| CoreError::InvalidInput(format!("invalid UUID for {field}")))
}

pub(crate) fn parse_pagination(
    map: &Map<String, Value>,
    default_page_size: usize,
) -> Result<(usize, usize), CoreError> {
    let page = match map.get("page") {
        None | Some(Value::Null) => 1,
        Some(Value::Number(num)) => {
            let val = num.as_u64().ok_or_else(|| {
                CoreError::InvalidInput("'page' must be a positive integer".to_owned())
            })?;
            let parsed = usize::try_from(val)
                .map_err(|_| CoreError::InvalidInput("'page' is out of range".to_owned()))?;
            if parsed == 0 {
                return Err(CoreError::InvalidInput("'page' must be >= 1".to_owned()));
            }
            parsed
        }
        Some(_) => {
            return Err(CoreError::InvalidInput(
                "'page' must be a positive integer".to_owned(),
            ));
        }
    };

    let page_size = match map.get("pageSize") {
        None | Some(Value::Null) => default_page_size.clamp(1, DEFAULT_MAX_PAGE_SIZE),
        Some(Value::Number(num)) => {
            let val = num.as_u64().ok_or_else(|| {
                CoreError::InvalidInput("'pageSize' must be a positive integer".to_owned())
            })?;
            let parsed = usize::try_from(val)
                .map_err(|_| CoreError::InvalidInput("'pageSize' is out of range".to_owned()))?;
            if parsed == 0 {
                return Err(CoreError::InvalidInput(
                    "'pageSize' must be >= 1".to_owned(),
                ));
            }
            parsed.min(DEFAULT_MAX_PAGE_SIZE)
        }
        Some(_) => {
            return Err(CoreError::InvalidInput(
                "'pageSize' must be a positive integer".to_owned(),
            ));
        }
    };

    Ok((page, page_size))
}

pub(crate) fn parse_enum_str<T>(raw: &str) -> Result<T, CoreError>
where
    T: FromStr<Err = CoreError>,
{
    T::from_str(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_desktop_view_schema_shares_allowed_settings_tabs() {
        let descriptors = all_tool_descriptors();
        let desktop_tool = descriptors
            .into_iter()
            .find(|tool| tool.name == McpToolName::DigestOpenDesktopView)
            .expect("DigestOpenDesktopView descriptor must exist");
        let tab_enum = desktop_tool.input_schema["properties"]["tab"]["enum"]
            .as_array()
            .expect("tab schema must specify enum");
        let actual: Vec<&str> = tab_enum.iter().filter_map(Value::as_str).collect();
        assert_eq!(actual.as_slice(), ALLOWED_SETTINGS_TABS);
    }
}
