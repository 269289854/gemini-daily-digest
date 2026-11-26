//! Resilient collector decorator providing bounded concurrency semaphore,
//! upfront SSRF checks, exponential retry backoff, and adaptive failure backoff.

use async_trait::async_trait;

use crate::collectors::CollectorContext;
use crate::config::{CollectorTuning, ResourceLimitsTuning};
use crate::constants::DEFAULT_COLLECTOR_BACKOFF_STEPS_SECS;
use crate::domain::SourceType;
use crate::error::CoreError;
use crate::http::global_fetch_semaphore;
use crate::ssrf::SsrfGuard;
use crate::traits::{ConditionalFetchHeaders, FetchResult, SourceCollector, SourceDescriptor};

/// Decorator wrapping any `SourceCollector` with global semaphore throttling,
/// SSRF pre-validation, and exponential retry on transient failures.
#[derive(Debug, Clone)]
pub struct ResilientCollectorDecorator<C> {
    inner: C,
    ssrf: SsrfGuard,
    tuning: CollectorTuning,
    limits: ResourceLimitsTuning,
}

impl<C> ResilientCollectorDecorator<C> {
    /// Creates a new decorator with explicit SSRF guard and tuning parameters.
    #[must_use]
    pub fn new(
        inner: C,
        ssrf: SsrfGuard,
        tuning: CollectorTuning,
        limits: ResourceLimitsTuning,
    ) -> Self {
        Self {
            inner,
            ssrf,
            tuning,
            limits,
        }
    }

    /// Creates a new decorator reusing `CollectorContext` settings.
    #[must_use]
    pub fn with_context(inner: C, ctx: CollectorContext) -> Self {
        Self {
            inner,
            ssrf: ctx.ssrf,
            tuning: ctx.tuning,
            limits: ctx.limits,
        }
    }
}

/// Computes adaptive failure backoff (`1h -> 4h -> 12h -> 24h`) from `tuning.failure_backoff_steps`.
#[must_use]
pub fn next_failure_backoff(
    consecutive_failures: u32,
    tuning: &CollectorTuning,
) -> chrono::Duration {
    if consecutive_failures == 0 {
        return chrono::Duration::zero();
    }

    let step = if let Some(last) = tuning.failure_backoff_steps.last() {
        let idx = (consecutive_failures as usize)
            .saturating_sub(1)
            .min(tuning.failure_backoff_steps.len().saturating_sub(1));
        tuning
            .failure_backoff_steps
            .get(idx)
            .copied()
            .unwrap_or(*last)
    } else {
        let idx = (consecutive_failures as usize)
            .saturating_sub(1)
            .min(DEFAULT_COLLECTOR_BACKOFF_STEPS_SECS.len().saturating_sub(1));
        std::time::Duration::from_secs(DEFAULT_COLLECTOR_BACKOFF_STEPS_SECS[idx])
    };

    chrono::Duration::from_std(step).unwrap_or_else(|_| chrono::Duration::hours(24))
}

fn is_retryable_error(err: &CoreError) -> bool {
    match err {
        CoreError::UpstreamTimeout(_) => true,
        CoreError::Upstream(msg) => {
            !msg.contains("exceeds cap of") && !msg.contains("exceeded cap of")
        }
        _ => false,
    }
}

#[async_trait]
impl<C: SourceCollector> SourceCollector for ResilientCollectorDecorator<C> {
    fn source_type(&self) -> SourceType {
        self.inner.source_type()
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
        let trimmed = source.handle_or_url.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            self.ssrf.validate_url(trimmed).await?;
        }

        let max_attempts = self.tuning.max_retry_attempts.get();
        let semaphore = global_fetch_semaphore(self.limits.global_fetch_concurrency.get());

        for attempt in 0..max_attempts {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|err| CoreError::Internal(format!("fetch semaphore closed: {err}")))?;

            let result = self
                .inner
                .fetch_with_credentials(source, conditional, credentials_json)
                .await;

            drop(permit);

