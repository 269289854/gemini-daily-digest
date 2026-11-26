//! Desktop navigation validation and `DesktopViewController` callback SPI.
//!
//! Isolates standalone `stdio` execution (`NoopDesktopViewController`) from
//! Tauri 2 desktop window navigation (`apps/desktop/src-tauri`).

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::NaiveDate;
use digest_core::error::CoreError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

const ALLOWED_VIEWS: &[&str] = &["daily", "sources", "history", "runs", "settings"];
pub const ALLOWED_SETTINGS_TABS: &[&str] = &["ai", "sources", "channels", "schedule", "mcp"];
const MAX_TAB_LEN: usize = 32;
const MAX_QUERY_LEN: usize = 512;

/// Validated desktop navigation request targeting one of the 5 React views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNavigationTarget {
    pub view: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}

impl DesktopNavigationTarget {
    /// Parses, normalizes, and strictly validates all fields to prevent route/XSS injection.
    pub fn validate(&mut self) -> Result<(), CoreError> {
        let view_trimmed = self.view.trim();
        if view_trimmed.is_empty()
            || has_unsafe_route_chars(view_trimmed)
            || !ALLOWED_VIEWS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(view_trimmed))
        {
            return Err(CoreError::InvalidInput(format!(
                "invalid desktop view '{}'; expected one of: {}",
                self.view,
                ALLOWED_VIEWS.join(", ")
            )));
        }
        self.view = view_trimmed.to_ascii_lowercase();

        if let Some(raw_date) = self.date.as_deref() {
            let trimmed = raw_date.trim();
            if trimmed.is_empty() {
                self.date = None;
            } else {
                if trimmed.len() != 10 || NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").is_err() {
                    return Err(CoreError::InvalidInput(
                        "date must be in strict YYYY-MM-DD format".to_owned(),
                    ));
                }
                self.date = Some(trimmed.to_owned());
            }
        }

        if let Some(raw_q) = self.q.as_deref() {
            let trimmed = raw_q.trim();
            if trimmed.is_empty() {
                self.q = None;
            } else {
                if trimmed.chars().count() > MAX_QUERY_LEN || trimmed.chars().any(char::is_control)
                {
                    return Err(CoreError::InvalidInput(
                        "search query q exceeds limit or contains control characters".to_owned(),
                    ));
                }
                self.q = Some(trimmed.to_owned());
            }
        }

        if let Some(raw_tab) = self.tab.as_deref() {
            let trimmed = raw_tab.trim();
            if trimmed.is_empty() {
                self.tab = None;
            } else {
                if self.view != "settings" {
                    return Err(CoreError::InvalidInput(
                        "tab is only valid when view is 'settings'".to_owned(),
                    ));
                }
                if !is_safe_tab_identifier(trimmed)
                    || !ALLOWED_SETTINGS_TABS
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(trimmed))
                {
                    return Err(CoreError::InvalidInput(format!(
                        "invalid settings tab '{}'; expected one of: {}",
                        trimmed,
                        ALLOWED_SETTINGS_TABS.join(", ")
                    )));
                }
                self.tab = Some(trimmed.to_ascii_lowercase());
            }
        }

        Ok(())
    }

    /// Parses and validates a `DesktopNavigationTarget` from an untyped JSON object.
    pub fn from_json(args: Value) -> Result<Self, CoreError> {
        let mut target: Self = serde_json::from_value(args).map_err(|err| {
            CoreError::InvalidInput(format!("invalid desktop navigation arguments: {err}"))
        })?;
        target.validate()?;
        Ok(target)
    }

    /// Returns the canonical React app route path (e.g. `"/daily"`).
    #[must_use]
    pub fn route_path(&self) -> &'static str {
        match self.view.as_str() {
            "sources" => "/sources",
            "history" => "/history",
            "runs" => "/runs",
            "settings" => "/settings",
            _ => "/daily",
        }
    }

    /// Converts optional parameters into string query params matching `App.tsx`.
    #[must_use]
    pub fn to_query_params(&self) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        if let Some(date) = &self.date {
            params.insert("date".to_owned(), date.clone());
        }
        if let Some(source_id) = self.source_id {
            params.insert("sourceId".to_owned(), source_id.to_string());
        }
        if let Some(q) = &self.q {
            params.insert("q".to_owned(), q.clone());
        }
        if let Some(bookmarked) = self.bookmarked {
            params.insert("bookmarked".to_owned(), bookmarked.to_string());
        }
        if let Some(tab) = &self.tab {
            params.insert("tab".to_owned(), tab.clone());
        }
        params
    }
}

