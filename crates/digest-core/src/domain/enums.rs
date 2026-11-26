//! Strongly typed enumerations enforcing zero magic strings across
//! collectors, storage, API endpoints, MCP tools, and the React UI.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

macro_rules! define_string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident, serde_rename = $rename:literal {
            $(
                $(#[$vmeta:meta])*
                $variant:ident => $str_val:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = $rename)]
        $vis enum $name {
            $(
                $(#[$vmeta])*
                $variant,
            )+
        }

        impl $name {
            /// Returns the canonical wire representation.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $str_val,)+
                }
            }

            /// Returns all declared variants in definition order.
            #[must_use]
            pub const fn all() -> &'static [Self] {
                &[$(Self::$variant,)+]
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = CoreError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let normalized = s.trim();
                $(
                    if normalized.eq_ignore_ascii_case($str_val) {
                        return Ok(Self::$variant);
                    }
                )+
                Err(CoreError::InvalidEnumValue {
                    enum_name: stringify!($name),
                    value: s.to_owned(),
                })
            }
        }
    };
}

define_string_enum! {
    /// Supported upstream collector source kinds.
    pub enum SourceType, serde_rename = "snake_case" {
        Rss => "rss",
        GithubRelease => "github_release",
        Reddit => "reddit",
        HackerNews => "hacker_news",
        GoogleNews => "google_news",
        XAccount => "x_account",
        XSearch => "x_search",
    }
}

define_string_enum! {
    /// Isolation scope of a canonical source in the shared pool.
    pub enum SourceScope, serde_rename = "snake_case" {
        SharedPublic => "shared_public",
        UserPrivate => "user_private",
    }
}

define_string_enum! {
    /// Result status of a conditional upstream fetch.
    pub enum FetchOutcome, serde_rename = "snake_case" {
        Updated => "updated",
        NotModified304 => "not_modified_304",
        SkippedFresh => "skipped_fresh",
        Failed => "failed",
    }
}

define_string_enum! {
    /// Supported outbound notification channel kinds.
    pub enum NotificationChannelType, serde_rename = "snake_case" {
        Smtp => "smtp",
        Telegram => "telegram",
        Feishu => "feishu",
        Dingtalk => "dingtalk",
        Wecom => "wecom",
        Webhook => "webhook",
    }
}

define_string_enum! {
    /// Trigger reason for a digest/alert execution run.
    pub enum RunType, serde_rename = "snake_case" {
        ScheduledDaily => "scheduled_daily",
        ManualDaily => "manual_daily",
        AlertScan => "alert_scan",
    }
}

define_string_enum! {
    /// Lifecycle status of an execution run.
    pub enum RunStatus, serde_rename = "snake_case" {
        Running => "running",
        Succeeded => "succeeded",
        Partial => "partial",
        Failed => "failed",
    }
}

define_string_enum! {
    /// Delivery status of a single channel dispatch.
    pub enum DeliveryStatus, serde_rename = "snake_case" {
        Pending => "pending",
        Sent => "sent",
        Failed => "failed",
    }
}

define_string_enum! {
    /// Priority assigned to watchlist keyword rules and alerts.
    pub enum AlertPriority, serde_rename = "snake_case" {
        Normal => "normal",
        High => "high",
        Urgent => "urgent",
    }
}

define_string_enum! {
    /// Permission level granted to external AI clients via MCP.
    pub enum McpAccessMode, serde_rename = "snake_case" {
        Disabled => "disabled",
        ReadOnly => "read_only",
        ReadWrite => "read_write",
    }
}

impl McpAccessMode {
    /// Checks whether a caller with `self` mode satisfies `required` capability.
    #[must_use]
    pub const fn allows(self, required: Self) -> bool {
        matches!(
            (self, required),
            (Self::ReadWrite, _)
                | (Self::ReadOnly, Self::ReadOnly | Self::Disabled)
                | (Self::Disabled, Self::Disabled)
        )
    }
}

define_string_enum! {
    /// Server authentication strategy.
    pub enum AuthMode, serde_rename = "snake_case" {
        Oidc => "oidc",
        DevBypass => "dev_bypass",
    }
}

