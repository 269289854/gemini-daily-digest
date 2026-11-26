//! Outbound notification channel implementations (`smtp`, `telegram`, `feishu`,
//! `dingtalk`, `wecom`, `webhook`) and `NotifierRegistry`.

pub mod dingtalk;
pub mod feishu;
pub mod smtp;
pub mod telegram;
pub mod webhook;
pub mod wecom;

use std::collections::HashMap;
use std::sync::Arc;

pub use dingtalk::DingTalkNotifier;
pub use feishu::FeishuNotifier;
pub use smtp::SmtpNotifier;
pub use telegram::TelegramNotifier;
pub use webhook::WebhookNotifier;
pub use wecom::WeComNotifier;

use crate::config::{AppConfig, NotifierTuning};
use crate::domain::{NotificationChannelType, NotifierConfig};
use crate::error::CoreError;
use crate::http::shared_http_client;
use crate::ssrf::SsrfGuard;
use crate::traits::{Notifier, RenderedNotification};

/// Strategy registry mapping each `NotificationChannelType` to its `Notifier` implementation.
#[derive(Clone)]
pub struct NotifierRegistry {
    notifiers: HashMap<NotificationChannelType, Arc<dyn Notifier>>,
}

impl NotifierRegistry {
    /// Registers all 6 built-in notification channel implementations.
    #[must_use]
    pub fn with_defaults(
        http_client: reqwest::Client,
        ssrf_guard: SsrfGuard,
        tuning: NotifierTuning,
    ) -> Self {
        let mut notifiers: HashMap<NotificationChannelType, Arc<dyn Notifier>> = HashMap::new();
        notifiers.insert(
            NotificationChannelType::Smtp,
            Arc::new(SmtpNotifier::with_ssrf_guard(
                tuning.smtp_timeout,
                ssrf_guard.clone(),
            )),
        );
        notifiers.insert(
            NotificationChannelType::Telegram,
            Arc::new(TelegramNotifier::with_options(
                http_client.clone(),
                ssrf_guard.clone(),
                tuning.webhook_timeout,
                tuning.telegram_api_base_url,
            )),
        );
        notifiers.insert(
            NotificationChannelType::Feishu,
            Arc::new(FeishuNotifier::with_timeout(
                http_client.clone(),
                ssrf_guard.clone(),
                tuning.webhook_timeout,
            )),
        );
        notifiers.insert(
            NotificationChannelType::Dingtalk,
            Arc::new(DingTalkNotifier::with_timeout(
                http_client.clone(),
                ssrf_guard.clone(),
                tuning.webhook_timeout,
            )),
        );
        notifiers.insert(
            NotificationChannelType::Wecom,
            Arc::new(WeComNotifier::with_timeout(
                http_client.clone(),
                ssrf_guard.clone(),
                tuning.webhook_timeout,
            )),
        );
        notifiers.insert(
            NotificationChannelType::Webhook,
            Arc::new(WebhookNotifier::with_timeout(
                http_client,
                ssrf_guard,
                tuning.webhook_timeout,
            )),
        );
        Self { notifiers }
    }

    /// Constructs a `NotifierRegistry` from `AppConfig`.
    pub fn from_config(config: &AppConfig) -> Result<Self, CoreError> {
        let client = shared_http_client(config.outbound_proxy_url.as_deref(), &config.collector)?;
        let guard = SsrfGuard::from_config(config);
        Ok(Self::with_defaults(client, guard, config.notifier.clone()))
    }

    /// Looks up the `Notifier` for `channel_type`.
    #[must_use]
    pub fn get(&self, channel_type: NotificationChannelType) -> Option<Arc<dyn Notifier>> {
        self.notifiers.get(&channel_type).cloned()
    }

    /// Dispatches a rendered notification through the matching channel strategy.
    pub async fn send(
        &self,
        channel: &NotifierConfig,
        payload: &RenderedNotification,
    ) -> Result<(), CoreError> {
        let notifier = self.get(channel.channel_type).ok_or_else(|| {
            CoreError::InvalidConfig(format!(
                "no notifier registered for channel type '{}'",
                channel.channel_type
            ))
        })?;
        notifier.send(&channel.config_json, payload).await
    }
}

pub(crate) fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_owned();
    }
    let truncated: String = input.chars().take(max_chars).collect();
    format!("{truncated}...")
}

