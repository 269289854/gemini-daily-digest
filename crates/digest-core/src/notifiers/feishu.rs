//! Feishu / Lark custom bot webhook notifier with optional HMAC-SHA256 signing.

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
    FEISHU_MAX_MARKDOWN_CHARS, HEADER_CONTENT_TYPE, SECRET_MASK_REDACTED_PLACEHOLDER,
};
use crate::domain::NotificationChannelType;
use crate::error::CoreError;
use crate::http::read_response_bytes_capped;
use crate::notifiers::{map_notifier_reqwest_error, truncate_chars};
use crate::ssrf::SsrfGuard;
use crate::traits::{Notifier, RenderedNotification};

type HmacSha256 = Hmac<Sha256>;

/// Typed Feishu bot webhook configuration with redacted `Debug`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuChannelConfig {
    #[serde(alias = "webhook_url")]
    pub webhook_url: String,
    #[serde(default)]
    pub secret: Option<String>,
}

impl fmt::Debug for FeishuChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let masked_secret = self
            .secret
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("FeishuChannelConfig")
            .field("webhook_url", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .field("secret", &masked_secret)
            .finish()
    }
}

/// Dispatches digest notifications to Feishu / Lark custom bot webhooks.
#[derive(Debug, Clone)]
pub struct FeishuNotifier {
    http_client: reqwest::Client,
    ssrf_guard: SsrfGuard,
    timeout: Duration,
}

impl FeishuNotifier {
    /// Creates a `FeishuNotifier` with default timeout.
    #[must_use]
    pub fn new(http_client: reqwest::Client, ssrf_guard: SsrfGuard) -> Self {
        Self::with_timeout(
            http_client,
            ssrf_guard,
            Duration::from_secs(DEFAULT_WEBHOOK_TIMEOUT_SECS),
        )
    }

    /// Creates a `FeishuNotifier` with an explicit timeout.
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

    /// Computes Feishu's HMAC-SHA256 signature: `base64(hmac_sha256(key = "{timestamp}\n{secret}", data = ""))`.
    pub fn compute_signature(timestamp_secs: i64, secret: &str) -> Result<String, CoreError> {
        let string_to_sign = format!("{timestamp_secs}\n{secret}");
        let mut mac = HmacSha256::new_from_slice(string_to_sign.as_bytes())
            .map_err(|_| CoreError::Crypto("failed to initialize Feishu HMAC key".to_owned()))?;
        mac.update(b"");
        Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }
}

#[derive(Serialize)]
struct FeishuWebhookPayload<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sign: Option<String>,
    msg_type: &'a str,
    card: FeishuCard<'a>,
}

#[derive(Serialize)]
struct FeishuCard<'a> {
    header: FeishuCardHeader<'a>,
    elements: Vec<FeishuCardElement>,
}

#[derive(Serialize)]
struct FeishuCardHeader<'a> {
    title: FeishuPlainText<'a>,
}

#[derive(Serialize)]
struct FeishuPlainText<'a> {
    tag: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct FeishuCardElement {
    tag: &'static str,
    content: String,
}

#[async_trait]
impl Notifier for FeishuNotifier {
    fn channel_type(&self) -> NotificationChannelType {
        NotificationChannelType::Feishu
    }

    async fn send(
        &self,
        channel_config_json: &serde_json::Value,
        payload: &RenderedNotification,
    ) -> Result<(), CoreError> {
        let cfg: FeishuChannelConfig = serde_json::from_value(channel_config_json.clone())
            .map_err(|err| {
                CoreError::InvalidConfig(format!("invalid Feishu channel config: {err}"))
            })?;

        let validated_url = self.ssrf_guard.validate_url(cfg.webhook_url.trim()).await?;

        let (timestamp, sign) = match cfg
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(secret) => {
                let ts = Utc::now().timestamp();
                let sig = Self::compute_signature(ts, secret)?;
                (Some(ts.to_string()), Some(sig))
            }
            None => (None, None),
        };

        let body = FeishuWebhookPayload {
            timestamp,
            sign,
            msg_type: "interactive",
            card: FeishuCard {
                header: FeishuCardHeader {
                    title: FeishuPlainText {
                        tag: "plain_text",
                        content: payload.subject.trim(),
                    },
                },
                elements: vec![FeishuCardElement {
                    tag: "markdown",
                    content: truncate_chars(
                        payload.markdown_body.trim(),
                        FEISHU_MAX_MARKDOWN_CHARS,
                    ),
                }],
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
            .map_err(|err| map_notifier_reqwest_error("Feishu", err))?;

        let status = response.status();
        let resp_bytes = read_response_bytes_capped(response, DEFAULT_MAX_FEED_BYTES).await?;
        if !status.is_success() {
            return Err(CoreError::Upstream(format!(
                "Feishu webhook returned HTTP {}",
                status.as_u16()
            )));
        }

        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&resp_bytes) {
            if let Some(code) = json_val.get("code").and_then(serde_json::Value::as_i64) {
                if code != 0 {
                    let msg = json_val
                        .get("msg")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown Feishu webhook error");
                    return Err(CoreError::Upstream(format!(
                        "Feishu webhook returned error code {code}: {msg}"
                    )));
                }
            }
        }

        Ok(())
    }
}
