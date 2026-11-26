//! Strategy collectors (`rss`, `github_release`, `reddit`, `hacker_news`,
//! `google_news`, compliant `x_account`/`x_search`), `CollectorRegistry`,
//! and `ResilientCollectorDecorator` (ETag/304, retry, adaptive backoff).

pub mod github_release;
pub mod google_news;
pub mod hacker_news;
pub mod reddit;
pub mod resilient;
pub mod rss;
pub mod x_compliant;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use reqwest::StatusCode;

use crate::config::{AppConfig, CollectorTuning, ResourceLimitsTuning};
use crate::constants::{
    DEFAULT_GITHUB_API_BASE_URL, DEFAULT_GOOGLE_NEWS_RSS_BASE_URL,
    DEFAULT_HACKER_NEWS_ALGOLIA_BASE_URL, DEFAULT_REDDIT_BASE_URL, DEFAULT_X_API_BASE_URL,
    HEADER_AUTHORIZATION, HEADER_LOCATION,
};
use crate::domain::SourceType;
use crate::error::CoreError;
use crate::http::{
    apply_conditional_headers, extract_conditional_headers, parse_http_status_error,
    read_response_bytes_capped, shared_http_client,
};
use crate::ssrf::SsrfGuard;
use crate::traits::{ConditionalFetchHeaders, FetchResult, SourceCollector, SourceDescriptor};

pub use github_release::GitHubReleaseCollector;
pub use google_news::GoogleNewsCollector;
pub use hacker_news::HackerNewsCollector;
pub use reddit::RedditCollector;
pub use resilient::{next_failure_backoff, ResilientCollectorDecorator};
pub use rss::RssCollector;
pub use x_compliant::XCompliantCollector;

/// Shared runtime dependencies injected into upstream collectors.
#[derive(Debug, Clone)]
pub struct CollectorContext {
    pub client: reqwest::Client,
    pub ssrf: SsrfGuard,
    pub tuning: CollectorTuning,
    pub limits: ResourceLimitsTuning,
    pub github_api_base_url: String,
    pub reddit_base_url: String,
    pub hacker_news_base_url: String,
    pub google_news_base_url: String,
    pub x_api_base_url: String,
}

impl CollectorContext {
    /// Builds a `CollectorContext` from `AppConfig` using the shared HTTP/2 singleton.
    pub fn from_config(config: &AppConfig) -> Result<Self, CoreError> {
        let client = shared_http_client(config.outbound_proxy_url.as_deref(), &config.collector)?;
        let ssrf = SsrfGuard::from_config(config);
        Ok(Self::new(
            client,
            ssrf,
            config.collector.clone(),
            config.resources.clone(),
        ))
    }

    /// Creates a `CollectorContext` with explicit dependencies and default upstream base URLs.
    #[must_use]
    pub fn new(
        client: reqwest::Client,
        ssrf: SsrfGuard,
        tuning: CollectorTuning,
        limits: ResourceLimitsTuning,
    ) -> Self {
        Self {
            client,
            ssrf,
            tuning,
            limits,
            github_api_base_url: DEFAULT_GITHUB_API_BASE_URL.to_owned(),
            reddit_base_url: DEFAULT_REDDIT_BASE_URL.to_owned(),
            hacker_news_base_url: DEFAULT_HACKER_NEWS_ALGOLIA_BASE_URL.to_owned(),
            google_news_base_url: DEFAULT_GOOGLE_NEWS_RSS_BASE_URL.to_owned(),
            x_api_base_url: DEFAULT_X_API_BASE_URL.to_owned(),
        }
    }

    #[must_use]
    pub fn with_github_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.github_api_base_url = url.into();
        self
    }

    #[must_use]
    pub fn with_reddit_base_url(mut self, url: impl Into<String>) -> Self {
        self.reddit_base_url = url.into();
        self
    }

    #[must_use]
    pub fn with_hacker_news_base_url(mut self, url: impl Into<String>) -> Self {
        self.hacker_news_base_url = url.into();
        self
    }

    #[must_use]
    pub fn with_google_news_base_url(mut self, url: impl Into<String>) -> Self {
        self.google_news_base_url = url.into();
        self
    }

    #[must_use]
    pub fn with_x_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.x_api_base_url = url.into();
        self
    }
}

