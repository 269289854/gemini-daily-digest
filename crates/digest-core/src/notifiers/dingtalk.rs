//! DingTalk custom robot webhook notifier with optional HMAC-SHA256 query signing.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::constants::{
    CONTENT_TYPE_JSON, DEFAULT_MAX_FEED_BYTES, DEFAULT_WEBHOOK_TIMEOUT_SECS,
    DINGTALK_MAX_MARKDOWN_CHARS, HEADER_CONTENT_TYPE, SECRET_MASK_REDACTED_PLACEHOLDER,
};
use crate::domain::NotificationChannelType;
use crate::error::CoreError;
use crate::http::read_response_bytes_capped;
use crate::notifiers::{map_notifier_reqwest_error, truncate_chars};
use crate::ssrf::SsrfGuard;
use crate::traits::{Notifier, RenderedNotification};

type HmacSha256 = Hmac<Sha256>;

/// Typed DingTalk bot webhook configuration with redacted `Debug`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DingTalkChannelConfig {
    #[serde(alias = "webhook_url")]
    pub webhook_url: String,
    #[serde(default)]
    pub secret: Option<String>,
}

impl fmt::Debug for DingTalkChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let masked_secret = self
            .secret
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("DingTalkChannelConfig")
            .field("webhook_url", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .field("secret", &masked_secret)
            .finish()
    }
}

/// Dispatches digest notifications to DingTalk custom robot webhooks.
#[derive(Debug, Clone)]
pub struct DingTalkNotifier {
    http_client: reqwest::Client,
    ssrf_guard: SsrfGuard,
    timeout: Duration,
}

impl DingTalkNotifier {
    /// Creates a `DingTalkNotifier` with default timeout.
    #[must_use]
    pub fn new(http_client: reqwest::Client, ssrf_guard: SsrfGuard) -> Self {
        Self::with_timeout(
            http_client,
            ssrf_guard,
            Duration::from_secs(DEFAULT_WEBHOOK_TIMEOUT_SECS),
        )
    }

    /// Creates a `DingTalkNotifier` with an explicit timeout.
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

    /// Computes DingTalk's HMAC-SHA256 signature: `base64(hmac_sha256(secret, "{timestamp_ms}\n{secret}"))`.
    pub fn compute_signature(timestamp_millis: i64, secret: &str) -> Result<String, CoreError> {
        let string_to_sign = format!("{timestamp_millis}\n{secret}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| CoreError::Crypto("failed to initialize DingTalk HMAC key".to_owned()))?;
        mac.update(string_to_sign.as_bytes());
        Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }
}

#[derive(Serialize)]
struct DingTalkPayload<'a> {
    msgtype: &'a str,
    markdown: DingTalkMarkdown<'a>,
}

#[derive(Serialize)]
struct DingTalkMarkdown<'a> {
    title: &'a str,
    text: String,
}

#[async_trait]
impl Notifier for DingTalkNotifier {
    fn channel_type(&self) -> NotificationChannelType {
        NotificationChannelType::Dingtalk
    }

    async fn send(
        &self,
        channel_config_json: &serde_json::Value,
        payload: &RenderedNotification,
    ) -> Result<(), CoreError> {
        let cfg: DingTalkChannelConfig = serde_json::from_value(channel_config_json.clone())
            .map_err(|err| {
                CoreError::InvalidConfig(format!("invalid DingTalk channel config: {err}"))
            })?;

        let mut validated_url = self.ssrf_guard.validate_url(cfg.webhook_url.trim()).await?;

        if let Some(secret) = cfg
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let ts_millis = Utc::now().timestamp_millis();
            let sign = Self::compute_signature(ts_millis, secret)?;
            validated_url
                .query_pairs_mut()
                .append_pair("timestamp", &ts_millis.to_string())
                .append_pair("sign", &sign);
        }

        let body = DingTalkPayload {
            msgtype: "markdown",
            markdown: DingTalkMarkdown {
                title: payload.subject.trim(),
                text: truncate_chars(payload.markdown_body.trim(), DINGTALK_MAX_MARKDOWN_CHARS),
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
            .map_err(|err| map_notifier_reqwest_error("DingTalk", err))?;

        let status = response.status();
        let resp_bytes = read_response_bytes_capped(response, DEFAULT_MAX_FEED_BYTES).await?;
        if !status.is_success() {
            return Err(CoreError::Upstream(format!(
                "DingTalk webhook returned HTTP {}",
                status.as_u16()
            )));
        }

        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&resp_bytes) {
            if let Some(errcode) = json_val.get("errcode").and_then(serde_json::Value::as_i64) {
                if errcode != 0 {
                    let errmsg = json_val
                        .get("errmsg")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown DingTalk webhook error");
                    return Err(CoreError::Upstream(format!(
                        "DingTalk webhook returned errcode {errcode}: {errmsg}"
                    )));
                }
            }
        }

        Ok(())
    }
}
