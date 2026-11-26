//! WeCom (Enterprise WeChat) group bot webhook notifier.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::constants::{
    CONTENT_TYPE_JSON, DEFAULT_MAX_FEED_BYTES, DEFAULT_WEBHOOK_TIMEOUT_SECS, HEADER_CONTENT_TYPE,
    SECRET_MASK_REDACTED_PLACEHOLDER, WECOM_MAX_MARKDOWN_CHARS,
};
use crate::domain::NotificationChannelType;
use crate::error::CoreError;
use crate::http::read_response_bytes_capped;
use crate::notifiers::{map_notifier_reqwest_error, truncate_chars};
use crate::ssrf::SsrfGuard;
use crate::traits::{Notifier, RenderedNotification};

/// Typed WeCom bot webhook configuration with redacted `Debug`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeComChannelConfig {
    #[serde(alias = "webhook_url")]
    pub webhook_url: String,
}

impl fmt::Debug for WeComChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeComChannelConfig")
            .field("webhook_url", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .finish()
    }
}

/// Dispatches digest notifications to WeCom group bot webhooks (`msgtype: "markdown"`).
#[derive(Debug, Clone)]
pub struct WeComNotifier {
    http_client: reqwest::Client,
    ssrf_guard: SsrfGuard,
    timeout: Duration,
}

impl WeComNotifier {
    /// Creates a `WeComNotifier` with default timeout.
    #[must_use]
    pub fn new(http_client: reqwest::Client, ssrf_guard: SsrfGuard) -> Self {
        Self::with_timeout(
            http_client,
            ssrf_guard,
            Duration::from_secs(DEFAULT_WEBHOOK_TIMEOUT_SECS),
        )
    }

    /// Creates a `WeComNotifier` with an explicit timeout.
    #[must_use]
    pub fn with_timeout(
        http_client: reqwest::Client,
        ssrf_guard: SsrfGuard,
        timeout: Duration,
    ) -> Self {
        Self {
            http_client,
            ssrf_guard,
            timeout,
        }
    }
}

#[derive(Serialize)]
struct WeComPayload<'a> {
    msgtype: &'a str,
    markdown: WeComMarkdown,
}

#[derive(Serialize)]
struct WeComMarkdown {
    content: String,
}

#[async_trait]
impl Notifier for WeComNotifier {
    fn channel_type(&self) -> NotificationChannelType {
        NotificationChannelType::Wecom
    }

    async fn send(
        &self,
        channel_config_json: &serde_json::Value,
        payload: &RenderedNotification,
    ) -> Result<(), CoreError> {
        let cfg: WeComChannelConfig =
            serde_json::from_value(channel_config_json.clone()).map_err(|err| {
                CoreError::InvalidConfig(format!("invalid WeCom channel config: {err}"))
            })?;

        let validated_url = self.ssrf_guard.validate_url(cfg.webhook_url.trim()).await?;

        let body = WeComPayload {
            msgtype: "markdown",
            markdown: WeComMarkdown {
                content: truncate_chars(payload.markdown_body.trim(), WECOM_MAX_MARKDOWN_CHARS),
            },
        };

        let response = self
            .http_client
            .post(validated_url)
            .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .map_err(|err| map_notifier_reqwest_error("WeCom", err))?;

        let status = response.status();
        let resp_bytes = read_response_bytes_capped(response, DEFAULT_MAX_FEED_BYTES).await?;
        if !status.is_success() {
            return Err(CoreError::Upstream(format!(
                "WeCom webhook returned HTTP {}",
                status.as_u16()
            )));
        }

        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&resp_bytes) {
            if let Some(errcode) = json_val.get("errcode").and_then(serde_json::Value::as_i64) {
                if errcode != 0 {
                    let errmsg = json_val
                        .get("errmsg")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown WeCom webhook error");
                    return Err(CoreError::Upstream(format!(
                        "WeCom webhook returned errcode {errcode}: {errmsg}"
                    )));
                }
            }
        }

        Ok(())
    }
}