pub(crate) enum RawHttpOutcome {
    NotModified304(ConditionalFetchHeaders),
    Ok {
        bytes: Bytes,
        headers: ConditionalFetchHeaders,
    },
}

fn is_redirect_status(status: StatusCode) -> bool {
    status.is_redirection()
        && matches!(
            status,
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        )
}

pub(crate) async fn execute_conditional_get(
    ctx: &CollectorContext,
    raw_url: &str,
    conditional: &ConditionalFetchHeaders,
    extra_headers: &[(&str, &str)],
    context_label: &str,
) -> Result<RawHttpOutcome, CoreError> {
    let initial_url = ctx.ssrf.validate_url(raw_url).await?;
    let initial_origin = initial_url.origin();
    let mut current_url = initial_url;

    for hop in 0..=ctx.tuning.max_redirects {
        let same_origin = hop == 0 || current_url.origin() == initial_origin;
        let mut request = ctx
            .client
            .get(current_url.clone())
            .timeout(ctx.tuning.request_timeout);
        if hop == 0 {
            request = apply_conditional_headers(request, conditional);
        }
        for (name, value) in extra_headers {
            if same_origin || !name.eq_ignore_ascii_case(HEADER_AUTHORIZATION) {
                request = request.header(*name, *value);
            }
        }

        let response = request.send().await.map_err(|err| {
            if err.is_timeout() {
                CoreError::UpstreamTimeout(format!("{context_label} request timed out: {err}"))
            } else {
                CoreError::Upstream(format!("{context_label} request failed: {err}"))
            }
        })?;

        let resp_headers = extract_conditional_headers(response.headers());
        let status = response.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(RawHttpOutcome::NotModified304(ConditionalFetchHeaders {
                etag: resp_headers.etag.or_else(|| conditional.etag.clone()),
                last_modified: resp_headers
                    .last_modified
                    .or_else(|| conditional.last_modified.clone()),
            }));
        }

        if is_redirect_status(status) {
            if hop >= ctx.tuning.max_redirects {
                return Err(CoreError::Upstream("too many redirects".to_owned()));
            }
            let location_header = response.headers().get(HEADER_LOCATION).ok_or_else(|| {
                CoreError::Upstream(format!(
                    "{context_label} redirect HTTP {} missing Location header",
                    status.as_u16()
                ))
            })?;
            let location = location_header
                .to_str()
                .map_err(|_| {
                    CoreError::Upstream(format!(
                        "{context_label} redirect Location header contains invalid UTF-8"
                    ))
                })?
                .trim();
            if location.is_empty() {
                return Err(CoreError::Upstream(format!(
                    "{context_label} redirect Location header is empty"
                )));
            }
            let next_url = current_url.join(location).map_err(|err| {
                CoreError::Upstream(format!(
                    "{context_label} invalid redirect Location '{location}': {err}"
                ))
            })?;
            current_url = ctx.ssrf.validate_url(next_url.as_str()).await?;
            continue;
        }

        if !status.is_success() {
            return Err(parse_http_status_error(status, context_label));
        }

        let bytes = read_response_bytes_capped(response, ctx.limits.max_feed_bytes.get()).await?;
        return Ok(RawHttpOutcome::Ok {
            bytes,
            headers: resp_headers,
        });
    }

    Err(CoreError::Upstream("too many redirects".to_owned()))
}

#[async_trait]
impl SourceCollector for Arc<dyn SourceCollector> {
    fn source_type(&self) -> SourceType {
        (**self).source_type()
    }

    async fn fetch(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
    ) -> Result<FetchResult, CoreError> {
        (**self).fetch(source, conditional).await
    }

    async fn fetch_with_credentials(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
        credentials_json: Option<&serde_json::Value>,
    ) -> Result<FetchResult, CoreError> {
        (**self)
            .fetch_with_credentials(source, conditional, credentials_json)
            .await
    }
}

/// Strategy registry mapping every `SourceType` to a `SourceCollector`.
pub struct CollectorRegistry {
    collectors: HashMap<SourceType, Arc<dyn SourceCollector>>,
}

