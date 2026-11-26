//! RSS / Atom / JSON Feed collector strategy powered by `feed_rs`.

use async_trait::async_trait;
use bytes::Bytes;

use crate::collectors::{execute_conditional_get, CollectorContext, RawHttpOutcome};
use crate::domain::{FetchOutcome, SourceType};
use crate::error::CoreError;
use crate::traits::{
    ConditionalFetchHeaders, FetchResult, RawCollectedItem, SourceCollector, SourceDescriptor,
};

/// Collector for standard RSS, Atom, and JSON Feed URLs.
#[derive(Debug, Clone)]
pub struct RssCollector {
    ctx: CollectorContext,
}

impl RssCollector {
    #[must_use]
    pub fn new(ctx: CollectorContext) -> Self {
        Self { ctx }
    }
}

pub(crate) async fn parse_feed_bytes(
    bytes: Bytes,
    context_label: &str,
) -> Result<Vec<RawCollectedItem>, CoreError> {
    let label = context_label.to_owned();
    tokio::task::spawn_blocking(move || {
        let feed = feed_rs::parser::parse(bytes.as_ref())
            .map_err(|err| CoreError::Upstream(format!("failed parsing {label} feed: {err}")))?;

        let items = feed
            .entries
            .into_iter()
            .filter_map(|entry| {
                let link = entry
                    .links
                    .iter()
                    .find(|l| l.rel.as_deref() == Some("alternate"))
                    .or_else(|| entry.links.first())
                    .map(|l| l.href.trim().to_owned())
                    .filter(|href| !href.is_empty())
                    .or_else(|| {
                        let id = entry.id.trim();
                        if id.starts_with("http://") || id.starts_with("https://") {
                            Some(id.to_owned())
                        } else {
                            None
                        }
                    })?;

                let title = entry
                    .title
                    .map(|t| t.content.trim().to_owned())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| link.clone());

                let author = entry
                    .authors
                    .into_iter()
                    .map(|p| p.name.trim().to_owned())
                    .find(|name| !name.is_empty());

                let summary = entry
                    .summary
                    .map(|s| s.content.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        entry
                            .content
                            .and_then(|c| c.body)
                            .map(|b| b.trim().to_owned())
                            .filter(|b| !b.is_empty())
                    });

                let external_id = Some(entry.id.trim().to_owned()).filter(|id| !id.is_empty());
                let published_at = entry.published.or(entry.updated);

                Some(RawCollectedItem {
                    external_id,
                    url: link,
                    title,
                    author,
                    summary,
                    published_at,
                })
            })
            .collect();

        Ok(items)
    })
    .await
    .map_err(|err| CoreError::Internal(format!("feed parser task failed: {err}")))?
}

#[async_trait]
impl SourceCollector for RssCollector {
    fn source_type(&self) -> SourceType {
        SourceType::Rss
    }

    async fn fetch(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
    ) -> Result<FetchResult, CoreError> {
        let feed_url = source.handle_or_url.trim();
        if feed_url.is_empty() {
            return Err(CoreError::InvalidInput(
                "RSS source URL must not be empty".to_owned(),
            ));
        }

        match execute_conditional_get(&self.ctx, feed_url, conditional, &[], "rss").await? {
            RawHttpOutcome::NotModified304(headers) => Ok(FetchResult {
                outcome: FetchOutcome::NotModified304,
                items: Vec::new(),
                etag: headers.etag,
                last_modified: headers.last_modified,
            }),
            RawHttpOutcome::Ok { bytes, headers } => {
                let items = parse_feed_bytes(bytes, "rss").await?;
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
    use std::num::NonZeroUsize;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SAMPLE_ATOM_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Gemini Official Blog</title>
  <entry>
    <id>tag:example.org,2026:gemini-2-5</id>
    <title>Gemini 2.5 Pro Release</title>
    <link rel="alternate" href="https://example.org/blog/gemini-2-5?utm_source=rss" />
    <author><name>DeepMind Team</name></author>
    <updated>2026-11-25T10:00:00Z</updated>
    <summary>Major reasoning and coding improvements.</summary>
  </entry>
</feed>"#;

    #[tokio::test]
    async fn rss_collector_parses_atom_and_enforces_size_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/atom.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "\"v1\"")
                    .set_body_string(SAMPLE_ATOM_XML),
            )
            .mount(&server)
            .await;

        let client = shared_http_client(None, &CollectorTuning::default()).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let ctx = CollectorContext::new(
            client.clone(),
            ssrf.clone(),
            CollectorTuning::default(),
            ResourceLimitsTuning::default(),
        );
        let collector = RssCollector::new(ctx);

        let source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            canonical_key: "rss:test".to_owned(),
            name: "Gemini Blog".to_owned(),
            handle_or_url: format!("{}/atom.xml", server.uri()),
            category: "Official".to_owned(),
        };

        let result = collector
            .fetch(&source, &ConditionalFetchHeaders::default())
            .await
            .unwrap();
        assert_eq!(result.outcome, FetchOutcome::Updated);
        assert_eq!(result.etag.as_deref(), Some("\"v1\""));
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].title, "Gemini 2.5 Pro Release");
        assert_eq!(result.items[0].author.as_deref(), Some("DeepMind Team"));
        assert_eq!(
            result.items[0].summary.as_deref(),
            Some("Major reasoning and coding improvements.")
        );

        let tight_limits = ResourceLimitsTuning {
            max_feed_bytes: NonZeroUsize::new(64).unwrap(),
            ..ResourceLimitsTuning::default()
        };
        let tight_ctx =
            CollectorContext::new(client, ssrf, CollectorTuning::default(), tight_limits);
        let tight_collector = RssCollector::new(tight_ctx);
        let err = tight_collector
            .fetch(&source, &ConditionalFetchHeaders::default())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Upstream(_)));
    }
}
