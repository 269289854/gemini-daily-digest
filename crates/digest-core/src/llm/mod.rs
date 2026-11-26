//! LLM summarization strategies (`OpenAiCompatProvider`, `DeterministicFallbackSummarizer`)
//! and fingerprint-based idempotency caching (`IdempotentLlmRunner`).

pub mod fallback;
pub mod fingerprint;
pub mod openai_compat;

pub use fallback::DeterministicFallbackSummarizer;
pub use fingerprint::{
    compute_input_fingerprint, compute_source_input_fingerprint, IdempotentDigestResult,
    IdempotentLlmRunner,
};
pub use openai_compat::OpenAiCompatProvider;

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::NaiveDate;
    use uuid::Uuid;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cache::InMemoryCacheCoordinator;
    use crate::constants::FALLBACK_MODEL_LABEL;
    use crate::domain::{
        AlertPriority, DailyDigestSynthesisInput, ItemMatchResult, ProcessedItem,
        SourceDigestInput, SourceScope, SourceType, UserAiConfig,
    };
    use crate::error::CoreError;
    use crate::ssrf::SsrfGuard;
    use crate::traits::{SourceDescriptor, StructuredLlmProvider};

    fn sample_item(title: &str, url: &str, hash: &str, priority: AlertPriority) -> ProcessedItem {
        ProcessedItem {
            external_id: Some(hash.to_owned()),
            canonical_url: url.to_owned(),
            content_hash: hash.to_owned(),
            title: title.to_owned(),
            author: Some("tester".to_owned()),
            summary: Some("Gemini SDK release details and benchmarks.".to_owned()),
            published_at: None,
            source_id: None,
            source_name: Some("Official Blog".to_owned()),
            category: Some("Official".to_owned()),
            match_result: ItemMatchResult {
                matched_keywords: if priority == AlertPriority::Normal {
                    Vec::new()
                } else {
                    vec!["gemini-2.5".to_owned()]
                },
                priority,
                importance_score: 50,
                notify_immediately: priority == AlertPriority::Urgent,
            },
        }
    }

    fn sample_ai_config(base_url: &str) -> UserAiConfig {
        UserAiConfig {
            provider_label: "DeepSeek".to_owned(),
            base_url: base_url.to_owned(),
            api_key: Some("test-key-placeholder".to_owned()),
            model: "deepseek-chat".to_owned(),
            system_prompt: "Summarize accurately.".to_owned(),
            temperature: 0.2,
            max_tokens: 1024,
        }
    }

    #[test]
    fn normalizes_base_url_with_or_without_v1_suffix() {
        assert_eq!(
            OpenAiCompatProvider::build_chat_completions_url("https://api.deepseek.com").unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            OpenAiCompatProvider::build_chat_completions_url("https://api.deepseek.com/").unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            OpenAiCompatProvider::build_chat_completions_url("https://api.deepseek.com/v1")
                .unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            OpenAiCompatProvider::build_chat_completions_url("https://api.deepseek.com/v1/")
                .unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            OpenAiCompatProvider::build_chat_completions_url(
                "https://api.deepseek.com/v1/chat/completions"
            )
            .unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            OpenAiCompatProvider::build_chat_completions_url(
                "https://example.com/v1beta/openai/chat/completions"
            )
            .unwrap(),
            "https://example.com/v1beta/openai/chat/completions"
        );
        assert_eq!(
            OpenAiCompatProvider::build_chat_completions_url(
                "https://example.com/openai/chat/completions/"
            )
            .unwrap(),
            "https://example.com/openai/chat/completions"
        );
    }

    #[test]
    fn fingerprint_is_deterministic_across_item_order_and_ignores_api_key() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let item_a = sample_item(
            "Alpha",
            "https://example.com/a",
            "hash-a",
            AlertPriority::Normal,
        );
        let item_b = sample_item(
            "Beta",
            "https://example.com/b",
            "hash-b",
            AlertPriority::Urgent,
        );

        let mut cfg1 = sample_ai_config("https://api.deepseek.com");
        cfg1.api_key = Some("test-key-placeholder-one".to_owned());
        let mut cfg2 = sample_ai_config("https://api.deepseek.com/v1");
        cfg2.api_key = Some("test-key-placeholder-two".to_owned());

        let fp1 = compute_input_fingerprint(date, &cfg1, &[item_a.clone(), item_b.clone()]);
        let fp2 = compute_input_fingerprint(date, &cfg2, &[item_b, item_a]);
        assert_eq!(fp1, fp2);
        assert_eq!(fp1.len(), 64);

        let mut cfg_different_model = cfg1.clone();
        cfg_different_model.model = "other-model".to_owned();
        let fp3 = compute_input_fingerprint(date, &cfg_different_model, &[]);
        assert_ne!(fp1, fp3);
    }

    #[tokio::test]
    async fn openai_compat_parses_source_daily_and_test_connection() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer test-key-placeholder"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "deepseek-chat",
                "choices": [{
                    "message": {
                        "content": "## Executive Summary\n- Gemini 2.5 Pro update shipped."
                    }
                }],
                "usage": {
                    "prompt_tokens": 120,
                    "completion_tokens": 45
                }
            })))
            .mount(&server)
            .await;

        let provider = OpenAiCompatProvider::new(
            reqwest::Client::new(),
            SsrfGuard::new(vec!["127.0.0.1".to_owned()]),
        );

        // Exercise both base URL without /v1 and base URL ending in /v1.
        let config_no_v1 = sample_ai_config(&server.uri());
        let config_with_v1 = sample_ai_config(&format!("{}/v1", server.uri()));
        let date = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();

        let source_input = SourceDigestInput {
            source: SourceDescriptor {
                id: Uuid::new_v4(),
                source_type: SourceType::Rss,
                scope: SourceScope::SharedPublic,
                canonical_key: "gemini-blog".to_owned(),
                name: "Google Gemini Blog".to_owned(),
                handle_or_url: "https://example.com/feed.xml".to_owned(),
                category: "Official".to_owned(),
            },
            digest_date: date,
            items: vec![sample_item(
                "Gemini 2.5 Pro Launch",
                "https://example.com/launch",
                "hash-1",
                AlertPriority::Urgent,
            )],
        };

        let source_res = provider
            .summarize_source(&config_with_v1, &source_input)
            .await
            .unwrap();
        assert_eq!(source_res.prompt_tokens, 120);
        assert_eq!(source_res.completion_tokens, 45);
        assert!(!source_res.degraded_fallback);
        assert_eq!(source_res.key_points.len(), 1);

        let daily_input = DailyDigestSynthesisInput {
            digest_date: date,
            source_summaries: vec![source_res],
            highlighted_items: source_input.items.clone(),
            all_items: source_input.items,
        };

        let daily = provider
            .synthesize_daily(&config_no_v1, &daily_input)
            .await
            .unwrap();
        assert!(!daily.degraded_fallback);
        assert_eq!(daily.prompt_tokens, 120);
        assert_eq!(daily.completion_tokens, 45);
        assert!(daily.overview_markdown.contains("Executive Summary"));

        provider.test_connection(&config_no_v1).await.unwrap();
    }

    #[tokio::test]
    async fn openai_compat_blocks_private_base_url_via_ssrf_guard() {
        let provider =
            OpenAiCompatProvider::new(reqwest::Client::new(), SsrfGuard::new(Vec::new()));
        let config = sample_ai_config("http://127.0.0.1:8080");

        let err = provider.test_connection(&config).await.unwrap_err();
        assert!(matches!(err, CoreError::SsrfBlocked(_)));
    }

    #[tokio::test]
    async fn idempotent_runner_caches_hit_with_zero_tokens_and_falls_back_on_500_or_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "deepseek-chat",
                "choices": [{
                    "message": { "content": "Daily summary ok." }
                }],
                "usage": { "prompt_tokens": 80, "completion_tokens": 30 }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cache = Arc::new(InMemoryCacheCoordinator::new());
        let provider = Arc::new(OpenAiCompatProvider::new(
            reqwest::Client::new(),
            SsrfGuard::new(vec!["127.0.0.1".to_owned()]),
        ));
        let runner = IdempotentLlmRunner::new(cache, provider);

        let date = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let item = sample_item(
            "Gemini CLI Update",
            "https://example.com/cli",
            "hash-cli",
            AlertPriority::High,
        );
        let input = DailyDigestSynthesisInput {
            digest_date: date,
            source_summaries: Vec::new(),
            highlighted_items: vec![item.clone()],
            all_items: vec![item.clone()],
        };
        let config = sample_ai_config(&server.uri());

        let first = runner
            .summarize_daily_idempotent(&config, &input)
            .await
            .unwrap();
        assert_eq!(first.prompt_tokens, 80);
        assert_eq!(first.completion_tokens, 30);
        assert!(!first.degraded_fallback);

        // Second call with identical fingerprint hits cache with 0 tokens and no extra HTTP call.
        let second = runner
            .summarize_daily_idempotent(&config, &input)
            .await
            .unwrap();
        assert_eq!(second.prompt_tokens, 0);
        assert_eq!(second.completion_tokens, 0);
        assert_eq!(second.overview_markdown, first.overview_markdown);

        // Failure / 500 fallback test
        let failing_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream error"))
            .mount(&failing_server)
            .await;

        let failing_cache = Arc::new(InMemoryCacheCoordinator::new());
        let tuning = crate::config::LlmTuning {
            request_timeout: Duration::from_millis(200),
            ..Default::default()
        };
        let failing_provider = Arc::new(OpenAiCompatProvider::with_tuning(
            reqwest::Client::new(),
            SsrfGuard::new(vec!["127.0.0.1".to_owned()]),
            tuning,
            1024 * 1024,
        ));
        let failing_runner = IdempotentLlmRunner::new(failing_cache, failing_provider);
        let failing_config = sample_ai_config(&failing_server.uri());

        let fallback_res = failing_runner
            .summarize_daily_idempotent(&failing_config, &input)
            .await
            .unwrap();
        assert!(fallback_res.degraded_fallback);
        assert_eq!(fallback_res.model_used, FALLBACK_MODEL_LABEL);
        assert_eq!(fallback_res.prompt_tokens, 0);
        assert_eq!(fallback_res.completion_tokens, 0);
        assert!(fallback_res.overview_markdown.contains("Gemini CLI Update"));

        // Timeout fallback test
        let slow_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(400))
                    .set_body_string("{}"),
            )
            .mount(&slow_server)
            .await;

        let timeout_cache = Arc::new(InMemoryCacheCoordinator::new());
        let timeout_tuning = crate::config::LlmTuning {
            request_timeout: Duration::from_millis(50),
            ..Default::default()
        };
        let timeout_provider = Arc::new(OpenAiCompatProvider::with_tuning(
            reqwest::Client::new(),
            SsrfGuard::new(vec!["127.0.0.1".to_owned()]),
            timeout_tuning,
            1024 * 1024,
        ));
        let timeout_runner = IdempotentLlmRunner::new(timeout_cache, timeout_provider);
        let timeout_res = timeout_runner
            .summarize_daily_idempotent(&sample_ai_config(&slow_server.uri()), &input)
            .await
            .unwrap();
        assert!(timeout_res.degraded_fallback);
        assert_eq!(timeout_res.model_used, FALLBACK_MODEL_LABEL);
    }
}
