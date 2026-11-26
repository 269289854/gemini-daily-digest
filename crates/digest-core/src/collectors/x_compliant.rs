//! Pluggable compliant X API v2 / bridge collector (`x_account`, `x_search`).
//! Strictly uses official API endpoints with user-supplied credentials — never scrapes HTML.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::collectors::{execute_conditional_get, CollectorContext, RawHttpOutcome};
use crate::constants::{HEADER_AUTHORIZATION, HEADER_BEARER_PREFIX};
use crate::domain::{FetchOutcome, SourceType};
use crate::error::CoreError;
use crate::traits::{
    ConditionalFetchHeaders, FetchResult, RawCollectedItem, SourceCollector, SourceDescriptor,
};

const X_API_V2_SUFFIX: &str = "/2";
const X_STATUS_URL_BASE: &str = "https://x.com";

/// Pluggable compliant collector for X API v2 (`x_account` and `x_search`).
#[derive(Debug, Clone)]
pub struct XCompliantCollector {
    ctx: CollectorContext,
    source_type: SourceType,
}

impl XCompliantCollector {
    /// Creates an `x_account` collector.
    #[must_use]
    pub fn new(ctx: CollectorContext) -> Self {
        Self {
            ctx,
            source_type: SourceType::XAccount,
        }
    }

    /// Creates a collector bound to either `SourceType::XAccount` or `SourceType::XSearch`.
    #[must_use]
    pub fn for_type(ctx: CollectorContext, source_type: SourceType) -> Self {
        Self { ctx, source_type }
    }
}

struct XCredentials {
    bearer_token: String,
    api_base_url: String,
}

#[derive(Debug, Deserialize)]
struct XUserLookupResponse {
    data: Option<XUserDto>,
}

#[derive(Debug, Deserialize)]
struct XUserDto {
    id: String,
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XTweetsResponse {
    data: Option<Vec<XTweetDto>>,
    includes: Option<XIncludesDto>,
}

#[derive(Debug, Deserialize)]
struct XIncludesDto {
    users: Option<Vec<XUserDto>>,
}

#[derive(Debug, Deserialize)]
struct XTweetDto {
    id: String,
    text: String,
    author_id: Option<String>,
    created_at: Option<DateTime<Utc>>,
}

fn extract_x_credentials(
    credentials_json: Option<&serde_json::Value>,
    default_base_url: &str,
) -> Result<XCredentials, CoreError> {
    let obj = credentials_json
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            CoreError::Unauthorized(
                "X compliant collector requires credentials_json with bearer_token".to_owned(),
            )
        })?;

    let bearer_token = ["bearer_token", "bearerToken", "access_token", "token"]
        .iter()
        .find_map(|key| {
            obj.get(*key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| {
            CoreError::Unauthorized(
                "X compliant collector missing non-empty bearer_token in credentials_json"
                    .to_owned(),
            )
        })?
        .to_owned();

    let api_base_url = ["api_base_url", "apiBaseUrl", "bridge_url", "base_url"]
        .iter()
        .find_map(|key| {
            obj.get(*key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(default_base_url)
        .to_owned();

    Ok(XCredentials {
        bearer_token,
        api_base_url,
    })
}

fn normalize_x_v2_base(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with(X_API_V2_SUFFIX) {
        trimmed.to_owned()
    } else {
        format!("{trimmed}{X_API_V2_SUFFIX}")
    }
}

fn parse_x_username(handle_or_url: &str) -> Result<String, CoreError> {
    let trimmed = handle_or_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "X account handle must not be empty".to_owned(),
        ));
    }

    let handle = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let parsed = url::Url::parse(trimmed)
            .map_err(|err| CoreError::InvalidInput(format!("invalid X account URL: {err}")))?;
        parsed
            .path_segments()
            .and_then(|mut segs| segs.find(|s| !s.is_empty()))
            .unwrap_or("")
            .trim_start_matches('@')
            .to_owned()
    } else {
        trimmed.trim_start_matches('@').to_owned()
    };

    if handle.is_empty()
        || !handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(CoreError::InvalidInput(format!(
            "invalid X username handle: {trimmed}"
        )));
    }
    Ok(handle)
}

fn summarize_tweet_title(text: &str) -> String {
    let single_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_TITLE_CHARS: usize = 120;
    if single_line.chars().count() <= MAX_TITLE_CHARS {
        return single_line;
    }
    let truncated: String = single_line.chars().take(MAX_TITLE_CHARS - 3).collect();
    format!("{truncated}...")
}

fn map_tweets(
    tweets: Vec<XTweetDto>,
    users_by_id: &HashMap<String, String>,
    fallback_username: Option<&str>,
) -> Vec<RawCollectedItem> {
    tweets
        .into_iter()
        .map(|tweet| {
            let username = tweet
                .author_id
                .as_ref()
                .and_then(|id| users_by_id.get(id))
                .map(String::as_str)
                .or(fallback_username);
            let url_user = username.unwrap_or("i/web");
            let author = username.map(|u| format!("@{u}"));
            let title = summarize_tweet_title(&tweet.text);
            let summary = Some(tweet.text.trim().to_owned()).filter(|s| !s.is_empty());

            RawCollectedItem {
                external_id: Some(tweet.id.clone()),
                url: format!("{X_STATUS_URL_BASE}/{url_user}/status/{}", tweet.id),
                title,
                author,
                summary,
                published_at: tweet.created_at,
            }
        })
        .collect()
}