define_string_enum! {
    /// Real-time Server-Sent Events emitted on `/api/v1/events/stream`.
    pub enum SseEventType, serde_rename = "snake_case" {
        RunProgress => "run_progress",
        DigestReady => "digest_ready",
        AlertTriggered => "alert_triggered",
        Heartbeat => "heartbeat",
    }
}

define_string_enum! {
    /// Canonical machine-readable API and MCP error codes.
    pub enum ErrorCode, serde_rename = "SCREAMING_SNAKE_CASE" {
        InvalidInput => "INVALID_INPUT",
        Unauthorized => "UNAUTHORIZED",
        Forbidden => "FORBIDDEN",
        NotFound => "NOT_FOUND",
        Conflict => "CONFLICT",
        SsrfBlocked => "SSRF_BLOCKED",
        CryptoError => "CRYPTO_ERROR",
        UpstreamTimeout => "UPSTREAM_TIMEOUT",
        UpstreamError => "UPSTREAM_ERROR",
        DatabaseError => "DATABASE_ERROR",
        InternalError => "INTERNAL_ERROR",
    }
}

define_string_enum! {
    /// Canonical MCP tool names exposed to external AI agents.
    pub enum McpToolName, serde_rename = "snake_case" {
        DigestGetDaily => "digest_get_daily",
        DigestListDates => "digest_list_dates",
        DigestListSources => "digest_list_sources",
        DigestGetSourceDigests => "digest_get_source_digests",
        DigestSearchItems => "digest_search_items",
        DigestGetBookmarks => "digest_get_bookmarks",
        DigestGetRunsStats => "digest_get_runs_stats",
        DigestTriggerRun => "digest_trigger_run",
        DigestAddSource => "digest_add_source",
        DigestToggleSource => "digest_toggle_source",
        DigestBookmarkItem => "digest_bookmark_item",
        DigestUpdateSchedule => "digest_update_schedule",
        DigestAddWatchlistRule => "digest_add_watchlist_rule",
        DigestOpenDesktopView => "digest_open_desktop_view",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_enum_roundtrip<T>(variants: &[T])
    where
        T: Copy + Eq + fmt::Debug + fmt::Display + FromStr<Err = CoreError> + Serialize,
        for<'de> T: Deserialize<'de>,
    {
        for &variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            let decoded: T = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, variant);

            let parsed_exact = T::from_str(&variant.to_string()).unwrap();
            assert_eq!(parsed_exact, variant);

            let parsed_upper = T::from_str(&variant.to_string().to_ascii_uppercase()).unwrap();
            assert_eq!(parsed_upper, variant);
        }
        assert!(T::from_str("definitely_not_a_valid_variant").is_err());
    }

    #[test]
    fn all_domain_enums_roundtrip_and_reject_unknown_values() {
        assert_enum_roundtrip(SourceType::all());
        assert_enum_roundtrip(SourceScope::all());
        assert_enum_roundtrip(FetchOutcome::all());
        assert_enum_roundtrip(NotificationChannelType::all());
        assert_enum_roundtrip(RunType::all());
        assert_enum_roundtrip(RunStatus::all());
        assert_enum_roundtrip(DeliveryStatus::all());
        assert_enum_roundtrip(AlertPriority::all());
        assert_enum_roundtrip(McpAccessMode::all());
        assert_enum_roundtrip(AuthMode::all());
        assert_enum_roundtrip(SseEventType::all());
        assert_enum_roundtrip(ErrorCode::all());
        assert_enum_roundtrip(McpToolName::all());
    }

    #[test]
    fn mcp_access_mode_lattice_is_enforced() {
        assert!(!McpAccessMode::Disabled.allows(McpAccessMode::ReadOnly));
        assert!(!McpAccessMode::Disabled.allows(McpAccessMode::ReadWrite));
        assert!(McpAccessMode::ReadOnly.allows(McpAccessMode::ReadOnly));
        assert!(!McpAccessMode::ReadOnly.allows(McpAccessMode::ReadWrite));
        assert!(McpAccessMode::ReadWrite.allows(McpAccessMode::ReadOnly));
        assert!(McpAccessMode::ReadWrite.allows(McpAccessMode::ReadWrite));
    }
}