fn has_unsafe_route_chars(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    raw.contains("..")
        || raw.contains('/')
        || raw.contains('\\')
        || raw.contains('?')
        || raw.contains('#')
        || raw.contains(':')
        || lower.contains("http:")
        || lower.contains("https:")
        || lower.contains("javascript:")
        || raw.chars().any(char::is_control)
}

fn is_safe_tab_identifier(raw: &str) -> bool {
    let len = raw.len();
    (1..=MAX_TAB_LEN).contains(&len)
        && !has_unsafe_route_chars(raw)
        && raw
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Strategy trait allowing `apps/desktop/src-tauri` to focus and navigate the WebView.
#[async_trait]
pub trait DesktopViewController: Send + Sync {
    /// Focuses the desktop window and navigates to the validated target route.
    async fn open_view(&self, target: DesktopNavigationTarget) -> Result<Value, CoreError>;
}

/// Default headless controller used in standalone `stdio` mode when no desktop UI is attached.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopDesktopViewController;

#[async_trait]
impl DesktopViewController for NoopDesktopViewController {
    async fn open_view(&self, mut target: DesktopNavigationTarget) -> Result<Value, CoreError> {
        target.validate()?;
        Ok(json!({
            "opened": false,
            "supported": false,
            "route": target.route_path(),
            "params": target.to_query_params(),
            "target": target,
            "reason": "Desktop window is not attached in standalone mode"
        }))
    }
}

#[cfg(test)]
mod tests {
    use digest_core::domain::ErrorCode;

    use super::*;

    #[test]
    fn desktop_navigation_target_validates_allowlist_and_blocks_injections() {
        for tab in ["ai", "sources", "channels", "schedule", "mcp", "MCP"] {
            let valid = DesktopNavigationTarget::from_json(json!({
                "view": "settings",
                "tab": tab
            }))
            .unwrap();
            assert_eq!(valid.route_path(), "/settings");
            assert_eq!(
                valid.to_query_params().get("tab").unwrap(),
                &tab.to_ascii_lowercase()
            );
        }

        let empty_tab_on_daily = DesktopNavigationTarget::from_json(json!({
            "view": "daily",
            "tab": "   "
        }))
        .unwrap();
        assert!(empty_tab_on_daily.tab.is_none());

        let bad_views = [
            json!({ "view": "javascript:alert(1)" }),
            json!({ "view": "../settings" }),
            json!({ "view": "daily?evil=1" }),
            json!({ "view": "unknown" }),
            json!({ "view": "https://evil.example.com" }),
        ];
        for payload in bad_views {
            let err = DesktopNavigationTarget::from_json(payload).unwrap_err();
            assert_eq!(err.error_code(), ErrorCode::InvalidInput);
        }

        let bad_tabs = [
            json!({ "view": "settings", "tab": "unknown_tab" }),
            json!({ "view": "settings", "tab": "mcp_config-1" }),
            json!({ "view": "daily", "tab": "mcp" }),
            json!({ "view": "sources", "tab": "ai" }),
            json!({ "view": "settings", "tab": "../ai" }),
            json!({ "view": "settings", "tab": "ai?x=1" }),
            json!({ "view": "settings", "tab": "ai#frag" }),
            json!({ "view": "settings", "tab": "javascript:alert(1)" }),
            json!({ "view": "settings", "tab": "a/b" }),
            json!({ "view": "settings", "tab": "a".repeat(33) }),
        ];
        for payload in bad_tabs {
            let err = DesktopNavigationTarget::from_json(payload).unwrap_err();
            assert_eq!(err.error_code(), ErrorCode::InvalidInput);
        }

        assert!(DesktopNavigationTarget::from_json(json!({
            "view": "daily",
            "date": "2026-1-5"
        }))
        .is_err());
    }
}
