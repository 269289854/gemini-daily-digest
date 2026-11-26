//! Async SMTP notifier using `lettre` (`Tokio1Executor` + `rustls-tls`) with multipart plain+HTML emails.

use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;

use crate::constants::{
    DEFAULT_SMTP_IMPLICIT_TLS_PORT, DEFAULT_SMTP_PORT, DEFAULT_SMTP_TIMEOUT_SECS,
    SECRET_MASK_REDACTED_PLACEHOLDER,
};
use crate::domain::NotificationChannelType;
use crate::error::CoreError;
use crate::ssrf::SsrfGuard;
use crate::traits::{Notifier, RenderedNotification};

/// Typed SMTP channel configuration with secret-redacting `Debug`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmtpChannelConfig {
    pub host: String,
    #[serde(default = "default_smtp_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    pub from: String,
    #[serde(
        default,
        alias = "recipients",
        deserialize_with = "deserialize_recipients"
    )]
    pub to: Vec<String>,
    #[serde(default, alias = "use_starttls")]
    pub use_starttls: Option<bool>,
    #[serde(default, alias = "use_tls")]
    pub use_tls: Option<bool>,
    #[serde(default, alias = "allow_insecure_plain")]
    pub allow_insecure_plain: Option<bool>,
}

impl fmt::Debug for SmtpChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_password = self
            .password
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        let redacted_user = self
            .username
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("SmtpChannelConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &redacted_user)
            .field("password", &redacted_password)
            .field("from", &self.from)
            .field("to", &self.to)
            .field("use_starttls", &self.use_starttls)
            .field("use_tls", &self.use_tls)
            .finish()
    }
}

const fn default_smtp_port() -> u16 {
    DEFAULT_SMTP_PORT
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrVec {
    One(String),
    Many(Vec<String>),
}

fn deserialize_recipients<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<StringOrVec>::deserialize(deserializer)?;
    let list = match value {
        None => Vec::new(),
        Some(StringOrVec::One(single)) => single
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(StringOrVec::Many(items)) => items
            .into_iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
    };
    Ok(list)
}

/// Async SMTP notifier backed by `lettre::AsyncSmtpTransport<Tokio1Executor>` and `rustls-tls`.
#[derive(Debug, Clone)]
pub struct SmtpNotifier {
    timeout: Duration,
    ssrf_guard: SsrfGuard,
}

impl Default for SmtpNotifier {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_SMTP_TIMEOUT_SECS))
    }
}