impl CollectorRegistry {
    /// Creates a registry containing all 7 `SourceType`s wrapped with `ResilientCollectorDecorator`.
    #[must_use]
    pub fn new(ctx: CollectorContext) -> Self {
        let mut registry = Self {
            collectors: HashMap::new(),
        };

        let wrap = |collector: Arc<dyn SourceCollector>, c: &CollectorContext| {
            Arc::new(ResilientCollectorDecorator::with_context(
                collector,
                c.clone(),
            )) as Arc<dyn SourceCollector>
        };

        registry.register(wrap(Arc::new(RssCollector::new(ctx.clone())), &ctx));
        registry.register(wrap(
            Arc::new(GitHubReleaseCollector::new(ctx.clone())),
            &ctx,
        ));
        registry.register(wrap(Arc::new(RedditCollector::new(ctx.clone())), &ctx));
        registry.register(wrap(Arc::new(HackerNewsCollector::new(ctx.clone())), &ctx));
        registry.register(wrap(Arc::new(GoogleNewsCollector::new(ctx.clone())), &ctx));
        registry.register(wrap(
            Arc::new(XCompliantCollector::for_type(
                ctx.clone(),
                SourceType::XAccount,
            )),
            &ctx,
        ));
        registry.register(wrap(
            Arc::new(XCompliantCollector::for_type(
                ctx.clone(),
                SourceType::XSearch,
            )),
            &ctx,
        ));

        registry
    }

    /// Creates a registry built directly from `AppConfig`.
    pub fn from_config(config: &AppConfig) -> Result<Self, CoreError> {
        let ctx = CollectorContext::from_config(config)?;
        Ok(Self::new(ctx))
    }

    /// Registers or replaces a collector for its `source_type()`.
    pub fn register(&mut self, collector: Arc<dyn SourceCollector>) {
        self.collectors.insert(collector.source_type(), collector);
    }

    /// Looks up a registered collector by `SourceType`.
    #[must_use]
    pub fn get(&self, source_type: SourceType) -> Option<Arc<dyn SourceCollector>> {
        self.collectors.get(&source_type).cloned()
    }

