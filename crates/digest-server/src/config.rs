//! Typed server runtime configuration composing `digest_core::config::AppConfig`
//! with database, OIDC, and envelope encryption settings.

use std::fmt;

use digest_core::config::AppConfig;
use digest_core::constants::{
    ENV_APP_MASTER_KEY_B64, ENV_DATABASE_URL, ENV_OIDC_AUDIENCE, ENV_OIDC_CLIENT_ID,
    ENV_OIDC_ISSUER_URL, ENV_STATIC_DIR, SECRET_MASK_REDACTED_PLACEHOLDER, SPA_INDEX_FILE,
};
use digest_core::crypto::{parse_master_key_b64, EnvelopeCipher};
use digest_core::domain::AuthMode;
use digest_core::error::CoreError;
use sha2::{Digest, Sha256};
use url::Url;

use crate::constants::{
    DEFAULT_ADMIN_OIDC_GROUPS, DEFAULT_OIDC_SCOPES, DEV_BYPASS_FALLBACK_CIPHER_LABEL,
    ENV_ADMIN_EMAILS, ENV_ADMIN_OIDC_GROUPS, ENV_ADMIN_OIDC_SUBS, ENV_PLATFORM_X_BRIDGE_BASE_URL,
    ENV_PLATFORM_X_BRIDGE_RELOAD_SECRET, ENV_PLATFORM_X_BRIDGE_RELOAD_URL,
};

/// Top-level server configuration loaded from environment variables.
#[derive(Clone, PartialEq, Default)]
pub struct ServerConfig {
    pub core: AppConfig,
    pub database_url: Option<String>,
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_audience: Option<String>,
    pub app_master_key_b64: Option<String>,
    pub admin_emails: Vec<String>,
    pub admin_oidc_subs: Vec<String>,
    pub admin_oidc_groups: Vec<String>,
    pub platform_x_bridge_base_url: Option<String>,
    pub platform_x_bridge_reload_url: Option<String>,
    pub platform_x_bridge_reload_secret: Option<String>,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_db = self
            .database_url
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        let redacted_key = self
            .app_master_key_b64
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        let redacted_bridge_secret = self
            .platform_x_bridge_reload_secret
            .as_ref()
            .map(|_| SECRET_MASK_REDACTED_PLACEHOLDER);
        f.debug_struct("ServerConfig")
            .field("core", &self.core)
            .field("database_url", &redacted_db)
            .field("oidc_issuer_url", &self.oidc_issuer_url)
            .field("oidc_client_id", &self.oidc_client_id)
            .field("oidc_audience", &self.oidc_audience)
            .field("app_master_key_b64", &redacted_key)
            .field("admin_emails", &self.admin_emails)
            .field("admin_oidc_subs", &self.admin_oidc_subs)
            .field("admin_oidc_groups", &self.admin_oidc_groups)
            .field(
                "platform_x_bridge_base_url",
                &self.platform_x_bridge_base_url,
            )
            .field(
                "platform_x_bridge_reload_url",
                &self.platform_x_bridge_reload_url,
            )
            .field("platform_x_bridge_reload_secret", &redacted_bridge_secret)
            .finish()
    }
}

fn read_optional_env(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    })
}