impl SmtpNotifier {
    /// Creates an `SmtpNotifier` with the given SMTP timeout and strict default SSRF guard.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            ssrf_guard: SsrfGuard::new(Vec::new()),
        }
    }

    /// Creates an `SmtpNotifier` with an explicit `SsrfGuard`.
    #[must_use]
    pub fn with_ssrf_guard(timeout: Duration, ssrf_guard: SsrfGuard) -> Self {
        Self {
            timeout,
            ssrf_guard,
        }
    }

    /// Parses and validates an SMTP channel JSON configuration without leaking secrets.
    pub fn parse_config(
        channel_config_json: &serde_json::Value,
    ) -> Result<SmtpChannelConfig, CoreError> {
        let cfg: SmtpChannelConfig =
            serde_json::from_value(channel_config_json.clone()).map_err(|err| {
                CoreError::InvalidConfig(format!("invalid SMTP channel config: {err}"))
            })?;

        if cfg.host.trim().is_empty() || cfg.host.chars().any(char::is_whitespace) {
            return Err(CoreError::InvalidConfig(
                "SMTP host must be a non-empty hostname".to_owned(),
            ));
        }
        if cfg.port == 0 {
            return Err(CoreError::InvalidConfig(
                "SMTP port must be greater than 0".to_owned(),
            ));
        }
        if cfg.to.is_empty() {
            return Err(CoreError::InvalidConfig(
                "SMTP configuration requires at least one recipient".to_owned(),
            ));
        }
        let _ = parse_mailbox(&cfg.from, "from")?;
        for recipient in &cfg.to {
            let _ = parse_mailbox(recipient, "to")?;
        }
        Ok(cfg)
    }

    /// Builds the `lettre::Message` with `MultiPart::alternative` (`SinglePart::plain` + `SinglePart::html`).
    pub fn build_message(
        cfg: &SmtpChannelConfig,
        payload: &RenderedNotification,
    ) -> Result<Message, CoreError> {
        let from_mailbox = parse_mailbox(&cfg.from, "from")?;
        let mut builder = Message::builder()
            .from(from_mailbox)
            .subject(payload.subject.trim());

        for recipient in &cfg.to {
            let to_mailbox = parse_mailbox(recipient, "to")?;
            builder = builder.to(to_mailbox);
        }

        let body = MultiPart::alternative()
            .singlepart(SinglePart::plain(payload.text_body.clone()))
            .singlepart(SinglePart::html(payload.html_body.clone()));

        builder
            .multipart(body)
            .map_err(|err| CoreError::Internal(format!("failed to build SMTP MIME message: {err}")))
    }

    fn build_transport(
        &self,
        cfg: &SmtpChannelConfig,
    ) -> Result<AsyncSmtpTransport<Tokio1Executor>, CoreError> {
        let host = cfg.host.trim();
        let use_implicit_tls = cfg.use_tls == Some(true)
            || (cfg.use_tls.is_none()
                && cfg.use_starttls != Some(true)
                && cfg.port == DEFAULT_SMTP_IMPLICIT_TLS_PORT);
        let allow_plain = cfg.allow_insecure_plain == Some(true);

        let mut builder = if allow_plain {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        } else if use_implicit_tls {
            AsyncSmtpTransport::<Tokio1Executor>::relay(host).map_err(|err| {
                CoreError::InvalidConfig(format!("failed to configure SMTP TLS for {host}: {err}"))
            })?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host).map_err(|err| {
                CoreError::InvalidConfig(format!(
                    "failed to configure SMTP STARTTLS for {host}: {err}"
                ))
            })?
        };

        builder = builder.port(cfg.port).timeout(Some(self.timeout));

        if let (Some(username), Some(password)) = (
            cfg.username
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            cfg.password.as_deref().filter(|s| !s.is_empty()),
        ) {
            builder =
                builder.credentials(Credentials::new(username.to_owned(), password.to_owned()));
        }

        Ok(builder.build())
    }
}

#[async_trait]
impl Notifier for SmtpNotifier {
    fn channel_type(&self) -> NotificationChannelType {
        NotificationChannelType::Smtp
    }

    async fn send(
        &self,
        channel_config_json: &serde_json::Value,
        payload: &RenderedNotification,
    ) -> Result<(), CoreError> {
        let cfg = Self::parse_config(channel_config_json)?;
        let raw_host = cfg.host.trim();
        let unbracketed_host = raw_host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = unbracketed_host.parse::<IpAddr>() {
            if !self.ssrf_guard.is_allowlisted(unbracketed_host) {
                self.ssrf_guard.validate_ip(ip)?;
            }
            let probe_host = match ip {
                IpAddr::V4(v4) => v4.to_string(),
                IpAddr::V6(v6) => format!("[{v6}]"),
            };
            let probe_url = format!("https://{probe_host}:{}/", cfg.port);
            let _ = self.ssrf_guard.validate_url(&probe_url).await?;
        } else {
            let probe_url = format!("https://{raw_host}:{}/", cfg.port);
            let _ = self.ssrf_guard.validate_url(&probe_url).await?;
        }

        let message = Self::build_message(&cfg, payload)?;
        let transport = self.build_transport(&cfg)?;

        transport.send(message).await.map(|_| ()).map_err(|err| {
            CoreError::Upstream(format!(
                "SMTP delivery via {}:{} failed: {err}",
                cfg.host.trim(),
                cfg.port
            ))
        })
    }
}

fn parse_mailbox(raw: &str, field: &str) -> Result<Mailbox, CoreError> {
    raw.trim().parse::<Mailbox>().map_err(|err| {
        CoreError::InvalidConfig(format!("invalid SMTP '{field}' email address: {err}"))
    })
}