    /// Dispatches a fetch for `source` through the registered collector.
    pub async fn fetch(
        &self,
        source: &SourceDescriptor,
        conditional: &ConditionalFetchHeaders,
        credentials_json: Option<&serde_json::Value>,
    ) -> Result<FetchResult, CoreError> {
        let collector = self.get(source.source_type).ok_or_else(|| {
            CoreError::NotFound(format!(
                "no collector registered for source type {}",
                source.source_type
            ))
        })?;
        collector
            .fetch_with_credentials(source, conditional, credentials_json)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{FetchOutcome, SourceScope};
    use uuid::Uuid;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_ctx(server: &MockServer) -> CollectorContext {
        let tuning = CollectorTuning {
            initial_backoff: std::time::Duration::from_millis(5),
            ..CollectorTuning::default()
        };
        let client = shared_http_client(None, &tuning).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()]);
        CollectorContext::new(client, ssrf, tuning, ResourceLimitsTuning::default())
            .with_github_api_base_url(server.uri())
            .with_reddit_base_url(server.uri())
            .with_hacker_news_base_url(server.uri())
            .with_google_news_base_url(server.uri())
            .with_x_api_base_url(server.uri())
    }

    fn make_source(source_type: SourceType, handle_or_url: &str) -> SourceDescriptor {
        SourceDescriptor {
            id: Uuid::new_v4(),
            source_type,
            scope: SourceScope::SharedPublic,
            canonical_key: format!("{source_type}:test"),
            name: "Test Source".to_owned(),
            handle_or_url: handle_or_url.to_owned(),
            category: "Test".to_owned(),
        }
    }

    #[test]
    fn registry_covers_all_seven_source_types() {
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let client = shared_http_client(None, &CollectorTuning::default()).unwrap();
        let ctx = CollectorContext::new(
            client,
            ssrf,
            CollectorTuning::default(),
            ResourceLimitsTuning::default(),
        );
        let registry = CollectorRegistry::new(ctx);
        for source_type in SourceType::all() {
            assert!(
                registry.get(*source_type).is_some(),
                "missing collector for {source_type}"
            );
        }
    }

    #[tokio::test]
    async fn registry_supports_conditional_304_short_circuit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .and(header("if-none-match", "\"etag-1\""))
            .respond_with(
                ResponseTemplate::new(304)
                    .insert_header("etag", "\"etag-1\"")
                    .insert_header("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT"),
            )
            .mount(&server)
            .await;

        let registry = CollectorRegistry::new(test_ctx(&server));
        let source = make_source(SourceType::Rss, &format!("{}/feed.xml", server.uri()));
        let conditional = ConditionalFetchHeaders {
            etag: Some("\"etag-1\"".to_owned()),
            last_modified: None,
        };

        let result = registry.fetch(&source, &conditional, None).await.unwrap();
        assert_eq!(result.outcome, FetchOutcome::NotModified304);
        assert!(result.items.is_empty());
        assert_eq!(result.etag.as_deref(), Some("\"etag-1\""));
    }

    #[tokio::test]
    async fn registry_blocks_ssrf_private_url_before_http_call() {
        let server = MockServer::start().await;
        let mut ctx = test_ctx(&server);
        ctx.ssrf = SsrfGuard::new(Vec::new());
        let registry = CollectorRegistry::new(ctx);
        let source = make_source(SourceType::Rss, "http://169.254.169.254/latest/meta-data/");
        let err = registry
            .fetch(&source, &ConditionalFetchHeaders::default(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::SsrfBlocked(_)));
    }

    #[tokio::test]
    async fn collector_blocks_302_redirect_to_non_allowlisted_ip_with_zero_target_hits() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let target_listener = TcpListener::bind("127.0.0.2:0").await.unwrap();
        let target_addr = target_listener.local_addr().unwrap();
        let target_hits = Arc::new(AtomicUsize::new(0));
        let target_hits_clone = target_hits.clone();

        let target_task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = target_listener.accept().await {
                target_hits_clone.fetch_add(1, Ordering::SeqCst);
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            }
        });

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/redirect-ssrf"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("http://{target_addr}/internal.xml")),
            )
            .mount(&server)
            .await;

        // Only 127.0.0.1 is allowlisted; 127.0.0.2 must be blocked on redirect.
        let mut ctx = test_ctx(&server);
        ctx.ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let collector = RssCollector::new(ctx);
        let source = make_source(SourceType::Rss, &format!("{}/redirect-ssrf", server.uri()));

        let err = collector
            .fetch(&source, &ConditionalFetchHeaders::default())
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::SsrfBlocked(_)),
            "expected SsrfBlocked on redirect to 127.0.0.2, got {err:?}"
        );

        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        target_task.abort();
        assert_eq!(target_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn collector_blocks_302_redirect_to_metadata_and_enforces_max_redirects() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/redirect-metadata"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/start.xml"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/final.xml"))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/final.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<?xml version="1.0"?><rss version="2.0"><channel><title>OK</title><item><title>Redirected Item</title><link>https://example.org/item</link></item></channel></rss>"#,
            ))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/loop"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
            .mount(&server)
            .await;

        let mut ctx = test_ctx(&server);
        ctx.tuning.max_redirects = 2;
        let collector = RssCollector::new(ctx);

        let metadata_source = make_source(
            SourceType::Rss,
            &format!("{}/redirect-metadata", server.uri()),
        );
        let metadata_err = collector
            .fetch(&metadata_source, &ConditionalFetchHeaders::default())
            .await
            .unwrap_err();
        assert!(matches!(metadata_err, CoreError::SsrfBlocked(_)));

        let ok_source = make_source(SourceType::Rss, &format!("{}/start.xml", server.uri()));
        let ok_res = collector
            .fetch(&ok_source, &ConditionalFetchHeaders::default())
            .await
            .unwrap();
        assert_eq!(ok_res.items.len(), 1);
        assert_eq!(ok_res.items[0].title, "Redirected Item");

        let loop_source = make_source(SourceType::Rss, &format!("{}/loop", server.uri()));
        let loop_err = collector
            .fetch(&loop_source, &ConditionalFetchHeaders::default())
            .await
            .unwrap_err();
        assert!(
            matches!(loop_err, CoreError::Upstream(ref msg) if msg.contains("too many redirects")),
            "expected too many redirects error, got {loop_err:?}"
        );
    }
}