pub(crate) fn map_notifier_reqwest_error(channel: &str, err: reqwest::Error) -> CoreError {
    if err.is_timeout() {
        CoreError::UpstreamTimeout(format!("{channel} notification timed out"))
    } else {
        let kind = if err.is_connect() {
            "connection error"
        } else if err.is_request() {
            "request error"
        } else {
            "transport error"
        };
        CoreError::Upstream(format!("{channel} notification failed ({kind})"))
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_json, header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::NotifierTuning;
    use crate::constants::HEADER_WEBHOOK_SIGNATURE;
    use crate::domain::{AlertPriority, NotificationChannelType};
    use crate::ssrf::SsrfGuard;
    use crate::traits::RenderedNotification;

    fn sample_payload() -> RenderedNotification {
        RenderedNotification {
            subject: "Gemini Daily Digest — 2026-11-26".to_owned(),
            text_body: "Plain text body".to_owned(),
            html_body: "<p>HTML body</p>".to_owned(),
            markdown_body: "# Digest\n- Item 1".to_owned(),
            priority: AlertPriority::High,
        }
    }

    #[tokio::test]
    async fn registry_covers_all_six_channels_and_dispatches_telegram() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/bottest-key-placeholder/sendMessage"))
            .and(body_json(serde_json::json!({
                "chat_id": "123456",
                "text": "Gemini Daily Digest — 2026-11-26\n\nPlain text body",
                "disable_web_page_preview": true
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": { "message_id": 42 }
            })))
            .mount(&server)
            .await;

        let registry = NotifierRegistry::with_defaults(
            reqwest::Client::new(),
            SsrfGuard::new(vec!["127.0.0.1".to_owned()]),
            NotifierTuning::default(),
        );
        for channel in NotificationChannelType::all() {
            assert!(registry.get(*channel).is_some());
        }

        let cfg = NotifierConfig {
            channel_id: None,
            channel_type: NotificationChannelType::Telegram,
            name: "tg".to_owned(),
            config_json: serde_json::json!({
                "bot_token": "test-key-placeholder",
                "chat_id": "123456",
                "api_base_url": server.uri()
            }),
        };
        registry.send(&cfg, &sample_payload()).await.unwrap();
    }

    #[tokio::test]
    async fn feishu_and_dingtalk_compute_hmac_signatures_and_post() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/feishu"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "code": 0 })),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/dingtalk"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "errcode": 0 })),
            )
            .mount(&server)
            .await;

        let guard = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let client = reqwest::Client::new();
        let payload = sample_payload();

        let feishu = FeishuNotifier::new(client.clone(), guard.clone());
        feishu
            .send(
                &serde_json::json!({
                    "webhook_url": format!("{}/feishu", server.uri()),
                    "secret": "test-key-placeholder"
                }),
                &payload,
            )
            .await
            .unwrap();

        let dingtalk = DingTalkNotifier::new(client, guard);
        dingtalk
            .send(
                &serde_json::json!({
                    "webhook_url": format!("{}/dingtalk", server.uri()),
                    "secret": "test-key-placeholder"
                }),
                &payload,
            )
            .await
            .unwrap();

        let recorded = server.received_requests().await.unwrap();
        assert_eq!(recorded.len(), 2);

        let feishu_body: serde_json::Value = serde_json::from_slice(&recorded[0].body).unwrap();
        let feishu_ts: i64 = feishu_body["timestamp"].as_str().unwrap().parse().unwrap();
        let expected_feishu_sig =
            FeishuNotifier::compute_signature(feishu_ts, "test-key-placeholder").unwrap();
        assert_eq!(feishu_body["sign"].as_str().unwrap(), expected_feishu_sig);

        let dt_pairs: std::collections::HashMap<String, String> =
            recorded[1].url.query_pairs().into_owned().collect();
        let dt_ts: i64 = dt_pairs.get("timestamp").unwrap().parse().unwrap();
        let expected_dt_sig =
            DingTalkNotifier::compute_signature(dt_ts, "test-key-placeholder").unwrap();
        assert_eq!(dt_pairs.get("sign").unwrap(), &expected_dt_sig);
    }

    #[tokio::test]
    async fn wecom_and_webhook_send_expected_payloads_and_hmac_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/wecom"))
            .and(body_json(serde_json::json!({
                "msgtype": "markdown",
                "markdown": { "content": "# Digest\n- Item 1" }
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "errcode": 0 })),
            )
            .mount(&server)
            .await;

        let payload = sample_payload();
        let raw_bytes = serde_json::to_vec(&payload).unwrap();
        let expected_hex =
            WebhookNotifier::compute_signature_hex("test-key-placeholder", &raw_bytes).unwrap();

        Mock::given(method("POST"))
            .and(path("/hook"))
            .and(header(HEADER_WEBHOOK_SIGNATURE, expected_hex.as_str()))
            .and(header("x-custom-test", "ok"))
            .and(header_exists("content-type"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;

        let guard = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let client = reqwest::Client::new();

        WeComNotifier::new(client.clone(), guard.clone())
            .send(
                &serde_json::json!({ "webhook_url": format!("{}/wecom", server.uri()) }),
                &payload,
            )
            .await
            .unwrap();

        WebhookNotifier::new(client, guard)
            .send(
                &serde_json::json!({
                    "url": format!("{}/hook", server.uri()),
                    "secret": "test-key-placeholder",
                    "headers": { "x-custom-test": "ok" }
                }),
                &payload,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn all_http_notifiers_block_private_urls_via_ssrf_guard() {
        let guard = SsrfGuard::new(Vec::new());
        let client = reqwest::Client::new();
        let payload = sample_payload();

        let tg = TelegramNotifier::new(client.clone(), guard.clone())
            .send(
                &serde_json::json!({
                    "bot_token": "test-key-placeholder",
                    "chat_id": "1",
                    "api_base_url": "http://127.0.0.1:8080"
                }),
                &payload,
            )
            .await;
        assert!(matches!(tg, Err(CoreError::SsrfBlocked(_))));

        let feishu = FeishuNotifier::new(client.clone(), guard.clone())
            .send(
                &serde_json::json!({ "webhook_url": "http://169.254.169.254/hook" }),
                &payload,
            )
            .await;
        assert!(matches!(feishu, Err(CoreError::SsrfBlocked(_))));

        let dingtalk = DingTalkNotifier::new(client.clone(), guard.clone())
            .send(
                &serde_json::json!({ "webhook_url": "http://10.0.0.5/hook" }),
                &payload,
            )
            .await;
        assert!(matches!(dingtalk, Err(CoreError::SsrfBlocked(_))));

        let wecom = WeComNotifier::new(client.clone(), guard.clone())
            .send(
                &serde_json::json!({ "webhook_url": "http://192.168.1.10/hook" }),
                &payload,
            )
            .await;
        assert!(matches!(wecom, Err(CoreError::SsrfBlocked(_))));

        let webhook = WebhookNotifier::new(client, guard)
            .send(
                &serde_json::json!({ "url": "http://127.0.0.1:9000/hook" }),
                &payload,
            )
            .await;
        assert!(matches!(webhook, Err(CoreError::SsrfBlocked(_))));
    }

    #[tokio::test]
    async fn smtp_validates_config_builds_multipart_and_redacts_secrets_in_errors() {
        let valid_json = serde_json::json!({
            "host": "127.0.0.1",
            "port": 587,
            "username": "mailer@example.com",
            "password": "test-key-placeholder-smtp-secret",
            "from": "Digest <digest@example.com>",
            "to": ["alice@example.com", "bob@example.com"],
            "useStarttls": true
        });
        let cfg = SmtpNotifier::parse_config(&valid_json).unwrap();
        let debug_str = format!("{cfg:?}");
        assert!(!debug_str.contains("test-key-placeholder-smtp-secret"));
        assert!(!debug_str.contains("mailer@example.com"));

        let message = SmtpNotifier::build_message(&cfg, &sample_payload()).unwrap();
        let raw_mime = String::from_utf8_lossy(&message.formatted()).to_string();
        assert!(raw_mime.contains("Plain text body"));
        assert!(raw_mime.contains("<p>HTML body</p>"));

        // Invalid config checks
        let missing_to = SmtpNotifier::parse_config(&serde_json::json!({
            "host": "smtp.example.com",
            "port": 587,
            "from": "digest@example.com",
            "to": []
        }));
        assert!(matches!(missing_to, Err(CoreError::InvalidConfig(_))));

        // SSRF check on 127.0.0.1 must fail without leaking password
        let err = SmtpNotifier::default()
            .send(&valid_json, &sample_payload())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::SsrfBlocked(_)));
        assert!(!err.to_string().contains("test-key-placeholder-smtp-secret"));

        // Unbracketed IPv6 loopback host must return SsrfBlocked, not InvalidInput
        let ipv6_json = serde_json::json!({
            "host": "::1",
            "port": 587,
            "from": "digest@example.com",
            "to": ["alice@example.com"]
        });
        let ipv6_err = SmtpNotifier::default()
            .send(&ipv6_json, &sample_payload())
            .await
            .unwrap_err();
        assert!(
            matches!(ipv6_err, CoreError::SsrfBlocked(_)),
            "expected SsrfBlocked for unbracketed ::1, got {ipv6_err:?}"
        );
    }
}