#[async_trait]
impl SourceCollector for XCompliantCollector {
    fn source_type(&self) -> SourceType {
        self.source_type
    }

    async fn fetch(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
    ) -> Result<FetchResult, CoreError> {
        self.fetch_with_credentials(source, conditional, None).await
    }

    async fn fetch_with_credentials(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
        credentials_json: Option<&serde_json::Value>,
    ) -> Result<FetchResult, CoreError> {
        let creds = extract_x_credentials(credentials_json, &self.ctx.x_api_base_url)?;
        let base_v2 = normalize_x_v2_base(&creds.api_base_url);
        let auth_value = format!("{HEADER_BEARER_PREFIX}{}", creds.bearer_token);
        let headers = [(HEADER_AUTHORIZATION, auth_value.as_str())];

        match source.source_type {
            SourceType::XAccount => {
                let handle = parse_x_username(&source.handle_or_url)?;
                let user_url = format!("{base_v2}/users/by/username/{handle}");
                let user_resp = execute_conditional_get(
                    &self.ctx,
                    &user_url,
                    &ConditionalFetchHeaders::default(),
                    &headers,
                    "x_user_lookup",
                )
                .await?;

                let user_bytes = match user_resp {
                    RawHttpOutcome::Ok { bytes, .. } => bytes,
                    RawHttpOutcome::NotModified304(_) => {
                        return Err(CoreError::Upstream(
                            "unexpected 304 on X user lookup".to_owned(),
                        ));
                    }
                };
                let user_lookup: XUserLookupResponse = serde_json::from_slice(&user_bytes)
                    .map_err(|err| {
                        CoreError::Upstream(format!("failed parsing X user lookup JSON: {err}"))
                    })?;
                let user = user_lookup.data.ok_or_else(|| {
                    CoreError::NotFound(format!("X user not found for @{handle}"))
                })?;
                let resolved_username = user.username.unwrap_or_else(|| handle.clone());

                let mut tweets_url =
                    url::Url::parse(&format!("{base_v2}/users/{}/tweets", user.id)).map_err(
                        |err| CoreError::InvalidConfig(format!("invalid X tweets URL: {err}")),
                    )?;
                tweets_url
                    .query_pairs_mut()
                    .append_pair("tweet.fields", "created_at,author_id,text")
                    .append_pair("max_results", "20");

                match execute_conditional_get(
                    &self.ctx,
                    tweets_url.as_str(),
                    conditional,
                    &headers,
                    "x_account_tweets",
                )
                .await?
                {
                    RawHttpOutcome::NotModified304(resp_headers) => Ok(FetchResult {
                        outcome: FetchOutcome::NotModified304,
                        items: Vec::new(),
                        etag: resp_headers.etag,
                        last_modified: resp_headers.last_modified,
                    }),
                    RawHttpOutcome::Ok {
                        bytes,
                        headers: resp_headers,
                    } => {
                        let parsed: XTweetsResponse =
                            serde_json::from_slice(&bytes).map_err(|err| {
                                CoreError::Upstream(format!("failed parsing X tweets JSON: {err}"))
                            })?;
                        let users_by_id = HashMap::new();
                        let items = map_tweets(
                            parsed.data.unwrap_or_default(),
                            &users_by_id,
                            Some(&resolved_username),
                        );
                        Ok(FetchResult {
                            outcome: FetchOutcome::Updated,
                            items,
                            etag: resp_headers.etag,
                            last_modified: resp_headers.last_modified,
                        })
                    }
                }
            }
            SourceType::XSearch => {
                let query = source.handle_or_url.trim();
                if query.is_empty() {
                    return Err(CoreError::InvalidInput(
                        "X search query must not be empty".to_owned(),
                    ));
                }

                let mut search_url = url::Url::parse(&format!("{base_v2}/tweets/search/recent"))
                    .map_err(|err| {
                        CoreError::InvalidConfig(format!("invalid X search URL: {err}"))
                    })?;
                search_url
                    .query_pairs_mut()
                    .append_pair("query", query)
                    .append_pair("tweet.fields", "created_at,author_id,text")
                    .append_pair("expansions", "author_id")
                    .append_pair("user.fields", "username,name")
                    .append_pair("max_results", "20");

                match execute_conditional_get(
                    &self.ctx,
                    search_url.as_str(),
                    conditional,
                    &headers,
                    "x_search",
                )
                .await?
                {
                    RawHttpOutcome::NotModified304(resp_headers) => Ok(FetchResult {
                        outcome: FetchOutcome::NotModified304,
                        items: Vec::new(),
                        etag: resp_headers.etag,
                        last_modified: resp_headers.last_modified,
                    }),
                    RawHttpOutcome::Ok {
                        bytes,
                        headers: resp_headers,
                    } => {
                        let parsed: XTweetsResponse =
                            serde_json::from_slice(&bytes).map_err(|err| {
                                CoreError::Upstream(format!("failed parsing X search JSON: {err}"))
                            })?;
                        let mut users_by_id = HashMap::new();
                        if let Some(users) = parsed.includes.and_then(|inc| inc.users) {
                            for user in users {
                                if let Some(username) = user.username {
                                    users_by_id.insert(user.id, username);
                                }
                            }
                        }
                        let items = map_tweets(parsed.data.unwrap_or_default(), &users_by_id, None);
                        Ok(FetchResult {
                            outcome: FetchOutcome::Updated,
                            items,
                            etag: resp_headers.etag,
                            last_modified: resp_headers.last_modified,
                        })
                    }
                }
            }
            other => Err(CoreError::InvalidInput(format!(
                "XCompliantCollector does not support source type {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CollectorTuning, ResourceLimitsTuning};
    use crate::domain::SourceScope;
    use crate::http::shared_http_client;
    use crate::ssrf::SsrfGuard;
    use uuid::Uuid;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn x_compliant_collector_requires_credentials() {
        let server = MockServer::start().await;
        let client = shared_http_client(None, &CollectorTuning::default()).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let ctx = CollectorContext::new(
            client,
            ssrf,
            CollectorTuning::default(),
            ResourceLimitsTuning::default(),
        )
        .with_x_api_base_url(server.uri());
        let collector = XCompliantCollector::new(ctx);

        let source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::XAccount,
            scope: SourceScope::UserPrivate,
            canonical_key: "x_account:GoogleDeepMind".to_owned(),
            name: "GoogleDeepMind".to_owned(),
            handle_or_url: "@GoogleDeepMind".to_owned(),
            category: "Social".to_owned(),
        };

        let err = collector
            .fetch_with_credentials(&source, &ConditionalFetchHeaders::default(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn x_compliant_collector_fetches_account_and_search_via_v2_api() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/2/users/by/username/GoogleDeepMind"))
            .and(header("authorization", "Bearer test-x-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "id": "42", "username": "GoogleDeepMind" }
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/2/users/42/tweets"))
            .and(header("authorization", "Bearer test-x-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {
                        "id": "99001",
                        "text": "Introducing Gemini 2.5 Flash thinking mode.",
                        "author_id": "42",
                        "created_at": "2026-11-25T15:00:00Z"
                    }
                ]
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/2/tweets/search/recent"))
            .and(query_param("query", "Gemini API"))
            .and(header("authorization", "Bearer test-x-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {
                        "id": "99002",
                        "text": "Gemini API batch pricing update",
                        "author_id": "77",
                        "created_at": "2026-11-25T16:00:00Z"
                    }
                ],
                "includes": {
                    "users": [{ "id": "77", "username": "GoogleAI" }]
                }
            })))
            .mount(&server)
            .await;

        let client = shared_http_client(None, &CollectorTuning::default()).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let ctx = CollectorContext::new(
            client,
            ssrf,
            CollectorTuning::default(),
            ResourceLimitsTuning::default(),
        )
        .with_x_api_base_url(server.uri());
        let creds = serde_json::json!({ "bearer_token": "test-x-token" });

        let account_collector = XCompliantCollector::for_type(ctx.clone(), SourceType::XAccount);
        let account_source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::XAccount,
            scope: SourceScope::UserPrivate,
            canonical_key: "x_account:GoogleDeepMind".to_owned(),
            name: "GoogleDeepMind".to_owned(),
            handle_or_url: "@GoogleDeepMind".to_owned(),
            category: "Social".to_owned(),
        };
        let account_res = account_collector
            .fetch_with_credentials(
                &account_source,
                &ConditionalFetchHeaders::default(),
                Some(&creds),
            )
            .await
            .unwrap();
        assert_eq!(account_res.items.len(), 1);
        assert_eq!(
            account_res.items[0].url,
            "https://x.com/GoogleDeepMind/status/99001"
        );

        let search_collector = XCompliantCollector::for_type(ctx, SourceType::XSearch);
        let search_source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::XSearch,
            scope: SourceScope::UserPrivate,
            canonical_key: "x_search:Gemini API".to_owned(),
            name: "X Search".to_owned(),
            handle_or_url: "Gemini API".to_owned(),
            category: "Social".to_owned(),
        };
        let search_res = search_collector
            .fetch_with_credentials(
                &search_source,
                &ConditionalFetchHeaders::default(),
                Some(&creds),
            )
            .await
            .unwrap();
        assert_eq!(search_res.items.len(), 1);
        assert_eq!(search_res.items[0].author.as_deref(), Some("@GoogleAI"));
        assert_eq!(
            search_res.items[0].url,
            "https://x.com/GoogleAI/status/99002"
        );
    }
}
