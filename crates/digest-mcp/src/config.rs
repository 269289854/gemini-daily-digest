//! Runtime configuration, CSPRNG local Bearer token generation, and shared
//! thread-safe state for `digest-mcp` (usable by both CLI and Tauri Desktop).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use digest_core::constants::{
    DEFAULT_MCP_BIND_HOST, SECRET_MASK_REDACTED_PLACEHOLDER, SECRET_MASK_VISIBLE_SUFFIX_LEN,
};
use digest_core::domain::McpAccessMode;
use digest_core::error::CoreError;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use url::Url;

/// Default loopback HTTP port for the local MCP & REST control bridge.
pub const DEFAULT_MCP_HTTP_PORT: u16 = 8765;

/// Default backend `digest-server` URL when `DIGEST_SERVER_URL` is unset.
pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8080";

/// Byte length of CSPRNG local MCP Bearer tokens.
pub const LOCAL_BEARER_TOKEN_BYTES: usize = 32;

/// Environment variable specifying the backend `digest-server` base URL.
pub const ENV_DIGEST_SERVER_URL: &str = "DIGEST_SERVER_URL";

/// Environment variable specifying the backend OIDC/API Bearer token.
pub const ENV_DIGEST_API_TOKEN: &str = "DIGEST_API_TOKEN";

/// Environment variable specifying transport mode (`stdio` or `http`).
pub const ENV_DIGEST_MCP_MODE: &str = "DIGEST_MCP_MODE";

/// Environment variable specifying local HTTP bridge port (`1..=65535`).
pub const ENV_DIGEST_MCP_PORT: &str = "DIGEST_MCP_PORT";

/// Environment variable specifying local HTTP bridge Bearer token.
pub const ENV_DIGEST_MCP_TOKEN: &str = "DIGEST_MCP_TOKEN";

/// Supported MCP server transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportMode {
    #[default]
    Stdio,
    Http,
}

impl McpTransportMode {
    /// Returns the canonical wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

impl fmt::Display for McpTransportMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for McpTransportMode {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim();
        if normalized.eq_ignore_ascii_case("stdio") {
            Ok(Self::Stdio)
        } else if normalized.eq_ignore_ascii_case("http") {
            Ok(Self::Http)
        } else {
            Err(CoreError::InvalidEnumValue {
                enum_name: "McpTransportMode",
                value: s.to_owned(),
            })
        }
    }
}