            match result {
                Ok(fetch_result) => return Ok(fetch_result),
                Err(err) => {
                    let has_more_attempts = attempt + 1 < max_attempts;
                    if has_more_attempts && is_retryable_error(&err) {
                        let multiplier = 1u32.checked_shl(attempt as u32).unwrap_or(u32::MAX);
                        let delay = self.tuning.initial_backoff.saturating_mul(multiplier);
                        tokio::time::sleep(delay).await;
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        Err(CoreError::Internal(
            "retry loop exited without terminal result".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::rss::RssCollector;
    use crate::domain::{FetchOutcome, SourceScope};
    use crate::http::shared_http_client;
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    struct FlakyRssResponder {
        attempts: Arc<AtomicUsize>,
    }

    impl Respond for FlakyRssResponder {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            let call = self.attempts.fetch_add(1, Ordering::SeqCst);
            if call < 2 {
                ResponseTemplate::new(503).set_body_string("temporary outage")
            } else {
                ResponseTemplate::new(200).set_body_string(
                    r#"<?xml version="1.0"?><rss version="2.0"><channel><title>OK</title><item><title>Recovered</title><link>https://example.org/ok</link></item></channel></rss>"#,
                )
            }
        }
    }

    #[tokio::test]
    async fn resilient_decorator_retries_transient_503_and_succeeds() {
        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/flaky.xml"))
            .respond_with(FlakyRssResponder {
                attempts: attempts.clone(),
            })
            .mount(&server)
            .await;

        let tuning = CollectorTuning {
            max_retry_attempts: NonZeroUsize::new(3).unwrap(),
            initial_backoff: std::time::Duration::from_millis(5),
            ..CollectorTuning::default()
        };

        let client = shared_http_client(None, &tuning).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let ctx = CollectorContext::new(
            client,
            ssrf.clone(),
            tuning.clone(),
            ResourceLimitsTuning::default(),
        );
        let decorator = ResilientCollectorDecorator::with_context(
            RssCollector::new(ctx),
            CollectorContext {
                client: shared_http_client(None, &tuning).unwrap(),
                ssrf,
                tuning,
                limits: ResourceLimitsTuning::default(),
                github_api_base_url: String::new(),
                reddit_base_url: String::new(),
                hacker_news_base_url: String::new(),
                google_news_base_url: String::new(),
                x_api_base_url: String::new(),
            },
        );

        let source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            canonical_key: "rss:flaky".to_owned(),
            name: "Flaky".to_owned(),
            handle_or_url: format!("{}/flaky.xml", server.uri()),
            category: "Test".to_owned(),
        };

        let result = decorator
            .fetch(&source, &ConditionalFetchHeaders::default())
            .await
            .unwrap();
        assert_eq!(result.outcome, FetchOutcome::Updated);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].title, "Recovered");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn resilient_decorator_retries_429_and_does_not_retry_404() {
        let server = MockServer::start().await;
        let rate_limit_attempts = Arc::new(AtomicUsize::new(0));
        let rate_limit_clone = rate_limit_attempts.clone();
        Mock::given(method("GET"))
            .and(path("/rate-limited.xml"))
            .respond_with(move |_req: &Request| {
                if rate_limit_clone.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(429).set_body_string("slow down")
                } else {
                    ResponseTemplate::new(200).set_body_string(
                        r#"<?xml version="1.0"?><rss version="2.0"><channel><title>OK</title><item><title>After429</title><link>https://example.org/429</link></item></channel></rss>"#,
                    )
                }
            })
            .mount(&server)
            .await;

        let not_found_attempts = Arc::new(AtomicUsize::new(0));
        let not_found_clone = not_found_attempts.clone();
        Mock::given(method("GET"))
            .and(path("/missing.xml"))
            .respond_with(move |_req: &Request| {
                not_found_clone.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(404)
            })
            .mount(&server)
            .await;

        let tuning = CollectorTuning {
            max_retry_attempts: NonZeroUsize::new(3).unwrap(),
            initial_backoff: std::time::Duration::from_millis(5),
            ..CollectorTuning::default()
        };
        let client = shared_http_client(None, &tuning).unwrap();
        let ssrf = SsrfGuard::new(vec!["127.0.0.1".to_owned()]);
        let ctx = CollectorContext::new(client, ssrf, tuning, ResourceLimitsTuning::default());
        let decorator =
            ResilientCollectorDecorator::with_context(RssCollector::new(ctx.clone()), ctx);

        let ok_source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            canonical_key: "rss:429".to_owned(),
            name: "Rate Limited".to_owned(),
            handle_or_url: format!("{}/rate-limited.xml", server.uri()),
            category: "Test".to_owned(),
        };
        let res = decorator
            .fetch(&ok_source, &ConditionalFetchHeaders::default())
            .await
            .unwrap();
        assert_eq!(res.items[0].title, "After429");
        assert_eq!(rate_limit_attempts.load(Ordering::SeqCst), 2);

        let missing_source = SourceDescriptor {
            handle_or_url: format!("{}/missing.xml", server.uri()),
            ..ok_source
        };
        let err = decorator
            .fetch(&missing_source, &ConditionalFetchHeaders::default())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
        assert_eq!(not_found_attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn next_failure_backoff_follows_adaptive_schedule() {
        let tuning = CollectorTuning::default();
        assert_eq!(next_failure_backoff(0, &tuning), chrono::Duration::zero());
        assert_eq!(next_failure_backoff(1, &tuning), chrono::Duration::hours(1));
        assert_eq!(next_failure_backoff(2, &tuning), chrono::Duration::hours(4));
        assert_eq!(
            next_failure_backoff(3, &tuning),
            chrono::Duration::hours(12)
        );
        assert_eq!(
            next_failure_backoff(4, &tuning),
            chrono::Duration::hours(24)
        );
        assert_eq!(
            next_failure_backoff(9, &tuning),
            chrono::Duration::hours(24)
        );
    }
}
