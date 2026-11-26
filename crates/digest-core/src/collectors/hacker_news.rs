//! Hacker News Algolia search collector strategy.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::collectors::{execute_conditional_get, CollectorContext, RawHttpOutcome};
use crate::domain::{FetchOutcome, SourceType};
use crate::error::CoreError;
use crate::traits::{
    ConditionalFetchHeaders, FetchResult, RawCollectedItem, SourceCollector, SourceDescriptor,
};

const HN_ITEM_URL_PREFIX: &str = "https://news.ycombinator.com/item?id=";
const HN_SEARCH_SUFFIX: &str = "/search_by_date";
const HN_API_V1_SUFFIX: &str = "/api/v1";

/// Collector for Hacker News stories via Algolia search API.
#[derive(Debug, Clone)]
pub struct HackerNewsCollector {
    ctx: CollectorContext,
}

impl HackerNewsCollector {
    #[must_use]
    pub fn new(ctx: CollectorContext) -> Self {
        Self { ctx }
    }
}

#[derive(Debug, Deserialize)]
struct AlgoliaResponse {
    #[serde(default)]
    hits: Vec<AlgoliaHit>,
}

#[derive(Debug, Deserialize)]
struct AlgoliaHit {
    #[serde(rename = "objectID")]
    object_id: Option<String>,
    title: Option<String>,
    story_title: Option<String>,
    url: Option<String>,
    story_url: Option<String>,
    author: Option<String>,
    story_text: Option<String>,
    comment_text: Option<String>,
    created_at: Option<DateTime<Utc>>,
    created_at_i: Option<i64>,
}

fn extract_query(handle_or_url: &str) -> Result<String, CoreError> {
    let trimmed = handle_or_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "Hacker News search query must not be empty".to_owned(),
        ));
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let parsed = url::Url::parse(trimmed)
            .map_err(|err| CoreError::InvalidInput(format!("invalid Hacker News URL: {err}")))?;
        let found = parsed
            .query_pairs()
            .find(|(k, _)| k == "query" || k == "q")
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        return found.ok_or_else(|| {
            CoreError::InvalidInput(
                "Hacker News URL must include a non-empty query parameter".to_owned(),
            )
        });
    }

    let query = trimmed
        .strip_prefix("query=")
        .unwrap_or(trimmed)
        .trim()
        .to_owned();
    if query.is_empty() {
        return Err(CoreError::InvalidInput(
            "Hacker News search query must not be empty".to_owned(),
        ));
    }
    Ok(query)
}

fn build_algolia_url(base_url: &str, query: &str) -> Result<String, CoreError> {
    let base = base_url.trim_end_matches('/');
    let endpoint = if base.ends_with(HN_API_V1_SUFFIX) {
        format!("{base}{HN_SEARCH_SUFFIX}")
    } else {
        format!("{base}{HN_API_V1_SUFFIX}{HN_SEARCH_SUFFIX}")
    };
    let mut url = url::Url::parse(&endpoint).map_err(|err| {
        CoreError::InvalidConfig(format!("invalid Hacker News base URL {endpoint}: {err}"))
    })?;
    url.query_pairs_mut()
        .append_pair("query", query)
        .append_pair("tags", "story");
    Ok(url.into())
}

#[async_trait]
impl SourceCollector for HackerNewsCollector {
    fn source_type(&self) -> SourceType {
        SourceType::HackerNews
    }

    async fn fetch(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
    ) -> Result<FetchResult, CoreError> {
        let query = extract_query(&source.handle_or_url)?;
        let url = build_algolia_url(&self.ctx.hacker_news_base_url, &query)?;

        match execute_conditional_get(&self.ctx, &url, conditional, &[], "hacker_news").await? {
            RawHttpOutcome::NotModified304(headers) => Ok(FetchResult {
                outcome: FetchOutcome::NotModified304,
                items: Vec::new(),
                etag: headers.etag,
                last_modified: headers.last_modified,
            }),
            RawHttpOutcome::Ok { bytes, headers } => {
                let parsed: AlgoliaResponse = serde_json::from_slice(&bytes).map_err(|err| {
                    CoreError::Upstream(format!("failed parsing Hacker News JSON: {err}"))
                })?;

                let items = parsed
                    .hits
                    .into_iter()
                    .filter_map(|hit| {
                        let title = hit
                            .title
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .or_else(|| {
                                hit.story_title
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                            })?
                            .to_owned();

                        let external_id = hit
                            .object_id
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());

                        let item_url = hit
                            .url
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .or_else(|| {
                                hit.story_url
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                            })
                            .map(str::to_owned)
                            .or_else(|| {
                                external_id
                                    .as_ref()
                                    .map(|id| format!("{HN_ITEM_URL_PREFIX}{id}"))
                            })?;

                        let author = hit
                            .author
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());
                        let summary = hit
                            .story_text
                            .or(hit.comment_text)
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());
                        let published_at = hit.created_at.or_else(|| {
                            hit.created_at_i
                                .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
                        });

                        Some(RawCollectedItem {
                            external_id,
                            url: item_url,
                            title,
                            author,
                            summary,
                            published_at,
                        })
                    })
                    .collect();

                Ok(FetchResult {
                    outcome: FetchOutcome::Updated,
                    items,
                    etag: headers.etag,
                    last_modified: headers.last_modified,
                })
            }
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
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn hacker_news_collector_queries_algolia_and_parses_hits() {
        let server = MockServer::start().await;
        let payload = serde_json::json!({
            "hits": [
                {
                    "objectID": "42000001",
                    "title": "Gemini CLI Launches Open Source",
                    "url": "https://example.org/gemini-cli",
                    "author": "pg_fan",
                    "story_text": "Terminal agent for Gemini models.",
                    "created_at": "2026-11-25T08:00:00Z"
                },
                {
                    "objectID": "42000002",
                    "title": "Ask HN: Gemini vs others",
                    "url": null,
                    "author": "hn_user",
                    "created_at_i": 1764057600
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/api/v1/search_by_date"))
            .and(query_param("query", "Gemini"))
            .and(query_param("tags", "story"))
            .respond_with(ResponseTemplate::new(200).set_body_json(payload))
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
        .with_hacker_news_base_url(server.uri());
        let collector = HackerNewsCollector::new(ctx);

        let source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::HackerNews,
            scope: SourceScope::SharedPublic,
            canonical_key: "hn:Gemini".to_owned(),
            name: "HN Gemini".to_owned(),
            handle_or_url: "Gemini".to_owned(),
            category: "Community".to_owned(),
        };

        let result = collector
            .fetch(&source, &ConditionalFetchHeaders::default())
            .await
            .unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.items[0].url, "https://example.org/gemini-cli");
        assert_eq!(
            result.items[1].url,
            "https://news.ycombinator.com/item?id=42000002"
        );
    }
}