/// Generates a 256-bit CSPRNG local Bearer token encoded as URL-safe Base64.
#[must_use]
pub fn generate_local_bearer_token() -> String {
    let mut bytes = [0u8; LOCAL_BEARER_TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Returns a redacted hint showing at most the trailing 4 characters (e.g. `****abcd`).
#[must_use]
pub fn mask_token_hint(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let char_count = trimmed.chars().count();
    if char_count <= SECRET_MASK_VISIBLE_SUFFIX_LEN {
        return SECRET_MASK_REDACTED_PLACEHOLDER.to_owned();
    }
    let suffix: String = trimmed
        .chars()
        .skip(char_count.saturating_sub(SECRET_MASK_VISIBLE_SUFFIX_LEN))
        .collect();
    format!("{SECRET_MASK_REDACTED_PLACEHOLDER}{suffix}")
}

/// Validates that `raw_url` uses `http` or `https`, has a host, and embeds no userinfo.
pub fn validate_server_base_url(raw_url: &str) -> Result<String, CoreError> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidConfig(
            "server_base_url must not be empty".to_owned(),
        ));
    }
    let parsed = Url::parse(trimmed)
        .map_err(|err| CoreError::InvalidConfig(format!("invalid server_base_url: {err}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(CoreError::InvalidConfig(format!(
            "server_base_url scheme must be http or https, got '{}'",
            parsed.scheme()
        )));
    }
    if !parsed.has_host() {
        return Err(CoreError::InvalidConfig(
            "server_base_url must contain a host".to_owned(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CoreError::InvalidConfig(
            "server_base_url must not embed credentials".to_owned(),
        ));
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// Runtime configuration for `digest-mcp` (redacted `Debug`).
#[derive(Clone, PartialEq, Eq)]
pub struct McpRuntimeConfig {
    pub transport: McpTransportMode,
    pub server_base_url: String,
    pub api_token: Option<String>,
    pub access_mode: McpAccessMode,
    pub bind_host: String,
    pub http_port: u16,
    pub local_bearer_token: String,
}

impl fmt::Debug for McpRuntimeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let api_token_redacted = self
            .api_token
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("McpRuntimeConfig")
            .field("transport", &self.transport)
            .field("server_base_url", &self.server_base_url)
            .field("api_token", &api_token_redacted)
            .field("access_mode", &self.access_mode)
            .field("bind_host", &self.bind_host)
            .field("http_port", &self.http_port)
            .field("local_bearer_token", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .finish()
    }
}

impl Default for McpRuntimeConfig {
    fn default() -> Self {
        Self {
            transport: McpTransportMode::Stdio,
            server_base_url: DEFAULT_SERVER_URL.to_owned(),
            api_token: None,
            access_mode: McpAccessMode::ReadOnly,
            bind_host: DEFAULT_MCP_BIND_HOST.to_owned(),
            http_port: DEFAULT_MCP_HTTP_PORT,
            local_bearer_token: generate_local_bearer_token(),
        }
    }
}

impl McpRuntimeConfig {
    /// Loads configuration from environment variables with safe defaults.
    pub fn from_env() -> Result<Self, CoreError> {
        let (transport, access_mode) = match std::env::var(ENV_DIGEST_MCP_MODE) {
            Ok(raw) if !raw.trim().is_empty() => {
                // `DIGEST_MCP_MODE` may specify transport (`stdio`/`http`) or access mode.
                if let Ok(t) = McpTransportMode::from_str(&raw) {
                    (t, McpAccessMode::ReadOnly)
                } else if let Ok(mode) = McpAccessMode::from_str(&raw) {
                    (McpTransportMode::Stdio, mode)
                } else {
                    return Err(CoreError::InvalidConfig(format!(
                        "{ENV_DIGEST_MCP_MODE} must be stdio, http, disabled, read_only, or read_write"
                    )));
                }
            }
            _ => (McpTransportMode::Stdio, McpAccessMode::ReadOnly),
        };

        let server_base_url = match std::env::var(ENV_DIGEST_SERVER_URL) {
            Ok(raw) if !raw.trim().is_empty() => validate_server_base_url(&raw)?,
            _ => DEFAULT_SERVER_URL.to_owned(),
        };

        let api_token = std::env::var(ENV_DIGEST_API_TOKEN)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());

        let http_port = match std::env::var(ENV_DIGEST_MCP_PORT) {
            Ok(raw) if !raw.trim().is_empty() => {
                let port: u16 = raw.trim().parse().map_err(|_| {
                    CoreError::InvalidConfig(format!(
                        "{ENV_DIGEST_MCP_PORT} must be an integer between 1 and 65535"
                    ))
                })?;
                if port == 0 {
                    return Err(CoreError::InvalidConfig(format!(
                        "{ENV_DIGEST_MCP_PORT} must be between 1 and 65535"
                    )));
                }
                port
            }
            _ => DEFAULT_MCP_HTTP_PORT,
        };

        let local_bearer_token = match std::env::var(ENV_DIGEST_MCP_TOKEN) {
            Ok(raw) if !raw.trim().is_empty() => raw.trim().to_owned(),
            _ => generate_local_bearer_token(),
        };

        Ok(Self {
            transport,
            server_base_url,
            api_token,
            access_mode,
            bind_host: DEFAULT_MCP_BIND_HOST.to_owned(),
            http_port,
            local_bearer_token,
        })
    }

    /// Validates that `bind_host` is loopback-only (`127.0.0.1`) and URLs/ports are valid.
    pub fn validate(&self) -> Result<(), CoreError> {
        let _ = validate_server_base_url(&self.server_base_url)?;
        if self.bind_host.trim() != DEFAULT_MCP_BIND_HOST {
            return Err(CoreError::InvalidConfig(format!(
                "MCP HTTP bridge must bind strictly to {DEFAULT_MCP_BIND_HOST}"
            )));
        }
        if self.http_port == 0 {
            return Err(CoreError::InvalidConfig(
                "http_port must be between 1 and 65535".to_owned(),
            ));
        }
        if self.local_bearer_token.trim().is_empty() {
            return Err(CoreError::InvalidConfig(
                "local_bearer_token must not be empty".to_owned(),
            ));
        }
        Ok(())
    }

    /// Returns the loopback socket address (`127.0.0.1:<http_port>`).
    #[must_use]
    pub const fn loopback_socket_addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.http_port)
    }
}

#[derive(Clone)]
struct McpSharedStateInner {
    server_base_url: String,
    api_token: Option<String>,
    access_mode: McpAccessMode,
    local_bearer_token: String,
}

/// Thread-safe shared runtime state synchronized between MCP transports and Desktop UI.
#[derive(Clone)]
pub struct McpSharedState {
    inner: Arc<RwLock<McpSharedStateInner>>,
}

impl fmt::Debug for McpSharedState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self
            .inner
            .read()
            .unwrap_or_else(|poison| poison.into_inner());
        let api_token_redacted = guard
            .api_token
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("McpSharedState")
            .field("server_base_url", &guard.server_base_url)
            .field("api_token", &api_token_redacted)
            .field("access_mode", &guard.access_mode)
            .field("local_bearer_token", &SECRET_MASK_REDACTED_PLACEHOLDER)
            .finish()
    }
}

impl McpSharedState {
    /// Creates a shared state handle from a validated runtime configuration.
    pub fn new(config: &McpRuntimeConfig) -> Result<Self, CoreError> {
        let normalized_url = validate_server_base_url(&config.server_base_url)?;
        let local_token = config.local_bearer_token.trim().to_owned();
        if local_token.is_empty() {
            return Err(CoreError::InvalidConfig(
                "local_bearer_token must not be empty".to_owned(),
            ));
        }
        let api_token = config
            .api_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        Ok(Self {
            inner: Arc::new(RwLock::new(McpSharedStateInner {
                server_base_url: normalized_url,
                api_token,
                access_mode: config.access_mode,
                local_bearer_token: local_token,
            })),
        })
    }