fn read_csv_env(key: &str, lowercase: bool) -> Vec<String> {
    read_optional_env(key)
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    if lowercase {
                        s.to_ascii_lowercase()
                    } else {
                        s.to_owned()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

impl ServerConfig {
    /// Loads `ServerConfig` from environment variables.
    pub fn from_env() -> Result<Self, CoreError> {
        let core = AppConfig::from_env()?;
        let env_groups = read_csv_env(ENV_ADMIN_OIDC_GROUPS, true);
        let admin_oidc_groups = if env_groups.is_empty() {
            DEFAULT_ADMIN_OIDC_GROUPS
                .iter()
                .map(|g| g.to_ascii_lowercase())
                .collect()
        } else {
            env_groups
        };
        Ok(Self {
            core,
            database_url: read_optional_env(ENV_DATABASE_URL),
            oidc_issuer_url: read_optional_env(ENV_OIDC_ISSUER_URL),
            oidc_client_id: read_optional_env(ENV_OIDC_CLIENT_ID),
            oidc_audience: read_optional_env(ENV_OIDC_AUDIENCE),
            app_master_key_b64: read_optional_env(ENV_APP_MASTER_KEY_B64),
            admin_emails: read_csv_env(ENV_ADMIN_EMAILS, true),
            admin_oidc_subs: read_csv_env(ENV_ADMIN_OIDC_SUBS, false),
            admin_oidc_groups,
            platform_x_bridge_base_url: read_optional_env(ENV_PLATFORM_X_BRIDGE_BASE_URL),
            platform_x_bridge_reload_url: read_optional_env(ENV_PLATFORM_X_BRIDGE_RELOAD_URL),
            platform_x_bridge_reload_secret: read_optional_env(ENV_PLATFORM_X_BRIDGE_RELOAD_SECRET),
        })
    }

    /// Validates configuration invariants required by the active `AuthMode`.
    pub fn validate(&self) -> Result<(), CoreError> {
        if let Some(ref b64) = self.app_master_key_b64 {
            parse_master_key_b64(b64)?;
        }

        if self.core.auth_mode == AuthMode::Oidc {
            let issuer = self.oidc_issuer_url.as_deref().ok_or_else(|| {
                CoreError::InvalidConfig(format!(
                    "{ENV_OIDC_ISSUER_URL} is required when AUTH_MODE=oidc"
                ))
            })?;
            let parsed = Url::parse(issuer).map_err(|err| {
                CoreError::InvalidConfig(format!(
                    "{ENV_OIDC_ISSUER_URL} must be a valid URL: {err}"
                ))
            })?;
            if parsed.scheme() != "https" && parsed.scheme() != "http" {
                return Err(CoreError::InvalidConfig(format!(
                    "{ENV_OIDC_ISSUER_URL} must use http or https scheme"
                )));
            }

            if self.oidc_client_id.is_none() {
                return Err(CoreError::InvalidConfig(format!(
                    "{ENV_OIDC_CLIENT_ID} is required when AUTH_MODE=oidc"
                )));
            }

            if self.app_master_key_b64.is_none() {
                return Err(CoreError::InvalidConfig(format!(
                    "{ENV_APP_MASTER_KEY_B64} is required when AUTH_MODE=oidc"
                )));
            }
        }

        if let Some(ref dir) = self.core.static_dir {
            if !dir.is_dir() {
                return Err(CoreError::InvalidConfig(format!(
                    "{ENV_STATIC_DIR} directory does not exist or is not a directory: {}",
                    dir.display()
                )));
            }
            let index_path = dir.join(SPA_INDEX_FILE);
            if !index_path.is_file() {
                return Err(CoreError::InvalidConfig(format!(
                    "{ENV_STATIC_DIR} must contain {SPA_INDEX_FILE}: {}",
                    index_path.display()
                )));
            }
        }

        Ok(())
    }

    /// Builds an `EnvelopeCipher` strictly from `APP_MASTER_KEY_B64`.
    pub fn build_cipher(&self) -> Result<EnvelopeCipher, CoreError> {
        let b64 = self.app_master_key_b64.as_deref().ok_or_else(|| {
            CoreError::InvalidConfig(format!("{ENV_APP_MASTER_KEY_B64} must be set"))
        })?;
        EnvelopeCipher::from_base64(b64)
    }

    /// Builds an `EnvelopeCipher` from `APP_MASTER_KEY_B64`, or derives a deterministic
    /// local development key ONLY when `auth_mode == AuthMode::DevBypass`.
    pub fn build_cipher_or_dev_fallback(&self) -> Result<EnvelopeCipher, CoreError> {
        if self.app_master_key_b64.is_some() {
            return self.build_cipher();
        }
        if self.core.auth_mode == AuthMode::DevBypass {
            tracing::warn!(
                env_var = ENV_APP_MASTER_KEY_B64,
                "APP_MASTER_KEY_B64 unset in dev_bypass mode; using deterministic local dev key"
            );
            let digest = Sha256::digest(DEV_BYPASS_FALLBACK_CIPHER_LABEL.as_bytes());
            let key: [u8; 32] = digest.into();
            return Ok(EnvelopeCipher::from_bytes(key));
        }
        Err(CoreError::InvalidConfig(format!(
            "{ENV_APP_MASTER_KEY_B64} is required when AUTH_MODE=oidc"
        )))
    }

    /// Returns the effective OIDC JWT audience (`OIDC_AUDIENCE` falling back to `OIDC_CLIENT_ID`).
    #[must_use]
    pub fn effective_oidc_audience(&self) -> Option<&str> {
        self.oidc_audience
            .as_deref()
            .or(self.oidc_client_id.as_deref())
    }

    /// Returns the OIDC scopes advertised to clients.
    #[must_use]
    pub fn oidc_scopes(&self) -> Vec<String> {
        DEFAULT_OIDC_SCOPES
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    use super::*;

    fn sample_master_key_b64() -> String {
        STANDARD.encode([0x55u8; 32])
    }

    #[test]
    fn debug_redacts_database_url_and_master_key() {
        let cfg = ServerConfig {
            core: AppConfig::default(),
            database_url: Some("postgres://user:supersecret@localhost:5432/digest".to_owned()),
            oidc_issuer_url: Some("https://auth.example.com/application/o/digest/".to_owned()),
            oidc_client_id: Some("digest-client".to_owned()),
            oidc_audience: None,
            app_master_key_b64: Some(sample_master_key_b64()),
            platform_x_bridge_reload_secret: Some("super-reload-secret-value".to_owned()),
            ..ServerConfig::default()
        };

        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("supersecret"));
        assert!(!rendered.contains(&sample_master_key_b64()));
        assert!(!rendered.contains("super-reload-secret-value"));
        assert!(rendered.contains(SECRET_MASK_REDACTED_PLACEHOLDER));
        assert!(rendered.contains("digest-client"));
    }

    #[test]
    fn oidc_validation_requires_issuer_client_id_and_master_key() {
        let mut cfg = ServerConfig::default();
        cfg.core.auth_mode = AuthMode::Oidc;
        assert!(matches!(cfg.validate(), Err(CoreError::InvalidConfig(_))));
        assert!(matches!(
            cfg.build_cipher_or_dev_fallback(),
            Err(CoreError::InvalidConfig(_))
        ));

        cfg.oidc_issuer_url = Some("https://auth.example.com/o/digest".to_owned());
        cfg.oidc_client_id = Some("client-1".to_owned());
        cfg.app_master_key_b64 = Some(sample_master_key_b64());
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.effective_oidc_audience(), Some("client-1"));
        assert!(cfg.build_cipher().is_ok());
    }

    #[test]
    fn dev_bypass_allows_explicit_fallback_cipher_only_in_dev_mode() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.core.auth_mode, AuthMode::DevBypass);
        assert!(cfg.validate().is_ok());
        assert!(matches!(
            cfg.build_cipher(),
            Err(CoreError::InvalidConfig(_))
        ));

        let cipher = cfg.build_cipher_or_dev_fallback().unwrap();
        let enc = cipher.encrypt("hello").unwrap();
        assert_eq!(cipher.decrypt(&enc).unwrap(), "hello");
    }

    #[test]
    fn static_dir_validation_requires_existing_dir_with_index_html() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nonexistent");

        let mut cfg = ServerConfig::default();
        cfg.core.static_dir = Some(missing);
        assert!(matches!(cfg.validate(), Err(CoreError::InvalidConfig(_))));

        cfg.core.static_dir = Some(dir.path().to_path_buf());
        assert!(matches!(cfg.validate(), Err(CoreError::InvalidConfig(_))));

        std::fs::write(dir.path().join(SPA_INDEX_FILE), "<!doctype html>").unwrap();
        assert!(cfg.validate().is_ok());
    }
}
