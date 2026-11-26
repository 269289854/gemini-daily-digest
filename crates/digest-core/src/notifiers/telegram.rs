//! Telegram Bot API notifier (`sendMessage`) with SSRF validation and secret redaction.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::constants::{
    CONTENT_TYPE_JSON, DEFAULT_MAX_FEED_BYTES, DEFAULT_TELEGRAM_API_BASE_URL,
    DEFAULT_WEBHOOK_TIMEOUT_SECS, HEADER_CONTENT_TYPE, SECRET_MASK_REDACTED_PLACEHOLDER,
    TELEGRAM_MAX_TEXT_CHARS,
};
use crate::domain::NotificationChannelType;
use crate::error::CoreError;
use crate::http::read_response_bytes_capped;
use crate::notifiers::{map_notifier_reqwest_error, truncate_chars};
use crate::ssrf::SsrfGuard;
use crate::traits::{Notifier, RenderedNotification};

/// Typed Telegram notification channel config with redacted `Debug`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramChannelConfig {
    #[serde(alias = "bot_token")]
    pub bot_token: String,
    #[serde(alias = "chat_id")]
    pub chat_id: String,
    #[serde(default, alias = "api_base_url")]
    pub api_base_url: Option<String>,
}

impl fmt::Debug for TelegramChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramChannelConfig")
            .field("bot_token", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .field("chat_id", &self.chat_id)
            .field("api_base_url", &self.api_base_url)
            .finish()
    }
}

/// Dispatches digest notifications to Telegram chats via Bot API `sendMessage`.
#[derive(Debug, Clone)]
pub struct TelegramNotifier {
    http_client: reqwest::Client,
    ssrf_guard: SsrfGuard,
    timeout: Duration,
    default_api_base_url: String,
}

impl TelegramNotifier {
    /// Creates a `TelegramNotifier` using default `DEFAULT_TELEGRAM_API_BASE_URL` and timeout.
    #[must_use]
    pub fn new(http_client: reqwest::Client, ssrf_guard: SsrfGuard) -> Self {
        Self::with_options(
            http_client,
            ssrf_guard,
            Duration::from_secs(DEFAULT_WEBHOOK_TIMEOUT_SECS),
            DEFAULT_TELEGRAM_API_BASE_URL.to_owned(),
        )
    }

    /// Creates a `TelegramNotifier` with explicit timeout and default Bot API base URL.
    #[must_use]
    pub fn with_options(
        http_client: reqwest::Client,
        ssrf_guard: SsrfGuard,
        timeout: Duration,
        default_api_base_url: String,
    ) -> Self {
        Self {
            http_client,
            ssrf_guard,
            timeout,
            default_api_base_url,
        }
    }
}

#[derive(Serialize)]
struct TelegramSendMessageRequest<'a> {
    chat_id: &'a str,
    text: String,
    disable_web_page_preview: bool,
}

#[async_trait]
impl Notifier for TelegramNotifier {
    fn channel_type(&self) -> NotificationChannelType {
        NotificationChannelType::Telegram
    }

    async fn send(
        &self,
        channel_config_json: &serde_json::Value,
        payload: &RenderedNotification,
    ) -> Result<(), CoreError> {
        let cfg: TelegramChannelConfig = serde_json::from_value(channel_config_json.clone())
            .map_err(|err| {
                CoreError::InvalidConfig(format!("invalid Telegram channel config: {err}"))
            })?;

        let bot_token = cfg.bot_token.trim();
        let chat_id = cfg.chat_id.trim();
        if bot_token.is_empty() || chat_id.is_empty() {
            return Err(CoreError::InvalidConfig(
                "Telegram bot_token and chat_id are required".to_owned(),
            ));
        }

        let base = cfg
            .api_base_url
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(self.default_api_base_url.as_str())
            .trim_end_matches('/');

        let raw_url = format!("{base}/bot{bot_token}/sendMessage");
        let validated_url = self.ssrf_guard.validate_url(&raw_url).await?;

        let combined_text = format!("{}\n\n{}", payload.subject.trim(), payload.text_body.trim());
        let body = TelegramSendMessageRequest {
            chat_id,
            text: truncate_chars(&combined_text, TELEGRAM_MAX_TEXT_CHARS),
            disable_web_page_preview: true,
        };

        let response = self
            .http_client
            .post(validated_url)
            .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .map_err(|err| map_notifier_reqwest_error("Telegram", err))?;

        let status = response.status();
        let resp_bytes = read_response_bytes_capped(response, DEFAULT_MAX_FEED_BYTES).await?;
        if !status.is_success() {
            return Err(CoreError::Upstream(format!(
                "Telegram API returned HTTP {}",
                status.as_u16()
            )));
        }

        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&resp_bytes) {
            if json_val.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
                let desc = json_val
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown Telegram API error");
                return Err(CoreError::Upstream(format!(
                    "Telegram API rejected message: {desc}"
                )));
            }
        }

        Ok(())
    }
}