    /// Convenience constructor from explicit connection and guard parameters.
    pub fn from_parts(
        server_base_url: impl Into<String>,
        api_token: Option<String>,
        access_mode: McpAccessMode,
        local_bearer_token: impl Into<String>,
    ) -> Result<Self, CoreError> {
        let cfg = McpRuntimeConfig {
            server_base_url: server_base_url.into(),
            api_token,
            access_mode,
            local_bearer_token: local_bearer_token.into(),
            ..McpRuntimeConfig::default()
        };
        Self::new(&cfg)
    }

    /// Returns the current `McpAccessMode`.
    #[must_use]
    pub fn access_mode(&self) -> McpAccessMode {
        self.inner
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .access_mode
    }

    /// Updates the granted `McpAccessMode` at runtime.
    pub fn set_access_mode(&self, mode: McpAccessMode) {
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        guard.access_mode = mode;
    }

    /// Returns the current normalized `digest-server` base URL.
    #[must_use]
    pub fn server_base_url(&self) -> String {
        self.inner
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .server_base_url
            .clone()
    }

    /// Returns the current backend OIDC/API Bearer token, if configured.
    #[must_use]
    pub fn api_token(&self) -> Option<String> {
        self.inner
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .api_token
            .clone()
    }

    /// Updates the backend `server_base_url` and `api_token` atomically.
    pub fn update_connection(
        &self,
        server_base_url: impl Into<String>,
        api_token: Option<String>,
    ) -> Result<(), CoreError> {
        let normalized_url = validate_server_base_url(&server_base_url.into())?;
        let normalized_token = api_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        guard.server_base_url = normalized_url;
        guard.api_token = normalized_token;
        Ok(())
    }

    /// Updates only the backend OIDC/API Bearer token.
    pub fn set_api_token(&self, api_token: Option<String>) {
        let normalized_token = api_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        guard.api_token = normalized_token;
    }

    /// Returns the local HTTP bridge Bearer token.
    #[must_use]
    pub fn local_bearer_token(&self) -> String {
        self.inner
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .local_bearer_token
            .clone()
    }

    /// Performs constant-time verification of a candidate local Bearer token.
    #[must_use]
    pub fn verify_local_bearer_token(&self, candidate: &str) -> bool {
        let guard = self
            .inner
            .read()
            .unwrap_or_else(|poison| poison.into_inner());
        let expected = guard.local_bearer_token.as_bytes();
        let provided = candidate.trim().as_bytes();
        !expected.is_empty() && bool::from(expected.ct_eq(provided))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csprng_token_is_random_and_masked_hint_preserves_only_suffix() {
        let t1 = generate_local_bearer_token();
        let t2 = generate_local_bearer_token();
        assert_ne!(t1, t2);
        assert!(t1.len() >= 43);
        assert_eq!(mask_token_hint(""), "");
        assert_eq!(mask_token_hint("abc"), SECRET_MASK_REDACTED_PLACEHOLDER);
        assert_eq!(mask_token_hint("1234567890abcdef"), "****cdef");
    }

    #[test]
    fn runtime_config_and_shared_state_redact_sensitive_fields_in_debug() {
        let cfg = McpRuntimeConfig {
            transport: McpTransportMode::Http,
            server_base_url: "https://digest.example.com/".to_owned(),
            api_token: Some("super-secret-oidc-token".to_owned()),
            access_mode: McpAccessMode::ReadOnly,
            bind_host: DEFAULT_MCP_BIND_HOST.to_owned(),
            http_port: 8765,
            local_bearer_token: "super-secret-local-bearer-token".to_owned(),
        };
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("super-secret-oidc-token"));
        assert!(!cfg_dbg.contains("super-secret-local-bearer-token"));
        assert!(cfg_dbg.contains(SECRET_MASK_REDACTED_PLACEHOLDER));

        let shared = McpSharedState::new(&cfg).unwrap();
        let shared_dbg = format!("{shared:?}");
        assert!(!shared_dbg.contains("super-secret-oidc-token"));
        assert!(!shared_dbg.contains("super-secret-local-bearer-token"));
        assert!(shared.verify_local_bearer_token("super-secret-local-bearer-token"));
        assert!(!shared.verify_local_bearer_token("wrong-token"));

        shared.set_access_mode(McpAccessMode::ReadWrite);
        assert_eq!(shared.access_mode(), McpAccessMode::ReadWrite);
    }

    #[test]
    fn validate_server_base_url_rejects_bad_schemes_and_embedded_userinfo() {
        assert!(validate_server_base_url("http://127.0.0.1:8080/").is_ok());
        assert!(validate_server_base_url("https://digest.example.com").is_ok());
        assert!(validate_server_base_url("ftp://digest.example.com").is_err());
        assert!(validate_server_base_url("https://user:pass@digest.example.com").is_err());
        assert!(validate_server_base_url("").is_err());
    }
}
