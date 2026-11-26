//! Generic HTTPS webhook notifier with SSRF validation, custom headers, and HMAC-SHA256 signing.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use sha2::Sha256;

use crate::constants::{
    CONTENT_TYPE_JSON, DEFAULT_MAX_FEED_BYTES, DEFAULT_WEBHOOK_TIMEOUT_SECS, HEADER_CONTENT_TYPE,
    HEADER_WEBHOOK_SIGNATURE, SECRET_MASK_REDACTED_PLACEHOLDER,
};
use crate::domain::NotificationChannelType;
use crate::error::CoreError;
use crate::http::read_response_bytes_capped;
use crate::notifiers::map_notifier_reqwest_error;
use crate::ssrf::SsrfGuard;
use crate::traits::{Notifier, RenderedNotification};

type HmacSha256 = Hmac<Sha256>;

/// Typed generic webhook configuration with redacted `Debug`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookChannelConfig {
    pub url: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl fmt::Debug for WebhookChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let masked_secret = self
            .secret
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("WebhookChannelConfig")
            .field("url", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .field("secret", &masked_secret)
            .field("header_count", &self.headers.len())
            .finish()
    }
}

/// Dispatches rendered JSON digest payloads to custom webhooks with optional HMAC-SHA256 signature.
#[derive(Debug, Clone)]
pub struct WebhookNotifier {
    http_client: reqwest::Client,
    ssrf_guard: SsrfGuard,
    timeout: Duration,
}

impl WebhookNotifier {
    /// Creates a `WebhookNotifier` with default timeout.
    #[must_use]
    pub fn new(http_client: reqwest::Client, ssrf_guard: SsrfGuard) -> Self {
        Self::with_timeout(
            http_client,
            ssrf_guard,
            Duration::from_secs(DEFAULT_WEBHOOK_TIMEOUT_SECS),
        )
    }

    /// Creates a `WebhookNotifier` with an explicit timeout.
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

    /// Computes lowercase hex HMAC-SHA256 over the serialized webhook payload bytes.
    pub fn compute_signature_hex(secret: &str, body_bytes: &[u8]) -> Result<String, CoreError> {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| CoreError::Crypto("failed to initialize webhook HMAC key".to_owned()))?;
        mac.update(body_bytes);
        Ok(hex::encode(mac.finalize().into_bytes()))
    }
}

#[async_trait]
impl Notifier for WebhookNotifier {
    fn channel_type(&self) -> NotificationChannelType {
        NotificationChannelType::Webhook
    }

    async fn send(
        &self,
        channel_config_json: &serde_json::Value,
        payload: &RenderedNotification,
    ) -> Result<(), CoreError> {
        let cfg: WebhookChannelConfig = serde_json::from_value(channel_config_json.clone())
            .map_err(|err| {
                CoreError::InvalidConfig(format!("invalid webhook channel config: {err}"))
            })?;

        let validated_url = self.ssrf_guard.validate_url(cfg.url.trim()).await?;

        let body_bytes = serde_json::to_vec(payload).map_err(|err| {
            CoreError::Internal(format!("failed to serialize webhook payload: {err}"))
        })?;

        let mut headers = HeaderMap::new();
        for (raw_name, raw_value) in &cfg.headers {
            let name = HeaderName::from_bytes(raw_name.trim().as_bytes()).map_err(|err| {
                CoreError::InvalidInput(format!("invalid webhook header name '{raw_name}': {err}"))
            })?;
            let lower = name.as_str();
            if lower == HEADER_CONTENT_TYPE || lower == HEADER_WEBHOOK_SIGNATURE {
                return Err(CoreError::InvalidInput(format!(
                    "webhook header '{lower}' is managed automatically"
                )));
            }
            let value = HeaderValue::from_str(raw_value.trim()).map_err(|err| {
                CoreError::InvalidInput(format!(
                    "invalid webhook header value for '{lower}': {err}"
                ))
            })?;
            headers.insert(name, value);
        }

        if let Some(secret) = cfg
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let sig_hex = Self::compute_signature_hex(secret, &body_bytes)?;
            let sig_name = HeaderName::from_static(HEADER_WEBHOOK_SIGNATURE);
            let sig_val = HeaderValue::from_str(&sig_hex).map_err(|err| {
                CoreError::Internal(format!("invalid webhook signature header value: {err}"))
            })?;
            headers.insert(sig_name, sig_val);
        }

        let response = self
            .http_client
            .post(validated_url)
            .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
            .headers(headers)
            .timeout(self.timeout)
            .body(body_bytes)
            .send()
            .await
            .map_err(|err| map_notifier_reqwest_error("Webhook", err))?;

        let status = response.status();
        let _ = read_response_bytes_capped(response, DEFAULT_MAX_FEED_BYTES).await?;
        if !status.is_success() {
            return Err(CoreError::Upstream(format!(
                "Webhook endpoint returned HTTP {}",
                status.as_u16()
            )));
        }

        Ok(())
    }
}
