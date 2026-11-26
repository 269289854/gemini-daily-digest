//! Reddit subreddit JSON collector strategy.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::collectors::{execute_conditional_get, CollectorContext, RawHttpOutcome};
use crate::constants::DEFAULT_REDDIT_BASE_URL;
use crate::domain::{FetchOutcome, SourceType};
use crate::error::CoreError;
use crate::traits::{
    ConditionalFetchHeaders, FetchResult, RawCollectedItem, SourceCollector, SourceDescriptor,
};

/// Collector for Reddit `/r/{subreddit}/new.json` feeds.
#[derive(Debug, Clone)]
pub struct RedditCollector {
    ctx: CollectorContext,
}

impl RedditCollector {
    #[must_use]
    pub fn new(ctx: CollectorContext) -> Self {
        Self { ctx }
    }
}

#[derive(Debug, Deserialize)]
struct RedditListingResponse {
    data: Option<RedditListingData>,
}

#[derive(Debug, Deserialize)]
struct RedditListingData {
    #[serde(default)]
    children: Vec<RedditChild>,
}

#[derive(Debug, Deserialize)]
struct RedditChild {
    data: RedditPostData,
}

#[derive(Debug, Deserialize)]
struct RedditPostData {
    id: Option<String>,
    name: Option<String>,
    title: Option<String>,
    author: Option<String>,
    selftext: Option<String>,
    url: Option<String>,
    permalink: Option<String>,
    created_utc: Option<f64>,
}

fn parse_subreddit(handle_or_url: &str) -> Result<String, CoreError> {
    let trimmed = handle_or_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "Reddit subreddit handle_or_url must not be empty".to_owned(),
        ));
    }

    let raw_sub: String = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let parsed = url::Url::parse(trimmed)
            .map_err(|err| CoreError::InvalidInput(format!("invalid Reddit URL: {err}")))?;
        let segments: Vec<&str> = parsed
            .path_segments()
            .map(|iter| iter.filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        if let Some(pos) = segments
            .iter()
            .position(|seg| seg.eq_ignore_ascii_case("r"))
        {
            segments.get(pos + 1).copied().unwrap_or("").to_owned()
        } else {
            segments.first().copied().unwrap_or("").to_owned()
        }
    } else {
        let without_leading = trimmed.trim_start_matches('/');
        let without_r = without_leading
            .strip_prefix("r/")
            .or_else(|| without_leading.strip_prefix("R/"))
            .unwrap_or(without_leading);
        without_r
            .trim_end_matches('/')
            .strip_suffix("/new.json")
            .unwrap_or(without_r.trim_end_matches('/'))
            .to_owned()
    };

    if raw_sub.is_empty()
        || !raw_sub
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(CoreError::InvalidInput(format!(
            "invalid Reddit subreddit name: {trimmed}"
        )));
    }

    Ok(raw_sub)
}

#[async_trait]
impl SourceCollector for RedditCollector {
    fn source_type(&self) -> SourceType {
        SourceType::Reddit
    }

    async fn fetch(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
    ) -> Result<FetchResult, CoreError> {
        let subreddit = parse_subreddit(&source.handle_or_url)?;
        let base = self.ctx.reddit_base_url.trim_end_matches('/');
        let url = format!("{base}/r/{subreddit}/new.json");

        match execute_conditional_get(&self.ctx, &url, conditional, &[], "reddit").await? {
            RawHttpOutcome::NotModified304(headers) => Ok(FetchResult {
                outcome: FetchOutcome::NotModified304,
                items: Vec::new(),
                etag: headers.etag,
                last_modified: headers.last_modified,
            }),
            RawHttpOutcome::Ok { bytes, headers } => {
                let listing: RedditListingResponse =
                    serde_json::from_slice(&bytes).map_err(|err| {
                        CoreError::Upstream(format!("failed parsing Reddit JSON: {err}"))
                    })?;

                let children = listing.data.map(|d| d.children).unwrap_or_default();
                let items = children
                    .into_iter()
                    .filter_map(|child| {
                        let post = child.data;
                        let title = post.title?.trim().to_owned();
                        if title.is_empty() {
                            return None;
                        }
                        let permalink = post
                            .permalink
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|p| {
                                if p.starts_with("http://") || p.starts_with("https://") {
                                    p.to_owned()
                                } else {
                                    format!("{DEFAULT_REDDIT_BASE_URL}{p}")
                                }
                            });
                        let url = post
                            .url
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .or(permalink)?;

                        let external_id = post
                            .name
                            .or(post.id)
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());
                        let author = post
                            .author
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());
                        let summary = post
                            .selftext
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());
                        let published_at = post
                            .created_utc
                            .and_then(|ts| DateTime::<Utc>::from_timestamp(ts.trunc() as i64, 0));

                        Some(RawCollectedItem {
                            external_id,
                            url,
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn reddit_collector_fetches_subreddit_new_json() {
        let server = MockServer::start().await;
        let payload = serde_json::json!({
            "data": {
                "children": [
                    {
                        "data": {
                            "id": "abc123",
                            "name": "t3_abc123",
                            "title": "Gemini 2.5 benchmark discussion",
                            "author": "ml_researcher",
                            "selftext": "Detailed latency numbers inside.",
                            "url": "https://www.reddit.com/r/LocalLLaMA/comments/abc123/gemini/",
                            "permalink": "/r/LocalLLaMA/comments/abc123/gemini/",
                            "created_utc": 1764072000.0
                        }
                    }
                ]
            }
        });

        Mock::given(method("GET"))
            .and(path("/r/LocalLLaMA/new.json"))
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
        .with_reddit_base_url(server.uri());
        let collector = RedditCollector::new(ctx);

        let source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::Reddit,
            scope: SourceScope::SharedPublic,
            canonical_key: "reddit:LocalLLaMA".to_owned(),
            name: "r/LocalLLaMA".to_owned(),
            handle_or_url: "r/LocalLLaMA".to_owned(),
            category: "Community".to_owned(),
        };

        let result = collector
            .fetch(&source, &ConditionalFetchHeaders::default())
            .await
            .unwrap();
        assert_eq!(result.outcome, FetchOutcome::Updated);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].external_id.as_deref(), Some("t3_abc123"));
        assert_eq!(result.items[0].author.as_deref(), Some("ml_researcher"));
        assert!(result.items[0].published_at.is_some());
    }
}
