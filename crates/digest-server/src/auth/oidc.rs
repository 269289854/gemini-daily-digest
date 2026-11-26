//! Authentik OIDC RS256 JWKS JWT validator with discovery caching,
//! singleflight key-rotation refresh, and `spawn_blocking` RSA verification.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use digest_core::constants::{HEADER_AUTHORIZATION, HEADER_BEARER_PREFIX};
use digest_core::error::CoreError;
use http::HeaderMap;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet, KeyAlgorithm, PublicKeyUse};
use jsonwebtoken::{DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};
use url::Url;

use crate::config::ServerConfig;
use crate::constants::{
    ADMIN_USER_ROLE, DEFAULT_USER_ROLE, JWKS_CACHE_TTL_SECS, JWT_LEEWAY_SECS,
    OIDC_HTTP_TIMEOUT_SECS, OIDC_JWT_ALGORITHM, OIDC_WELL_KNOWN_SUFFIX,
};
use crate::repo::{UpsertUserCmd, UserRecord, UserRepo};
use crate::services::AuthIdentity;

#[derive(Debug, Deserialize)]
struct OidcDiscoveryDoc {
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct OidcClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    is_superuser: Option<bool>,
}

struct CachedJwks {
    keys_by_kid: HashMap<String, DecodingKey>,
    single_fallback_key: Option<DecodingKey>,
    fetched_at: Instant,
    generation: u64,
}

impl CachedJwks {
    fn lookup(&self, kid: Option<&str>) -> Option<DecodingKey> {
        match kid.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => self.keys_by_kid.get(k).cloned(),
            None => self.single_fallback_key.clone(),
        }
    }
}

/// Validates OIDC Bearer JWTs using `RS256` and cached JWKS public keys.
pub struct OidcValidator {
    config: Arc<ServerConfig>,
    http: reqwest::Client,
    cache: RwLock<Option<CachedJwks>>,
    refresh_lock: Mutex<()>,
}

