//! Google News RSS search collector strategy.

use async_trait::async_trait;

use crate::collectors::rss::parse_feed_bytes;
use crate::collectors::{execute_conditional_get, CollectorContext, RawHttpOutcome};
use crate::domain::{FetchOutcome, SourceType};
use crate::error::CoreError;
use crate::traits::{ConditionalFetchHeaders, FetchResult, SourceCollector, SourceDescriptor};

const GOOGLE_NEWS_RSS_PATH_SUFFIX: &str = "/rss/search";

/// Collector for Google News RSS keyword searches.
#[derive(Debug, Clone)]
pub struct GoogleNewsCollector {
    ctx: CollectorContext,
}

impl GoogleNewsCollector {
    #[must_use]
    pub fn new(ctx: CollectorContext) -> Self {
        Self { ctx }
    }
}

fn extract_search_query(handle_or_url: &str) -> Result<String, CoreError> {
    let trimmed = handle_or_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "Google News search query must not be empty".to_owned(),
        ));
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let parsed = url::Url::parse(trimmed)
            .map_err(|err| CoreError::InvalidInput(format!("invalid Google News URL: {err}")))?;
        let found = parsed
            .query_pairs()
            .find(|(k, _)| k == "q" || k == "query")
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        return found.ok_or_else(|| {
            CoreError::InvalidInput(
                "Google News URL must contain a non-empty q parameter".to_owned(),
            )
        });
    }

    Ok(trimmed.to_owned())
}

fn build_google_news_url(base_url: &str, query: &str) -> Result<String, CoreError> {
    let base = base_url.trim_end_matches('/');
    let endpoint = if base.ends_with(GOOGLE_NEWS_RSS_PATH_SUFFIX) {
        base.to_owned()
    } else {
        format!("{base}{GOOGLE_NEWS_RSS_PATH_SUFFIX}")
    };
    let mut url = url::Url::parse(&endpoint).map_err(|err| {
        CoreError::InvalidConfig(format!("invalid Google News base URL {endpoint}: {err}"))
    })?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("hl", "en-US")
        .append_pair("gl", "US")
        .append_pair("ceid", "US:en");
    Ok(url.into())
}

#[async_trait]
impl SourceCollector for GoogleNewsCollector {
    fn source_type(&self) -> SourceType {
        SourceType::GoogleNews
    }

    async fn fetch(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
    ) -> Result<FetchResult, CoreError> {
        let query = extract_search_query(&source.handle_or_url)?;
        let url = build_google_news_url(&self.ctx.google_news_base_url, &query)?;

        match execute_conditional_get(&self.ctx, &url, conditional, &[], "google_news").await? {
            RawHttpOutcome::NotModified304(headers) => Ok(FetchResult {
                outcome: FetchOutcome::NotModified304,
                items: Vec::new(),
                etag: headers.etag,
                last_modified: headers.last_modified,
            }),
            RawHttpOutcome::Ok { bytes, headers } => {
                let items = parse_feed_bytes(bytes, "google_news").await?;
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

    const SAMPLE_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Google News - Gemini</title>
    <item>
      <title>Google expands Gemini Enterprise</title>
      <link>https://news.example.org/gemini-enterprise</link>
      <guid>gnews-1</guid>
      <pubDate>Tue, 25 Nov 2026 09:30:00 GMT</pubDate>
      <description>New multimodal enterprise capabilities.</description>
    </item>
  </channel>
</rss>"#;

    #[tokio::test]
    async fn google_news_collector_fetches_and_parses_rss() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rss/search"))
            .and(query_param("q", "Google Gemini"))
            .and(query_param("hl", "en-US"))
            .and(query_param("gl", "US"))
            .and(query_param("ceid", "US:en"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SAMPLE_RSS))
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
        .with_google_news_base_url(server.uri());
        let collector = GoogleNewsCollector::new(ctx);

        let source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::GoogleNews,
            scope: SourceScope::SharedPublic,
            canonical_key: "gnews:Google Gemini".to_owned(),
            name: "Google News Gemini".to_owned(),
            handle_or_url: "Google Gemini".to_owned(),
            category: "News".to_owned(),
        };

        let result = collector
            .fetch(&source, &ConditionalFetchHeaders::default())
            .await
            .unwrap();
        assert_eq!(result.outcome, FetchOutcome::Updated);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].title, "Google expands Gemini Enterprise");
    }
}