impl OidcValidator {
    /// Creates a new `OidcValidator`.
    #[must_use]
    pub fn new(config: Arc<ServerConfig>, http: reqwest::Client) -> Self {
        Self {
            config,
            http,
            cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    /// Authenticates an incoming HTTP request using `Authorization: Bearer <jwt>`.
    pub async fn authenticate(
        &self,
        users: &dyn UserRepo,
        headers: &HeaderMap,
    ) -> Result<UserRecord, CoreError> {
        let token = extract_bearer_token(headers)?;
        let identity = self.verify_token(token).await?;
        users
            .upsert_oidc_user(UpsertUserCmd {
                oidc_sub: identity.oidc_sub,
                email: identity.email,
                display_name: identity.display_name,
                role: identity.role,
            })
            .await
    }

    /// Verifies an OIDC JWT strictly with `Algorithm::RS256` on `spawn_blocking`.
    pub async fn verify_token(&self, token: &str) -> Result<AuthIdentity, CoreError> {
        let issuer = self.config.oidc_issuer_url.as_deref().ok_or_else(|| {
            CoreError::InvalidConfig("OIDC issuer URL is not configured".to_owned())
        })?;
        let audience = self.config.effective_oidc_audience().ok_or_else(|| {
            CoreError::InvalidConfig("OIDC audience/client_id is not configured".to_owned())
        })?;

        if token.split('.').count() == 5 {
            return Err(CoreError::Unauthorized(
                "encrypted JWT (JWE) is not supported; disable Encryption Key on the OIDC provider and keep only RS256 Signing Key".to_owned(),
            ));
        }

        let header = jsonwebtoken::decode_header(token)
            .map_err(|_| CoreError::Unauthorized("malformed JWT header".to_owned()))?;
        if header.alg != OIDC_JWT_ALGORITHM {
            return Err(CoreError::Unauthorized(
                "unsupported JWT signing algorithm".to_owned(),
            ));
        }

        let decoding_key = self.resolve_decoding_key(header.kid.as_deref()).await?;

        let base_issuer = issuer.trim().trim_end_matches('/');
        let issuer_with_slash = format!("{base_issuer}/");

        let mut validation = Validation::new(OIDC_JWT_ALGORITHM);
        validation.leeway = JWT_LEEWAY_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.set_audience(&[audience]);
        validation.set_issuer(&[base_issuer, issuer_with_slash.as_str()]);
        validation.set_required_spec_claims(&["exp", "sub", "iss", "aud"]);

        let token_owned = token.to_owned();
        let token_data = tokio::task::spawn_blocking(move || {
            jsonwebtoken::decode::<OidcClaims>(&token_owned, &decoding_key, &validation)
        })
        .await
        .map_err(|err| CoreError::Internal(format!("JWT verification task failed: {err}")))?
        .map_err(|err| CoreError::Unauthorized(format!("invalid OIDC token: {err}")))?;

        let claims = token_data.claims;
        let sub = claims.sub.trim();
        if sub.is_empty() {
            return Err(CoreError::Unauthorized(
                "OIDC token missing non-empty sub claim".to_owned(),
            ));
        }

        let email = claims
            .email
            .map(|e| e.trim().to_owned())
            .filter(|e| !e.is_empty());
        let display_name = claims
            .name
            .or(claims.preferred_username)
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty());
        let is_admin = claims.is_superuser == Some(true)
            || claims.groups.iter().any(|group| {
                let g = group.trim().to_ascii_lowercase();
                !g.is_empty()
                    && self
                        .config
                        .admin_oidc_groups
                        .iter()
                        .any(|configured| configured == &g)
            })
            || email.as_ref().is_some_and(|e| {
                let lower = e.to_ascii_lowercase();
                self.config
                    .admin_emails
                    .iter()
                    .any(|configured| configured == &lower)
            })
            || self
                .config
                .admin_oidc_subs
                .iter()
                .any(|configured| configured == sub);
        let role = if is_admin {
            ADMIN_USER_ROLE.to_owned()
        } else {
            DEFAULT_USER_ROLE.to_owned()
        };

        Ok(AuthIdentity {
            oidc_sub: sub.to_owned(),
            email,
            display_name,
            role,
        })
    }

    async fn resolve_decoding_key(&self, kid: Option<&str>) -> Result<DecodingKey, CoreError> {
        let ttl = Duration::from_secs(JWKS_CACHE_TTL_SECS);
        let mut seen_generation: Option<u64> = None;

        {
            let guard = self.cache.read().await;
            if let Some(cached) = guard.as_ref() {
                seen_generation = Some(cached.generation);
                if cached.fetched_at.elapsed() < ttl {
                    if let Some(key) = cached.lookup(kid) {
                        return Ok(key);
                    }
                }
            }
        }

        let _refresh_guard = self.refresh_lock.lock().await;

        let current_generation = {
            let guard = self.cache.read().await;
            if let Some(cached) = guard.as_ref() {
                if Some(cached.generation) != seen_generation && cached.fetched_at.elapsed() < ttl {
                    if let Some(key) = cached.lookup(kid) {
                        return Ok(key);
                    }
                }
                cached.generation
            } else {
                0
            }
        };

        let refreshed = self
            .fetch_jwks(current_generation.saturating_add(1))
            .await?;
        let resolved = refreshed.lookup(kid).ok_or_else(|| {
            CoreError::Unauthorized("matching signing key not found in OIDC JWKS".to_owned())
        })?;

        let mut write_guard = self.cache.write().await;
        *write_guard = Some(refreshed);

        Ok(resolved)
    }

    async fn fetch_jwks(&self, generation: u64) -> Result<CachedJwks, CoreError> {
        let issuer = self.config.oidc_issuer_url.as_deref().ok_or_else(|| {
            CoreError::InvalidConfig("OIDC issuer URL is not configured".to_owned())
        })?;
        let discovery_url = format!(
            "{}{OIDC_WELL_KNOWN_SUFFIX}",
            issuer.trim().trim_end_matches('/')
        );
        let timeout = Duration::from_secs(OIDC_HTTP_TIMEOUT_SECS);

        let discovery_resp = self
            .http
            .get(&discovery_url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|err| {
                if err.is_timeout() {
                    CoreError::UpstreamTimeout("OIDC discovery request timed out".to_owned())
                } else {
                    CoreError::Upstream(format!("OIDC discovery request failed: {err}"))
                }
            })?;

        if !discovery_resp.status().is_success() {
            return Err(CoreError::Upstream(format!(
                "OIDC discovery returned HTTP {}",
                discovery_resp.status().as_u16()
            )));
        }

        let discovery: OidcDiscoveryDoc = discovery_resp.json().await.map_err(|err| {
            CoreError::Upstream(format!("invalid OIDC discovery JSON response: {err}"))
        })?;

        let jwks_url = Url::parse(discovery.jwks_uri.trim()).map_err(|err| {
            CoreError::Upstream(format!("invalid jwks_uri in OIDC discovery: {err}"))
        })?;
        if jwks_url.scheme() != "http" && jwks_url.scheme() != "https" {
            return Err(CoreError::Upstream(
                "OIDC jwks_uri must use http or https scheme".to_owned(),
            ));
        }

        let jwks_resp = self
            .http
            .get(jwks_url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|err| {
                if err.is_timeout() {
                    CoreError::UpstreamTimeout("OIDC JWKS request timed out".to_owned())
                } else {
                    CoreError::Upstream(format!("OIDC JWKS request failed: {err}"))
                }
            })?;

        if !jwks_resp.status().is_success() {
            return Err(CoreError::Upstream(format!(
                "OIDC JWKS endpoint returned HTTP {}",
                jwks_resp.status().as_u16()
            )));
        }

        let jwks: JwkSet = jwks_resp
            .json()
            .await
            .map_err(|err| CoreError::Upstream(format!("invalid OIDC JWKS JSON: {err}")))?;

        let mut keys_by_kid = HashMap::new();
        let mut all_rsa_keys = Vec::new();

        for jwk in &jwks.keys {
            if !matches!(jwk.algorithm, AlgorithmParameters::RSA(_)) {
                continue;
            }
            if let Some(ref key_use) = jwk.common.public_key_use {
                if *key_use != PublicKeyUse::Signature {
                    continue;
                }
            }
            if let Some(alg) = jwk.common.key_algorithm {
                if alg != KeyAlgorithm::RS256 {
                    continue;
                }
            }
            if let Ok(decoding_key) = DecodingKey::from_jwk(jwk) {
                all_rsa_keys.push(decoding_key.clone());
                if let Some(kid) = jwk
                    .common
                    .key_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                {
                    keys_by_kid.insert(kid.to_owned(), decoding_key);
                }
            }
        }

        if keys_by_kid.is_empty() && all_rsa_keys.is_empty() {
            return Err(CoreError::Upstream(
                "OIDC JWKS contained no usable RS256 public keys".to_owned(),
            ));
        }

        let single_fallback_key = if all_rsa_keys.len() == 1 {
            all_rsa_keys.into_iter().next()
        } else {
            None
        };

        Ok(CachedJwks {
            keys_by_kid,
            single_fallback_key,
            fetched_at: Instant::now(),
            generation,
        })
    }
}

/// Extracts a non-empty Bearer token from `Authorization` using `HEADER_AUTHORIZATION` / `HEADER_BEARER_PREFIX`.
pub fn extract_bearer_token(headers: &HeaderMap) -> Result<&str, CoreError> {
    let raw = headers
        .get(HEADER_AUTHORIZATION)
        .ok_or_else(|| CoreError::Unauthorized("missing authorization header".to_owned()))?
        .to_str()
        .map_err(|_| CoreError::Unauthorized("invalid authorization header encoding".to_owned()))?
        .trim();

    let prefix_len = HEADER_BEARER_PREFIX.len();
    let token = raw
        .get(..prefix_len)
        .filter(|prefix| prefix.eq_ignore_ascii_case(HEADER_BEARER_PREFIX))
        .and_then(|_| raw.get(prefix_len..))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            CoreError::Unauthorized("authorization header must use Bearer <token>".to_owned())
        })?;

    Ok(token)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use chrono::Utc;
    use digest_core::config::AppConfig;
    use digest_core::domain::AuthMode;
    use http::HeaderValue;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;
    use serde::Serialize;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    struct TestUserRepo;

    #[async_trait]
    impl UserRepo for TestUserRepo {
        async fn upsert_oidc_user(&self, cmd: UpsertUserCmd) -> Result<UserRecord, CoreError> {
            Ok(UserRecord {
                id: Uuid::new_v4(),
                oidc_sub: cmd.oidc_sub,
                email: cmd.email,
                display_name: cmd.display_name,
                role: cmd.role,
                created_at: Utc::now(),
                last_login_at: Some(Utc::now()),
            })
        }
        async fn get_user_by_id(&self, _: Uuid) -> Result<Option<UserRecord>, CoreError> {
            Ok(None)
        }
        async fn get_onboarding_flags(&self, _: Uuid) -> Result<OnboardingFlags, CoreError> {
            Ok(OnboardingFlags::default())
        }
    }

    use crate::repo::OnboardingFlags;

    #[derive(Clone, Serialize)]
    struct TestClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: usize,
        nbf: usize,
        email: Option<String>,
        name: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        groups: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_superuser: Option<bool>,
    }

    fn build_rsa_keypair_and_jwk(kid: &str) -> (EncodingKey, serde_json::Value) {
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let public_key = private_key.to_public_key();

        let pem = private_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();

        let n = URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

        let jwk = serde_json::json!({
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": kid,
            "n": n,
            "e": e,
        });

        (encoding_key, jwk)
    }

    async fn mount_oidc_server(
        server: &MockServer,
        jwk: serde_json::Value,
        discovery_expectation: u64,
    ) {
        let jwks_uri = format!("{}/jwks.json", server.uri());
        Mock::given(method("GET"))
            .and(path("/issuer/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": format!("{}/issuer", server.uri()),
                "jwks_uri": jwks_uri,
            })))
            .expect(discovery_expectation)
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": [jwk]
            })))
            .expect(discovery_expectation)
            .mount(server)
            .await;
    }

    fn test_oidc_config(issuer: String) -> Arc<ServerConfig> {
        let core = AppConfig {
            auth_mode: AuthMode::Oidc,
            ..AppConfig::default()
        };
        Arc::new(ServerConfig {
            core,
            database_url: None,
            oidc_issuer_url: Some(issuer),
            oidc_client_id: Some("digest-web".to_owned()),
            oidc_audience: Some("digest-api".to_owned()),
            app_master_key_b64: None,
            admin_oidc_groups: vec!["authentik admins".to_owned(), "admin".to_owned()],
            ..ServerConfig::default()
        })
    }

    #[tokio::test]
    async fn oidc_validates_rs256_jwt_caches_jwks_and_rejects_expired_wrong_aud_and_alg_none() {
        let server = MockServer::start().await;
        let (encoding_key, jwk) = build_rsa_keypair_and_jwk("key-1");
        mount_oidc_server(&server, jwk, 1).await;

        let issuer = format!("{}/issuer", server.uri());
        let config = test_oidc_config(issuer.clone());
        let validator = OidcValidator::new(config, reqwest::Client::new());
        let users = TestUserRepo;

        let now = Utc::now().timestamp() as usize;
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("key-1".to_owned());

        // 1. Valid JWT (with trailing slash issuer normalization test)
        let valid_claims = TestClaims {
            sub: "oidc-user-42".to_owned(),
            iss: format!("{issuer}/"),
            aud: "digest-api".to_owned(),
            exp: now + 600,
            nbf: now - 10,
            email: Some("alice@example.com".to_owned()),
            name: Some("Alice".to_owned()),
            groups: Vec::new(),
            is_superuser: None,
        };
        let valid_jwt = jsonwebtoken::encode(&header, &valid_claims, &encoding_key).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            HEADER_AUTHORIZATION,
            HeaderValue::from_str(&format!("{HEADER_BEARER_PREFIX}{valid_jwt}")).unwrap(),
        );
        let user = validator.authenticate(&users, &headers).await.unwrap();
        assert_eq!(user.oidc_sub, "oidc-user-42");
        assert_eq!(user.email.as_deref(), Some("alice@example.com"));
        assert_eq!(user.display_name.as_deref(), Some("Alice"));
        assert_eq!(user.role, "member");

        // Second call uses cached JWKS (wiremock expect(1) verifies no second HTTP fetch!)
        let user_cached = validator.authenticate(&users, &headers).await.unwrap();
        assert_eq!(user_cached.oidc_sub, "oidc-user-42");

        // 2. Expired JWT (beyond JWT_LEEWAY_SECS)
        let expired_claims = TestClaims {
            sub: "oidc-user-42".to_owned(),
            iss: issuer.clone(),
            aud: "digest-api".to_owned(),
            exp: now.saturating_sub(300),
            nbf: now.saturating_sub(600),
            email: None,
            name: None,
            groups: Vec::new(),
            is_superuser: None,
        };
        let expired_jwt = jsonwebtoken::encode(&header, &expired_claims, &encoding_key).unwrap();
        assert!(matches!(
            validator.verify_token(&expired_jwt).await,
            Err(CoreError::Unauthorized(_))
        ));

        // 3. Wrong audience
        let wrong_aud_claims = TestClaims {
            sub: "oidc-user-42".to_owned(),
            iss: issuer.clone(),
            aud: "wrong-audience".to_owned(),
            exp: now + 600,
            nbf: now - 10,
            email: None,
            name: None,
            groups: Vec::new(),
            is_superuser: None,
        };
        let wrong_aud_jwt =
            jsonwebtoken::encode(&header, &wrong_aud_claims, &encoding_key).unwrap();
        assert!(matches!(
            validator.verify_token(&wrong_aud_jwt).await,
            Err(CoreError::Unauthorized(_))
        ));

        // 4. alg=none downgrade attack MUST be rejected
        let alg_none_header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let alg_none_payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&TestClaims {
                sub: "attacker".to_owned(),
                iss: issuer,
                aud: "digest-api".to_owned(),
                exp: now + 600,
                nbf: now - 10,
                email: None,
                name: None,
                groups: Vec::new(),
                is_superuser: None,
            })
            .unwrap(),
        );
        let alg_none_jwt = format!("{alg_none_header}.{alg_none_payload}.");
        assert!(matches!(
            validator.verify_token(&alg_none_jwt).await,
            Err(CoreError::Unauthorized(_))
        ));

        // 5. Missing/malformed Authorization header
        assert!(matches!(
            extract_bearer_token(&HeaderMap::new()),
            Err(CoreError::Unauthorized(_))
        ));

        // 6. 5-segment JWE token returns clear actionable guidance
        let jwe_err = validator
            .verify_token("eyJhbGciOiJSU0EtT0FFUC0yNTYifQ.enc_key.iv.ciphertext.tag")
            .await
            .unwrap_err();
        assert!(matches!(jwe_err, CoreError::Unauthorized(_)));
        assert!(jwe_err.to_string().contains("JWE"));
    }

    #[tokio::test]
    async fn oidc_refreshes_jwks_on_unknown_kid() {
        let server = MockServer::start().await;
        let (key1, jwk1) = build_rsa_keypair_and_jwk("kid-old");
        let (key2, jwk2) = build_rsa_keypair_and_jwk("kid-rotated");

        let jwks_uri = format!("{}/jwks.json", server.uri());
        Mock::given(method("GET"))
            .and(path("/issuer/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": format!("{}/issuer", server.uri()),
                "jwks_uri": jwks_uri,
            })))
            .mount(&server)
            .await;

        // First JWKS returns kid-old only
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": [jwk1]
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Second JWKS after rotation returns kid-rotated
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": [jwk2]
            })))
            .mount(&server)
            .await;

        let issuer = format!("{}/issuer", server.uri());
        let validator =
            OidcValidator::new(test_oidc_config(issuer.clone()), reqwest::Client::new());

        let now = Utc::now().timestamp() as usize;
        let claims = TestClaims {
            sub: "user-rotated".to_owned(),
            iss: issuer.clone(),
            aud: "digest-api".to_owned(),
            exp: now + 600,
            nbf: now - 10,
            email: None,
            name: None,
            groups: Vec::new(),
            is_superuser: None,
        };

        let mut h1 = Header::new(Algorithm::RS256);
        h1.kid = Some("kid-old".to_owned());
        let jwt1 = jsonwebtoken::encode(&h1, &claims, &key1).unwrap();
        assert_eq!(
            validator.verify_token(&jwt1).await.unwrap().oidc_sub,
            "user-rotated"
        );

        // Now present token signed with rotated key -> triggers JWKS refresh
        let mut h2 = Header::new(Algorithm::RS256);
        h2.kid = Some("kid-rotated".to_owned());
        let jwt2 = jsonwebtoken::encode(&h2, &claims, &key2).unwrap();
        assert_eq!(
            validator.verify_token(&jwt2).await.unwrap().oidc_sub,
            "user-rotated"
        );
    }

    #[tokio::test]
    async fn oidc_promotes_admin_via_groups_superuser_or_configured_email() {
        let server = MockServer::start().await;
        let (key, jwk) = build_rsa_keypair_and_jwk("key-admin");
        mount_oidc_server(&server, jwk, 1).await;

        let issuer = format!("{}/issuer", server.uri());
        let mut base = (*test_oidc_config(issuer.clone())).clone();
        base.admin_emails = vec!["boss@example.com".to_owned()];
        let validator = OidcValidator::new(Arc::new(base), reqwest::Client::new());

        let now = Utc::now().timestamp() as usize;
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("key-admin".to_owned());

        let group_claims = TestClaims {
            sub: "sub-group".to_owned(),
            iss: issuer.clone(),
            aud: "digest-api".to_owned(),
            exp: now + 600,
            nbf: now - 10,
            email: None,
            name: None,
            groups: vec!["authentik Admins".to_owned()],
            is_superuser: None,
        };
        let jwt_group = jsonwebtoken::encode(&header, &group_claims, &key).unwrap();
        assert_eq!(
            validator.verify_token(&jwt_group).await.unwrap().role,
            ADMIN_USER_ROLE
        );

        let super_claims = TestClaims {
            sub: "sub-super".to_owned(),
            is_superuser: Some(true),
            groups: Vec::new(),
            ..group_claims.clone()
        };
        let jwt_super = jsonwebtoken::encode(&header, &super_claims, &key).unwrap();
        assert_eq!(
            validator.verify_token(&jwt_super).await.unwrap().role,
            ADMIN_USER_ROLE
        );

        let email_claims = TestClaims {
            sub: "sub-email".to_owned(),
            email: Some("BOSS@example.com".to_owned()),
            is_superuser: None,
            groups: Vec::new(),
            ..group_claims
        };
        let jwt_email = jsonwebtoken::encode(&header, &email_claims, &key).unwrap();
        assert_eq!(
            validator.verify_token(&jwt_email).await.unwrap().role,
            ADMIN_USER_ROLE
        );
    }
}
