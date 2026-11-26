//! OpenAI-compatible HTTP LLM provider (`DeepSeek` default) with SSRF checks,
//! clean `/v1/chat/completions` normalization, capped reads, and token accounting.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{AppConfig, LlmTuning};
use crate::constants::{
    CONTENT_TYPE_JSON, DEFAULT_MAX_FEED_BYTES, HEADER_ACCEPT, HEADER_AUTHORIZATION,
    HEADER_BEARER_PREFIX, HEADER_CONTENT_TYPE, LLM_CHAT_COMPLETIONS_PATH,
    LLM_CHAT_COMPLETIONS_SUFFIX, LLM_MODELS_PATH, LLM_MODELS_SUFFIX,
};
use crate::domain::{
    DailyDigestSynthesisInput, ProcessedItem, SourceDigestInput, SourceSummaryResult, UserAiConfig,
};
use crate::error::CoreError;
use crate::http::{read_response_bytes_capped, shared_http_client};
use crate::llm::fingerprint::compute_source_input_fingerprint;
use crate::ssrf::SsrfGuard;
use crate::traits::{
    AiModelOption, LlmProvider, RawCollectedItem, StructuredLlmProvider, SummarizedDigest,
};

const MAX_PROMPT_SUMMARY_CHARS: usize = 400;
const MAX_ERROR_BODY_CHARS: usize = 320;
const TEST_CONNECTION_MAX_TOKENS: u32 = 16;

/// OpenAI-compatible chat completions provider (supporting DeepSeek, OpenAI, and compatible proxies).
#[derive(Debug, Clone)]
pub struct OpenAiCompatProvider {
    http_client: reqwest::Client,
    ssrf_guard: SsrfGuard,
    tuning: LlmTuning,
    max_response_bytes: usize,
    default_config: Option<UserAiConfig>,
}

impl OpenAiCompatProvider {
    /// Creates an `OpenAiCompatProvider` with default `LlmTuning` and 2 MiB response cap.
    #[must_use]
    pub fn new(http_client: reqwest::Client, ssrf_guard: SsrfGuard) -> Self {
        Self::with_tuning(
            http_client,
            ssrf_guard,
            LlmTuning::default(),
            DEFAULT_MAX_FEED_BYTES,
        )
    }

    /// Creates an `OpenAiCompatProvider` with explicit tuning and response size cap.
    #[must_use]
    pub fn with_tuning(
        http_client: reqwest::Client,
        ssrf_guard: SsrfGuard,
        tuning: LlmTuning,
        max_response_bytes: usize,
    ) -> Self {
        Self {
            http_client,
            ssrf_guard,
            tuning,
            max_response_bytes,
            default_config: None,
        }
    }

    /// Attaches a default `UserAiConfig` used when invoked through the `LlmProvider` trait.
    #[must_use]
    pub fn with_default_config(mut self, config: UserAiConfig) -> Self {
        self.default_config = Some(config);
        self
    }

    /// Constructs an `OpenAiCompatProvider` from `AppConfig`.
    pub fn from_config(config: &AppConfig) -> Result<Self, CoreError> {
        let client = shared_http_client(config.outbound_proxy_url.as_deref(), &config.collector)?;
        let guard = SsrfGuard::from_config(config);
        Ok(Self::with_tuning(
            client,
            guard,
            config.llm.clone(),
            config.resources.max_feed_bytes.get(),
        ))
    }

    /// Normalizes a provider `base_url` with `LLM_CHAT_COMPLETIONS_PATH` without doubling `/v1`.
    pub fn build_chat_completions_url(base_url: &str) -> Result<String, CoreError> {
        let trimmed = base_url.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(CoreError::InvalidConfig(
                "AI base_url cannot be empty".to_owned(),
            ));
        }

        if trimmed.ends_with(LLM_CHAT_COMPLETIONS_PATH)
            || trimmed.ends_with(LLM_CHAT_COMPLETIONS_SUFFIX)
        {
            return Ok(trimmed.to_owned());
        }

        let base_without_v1 = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
        Ok(format!("{base_without_v1}{LLM_CHAT_COMPLETIONS_PATH}"))
    }

    /// Normalizes a provider `base_url` to its `/models` discovery endpoint.
    pub fn build_models_url(base_url: &str) -> Result<String, CoreError> {
        let trimmed = base_url.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(CoreError::InvalidConfig(
                "AI base_url cannot be empty".to_owned(),
            ));
        }

        if trimmed.ends_with(LLM_MODELS_SUFFIX) {
            return Ok(trimmed.to_owned());
        }

        let normalized = trimmed
            .strip_suffix(LLM_CHAT_COMPLETIONS_SUFFIX)
            .unwrap_or(trimmed)
            .trim_end_matches('/');

        let parsed = Url::parse(normalized)
            .map_err(|err| CoreError::InvalidConfig(format!("invalid AI base_url: {err}")))?;

        if parsed.path().trim_matches('/').is_empty() {
            Ok(format!("{normalized}{LLM_MODELS_PATH}"))
        } else {
            Ok(format!("{normalized}{LLM_MODELS_SUFFIX}"))
        }
    }

    async fn call_chat_completion(
        &self,
        config: &UserAiConfig,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens_override: Option<u32>,
    ) -> Result<ChatCompletionOutput, CoreError> {
        let endpoint_url = Self::build_chat_completions_url(&config.base_url)?;
        let validated_url = self.ssrf_guard.validate_url(&endpoint_url).await?;

        let api_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CoreError::InvalidConfig("AI API key is not configured".to_owned()))?;

        let model = config.model.trim();
        if model.is_empty() {
            return Err(CoreError::InvalidConfig(
                "AI model cannot be empty".to_owned(),
            ));
        }

        let effective_system = if system_prompt.trim().is_empty() {
            self.tuning.default_system_prompt.as_str()
        } else {
            system_prompt.trim()
        };

        let request_body = ChatCompletionRequest {
            model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: effective_system,
                },
                ChatMessage {
                    role: "user",
                    content: user_prompt,
                },
            ],
            temperature: config.temperature,
            max_tokens: max_tokens_override.unwrap_or(config.max_tokens).max(1),
        };

        let response = self
            .http_client
            .post(validated_url)
            .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
            .header(
                HEADER_AUTHORIZATION,
                format!("{HEADER_BEARER_PREFIX}{api_key}"),
            )
            .timeout(self.tuning.request_timeout)
            .json(&request_body)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        let body = read_response_bytes_capped(response, self.max_response_bytes).await?;
        if !status.is_success() {
            let preview = truncate_chars(&String::from_utf8_lossy(&body), MAX_ERROR_BODY_CHARS);
            return Err(CoreError::Upstream(format!(
                "LLM endpoint returned HTTP {}: {}",
                status.as_u16(),
                preview
            )));
        }

        let parsed: ChatCompletionResponse = serde_json::from_slice(&body).map_err(|err| {
            CoreError::Upstream(format!("failed to parse LLM JSON response: {err}"))
        })?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .ok_or_else(|| {
                CoreError::Upstream("LLM response contained no message content".to_owned())
            })?;

        let usage = parsed.usage.unwrap_or_default();
        let model_used = parsed
            .model
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| model.to_owned());

        Ok(ChatCompletionOutput {
            content,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            model_used,
        })
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    async fn summarize(
        &self,
        system_prompt: &str,
        items: &[RawCollectedItem],
    ) -> Result<SummarizedDigest, CoreError> {
        let mut config = self.default_config.clone().unwrap_or_else(|| UserAiConfig {
            provider_label: self.tuning.default_provider_label.clone(),
            base_url: self.tuning.default_base_url.clone(),
            api_key: None,
            model: self.tuning.default_model.clone(),
            system_prompt: self.tuning.default_system_prompt.clone(),
            temperature: self.tuning.default_temperature,
            max_tokens: self.tuning.default_max_tokens,
        });
        if !system_prompt.trim().is_empty() {
            config.system_prompt = system_prompt.trim().to_owned();
        }

        let mut user_lines = Vec::new();
        user_lines.push(format!(
            "Summarize the following {} collected item(s) in concise Markdown:",
            items.len()
        ));
        for (idx, item) in items
            .iter()
            .take(self.tuning.max_items_per_digest)
            .enumerate()
        {
            let summary_snippet = item
                .summary
                .as_deref()
                .map(|text| truncate_chars(text.trim(), MAX_PROMPT_SUMMARY_CHARS))
                .unwrap_or_default();
            user_lines.push(format!(
                "{}. {} | {} | {}",
                idx + 1,
                item.title.trim(),
                item.url.trim(),
                summary_snippet
            ));
        }

        let output = self
            .call_chat_completion(&config, &config.system_prompt, &user_lines.join("\n"), None)
            .await?;

        Ok(SummarizedDigest {
            title: "Gemini Ecosystem Daily Digest".to_owned(),
            overview_markdown: output.content,
            prompt_tokens: output.prompt_tokens,
            completion_tokens: output.completion_tokens,
            model_used: output.model_used,
            degraded_fallback: false,
        })
    }
}

#[async_trait]
impl StructuredLlmProvider for OpenAiCompatProvider {
    async fn summarize_source(
        &self,
        config: &UserAiConfig,
        input: &SourceDigestInput,
    ) -> Result<SourceSummaryResult, CoreError> {
        let fingerprint = compute_source_input_fingerprint(
            input.digest_date,
            input.source.id,
            config,
            &input.items,
        );

        if input.items.is_empty() {
            return Ok(SourceSummaryResult {
                source_id: input.source.id,
                source_name: input.source.name.clone(),
                category: input.source.category.clone(),
                digest_date: input.digest_date,
                input_fingerprint: fingerprint,
                summary_markdown: format!(
                    "No new items from {} on {}.",
                    input.source.name, input.digest_date
                ),
                key_points: Vec::new(),
                item_count: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
                model_used: config.model.clone(),
                degraded_fallback: false,
            });
        }

        let mut prompt_lines = Vec::new();
        prompt_lines.push(format!(
            "Source: {} (category: {}, date: {})",
            input.source.name, input.source.category, input.digest_date
        ));
        prompt_lines.push(
            "Produce a concise Markdown summary and up to 5 bullet key points grounded only in these items. You may reply with JSON {\"summaryMarkdown\": \"...\", \"keyPoints\": [\"...\"]} or Markdown bullets."
                .to_owned(),
        );
        for (idx, item) in input
            .items
            .iter()
            .take(self.tuning.max_items_per_digest)
            .enumerate()
        {
            prompt_lines.push(format_processed_item_for_prompt(idx + 1, item));
        }

        let output = self
            .call_chat_completion(
                config,
                &config.system_prompt,
                &prompt_lines.join("\n"),
                None,
            )
            .await?;

        let (summary_markdown, key_points) = parse_source_summary_output(&output.content);

        Ok(SourceSummaryResult {
            source_id: input.source.id,
            source_name: input.source.name.clone(),
            category: input.source.category.clone(),
            digest_date: input.digest_date,
            input_fingerprint: fingerprint,
            summary_markdown,
            key_points,
            item_count: input.items.len(),
            prompt_tokens: output.prompt_tokens,
            completion_tokens: output.completion_tokens,
            model_used: output.model_used,
            degraded_fallback: false,
        })
    }

    async fn synthesize_daily(
        &self,
        config: &UserAiConfig,
        input: &DailyDigestSynthesisInput,
    ) -> Result<SummarizedDigest, CoreError> {
        let mut prompt_lines = Vec::new();
        prompt_lines.push(format!(
            "Synthesize the daily executive digest for {} ({} total item(s), {} highlighted alert(s)).",
            input.digest_date,
            input.all_items.len(),
            input.highlighted_items.len()
        ));

        if !input.highlighted_items.is_empty() {
            prompt_lines.push("\nHighlighted Alerts:".to_owned());
            for (idx, item) in input
                .highlighted_items
                .iter()
                .take(self.tuning.max_items_per_digest)
                .enumerate()
            {
                prompt_lines.push(format_processed_item_for_prompt(idx + 1, item));
            }
        }

        if !input.source_summaries.is_empty() {
            prompt_lines.push("\nPer-Source Summaries:".to_owned());
            for summary in &input.source_summaries {
                prompt_lines.push(format!(
                    "## {} ({}, {} items)\n{}",
                    summary.source_name,
                    summary.category,
                    summary.item_count,
                    truncate_chars(&summary.summary_markdown, MAX_PROMPT_SUMMARY_CHARS * 2)
                ));
            }
        } else if !input.all_items.is_empty() {
            prompt_lines.push("\nCollected Items:".to_owned());
            for (idx, item) in input
                .all_items
                .iter()
                .take(self.tuning.max_items_per_digest)
                .enumerate()
            {
                prompt_lines.push(format_processed_item_for_prompt(idx + 1, item));
            }
        }

        let output = self
            .call_chat_completion(
                config,
                &config.system_prompt,
                &prompt_lines.join("\n"),
                None,
            )
            .await?;

        Ok(SummarizedDigest {
            title: format!("Gemini Daily Digest — {}", input.digest_date),
            overview_markdown: output.content,
            prompt_tokens: output.prompt_tokens,
            completion_tokens: output.completion_tokens,
            model_used: output.model_used,
            degraded_fallback: false,
        })
    }

    async fn test_connection(&self, config: &UserAiConfig) -> Result<(), CoreError> {
        let _output = self
            .call_chat_completion(
                config,
                "You are a health check responder.",
                "Reply with OK.",
                Some(TEST_CONNECTION_MAX_TOKENS),
            )
            .await?;
        Ok(())
    }

    async fn list_models(&self, config: &UserAiConfig) -> Result<Vec<AiModelOption>, CoreError> {
        let endpoint_url = Self::build_models_url(&config.base_url)?;
        let validated_url = self.ssrf_guard.validate_url(&endpoint_url).await?;

        let mut request = self
            .http_client
            .get(validated_url)
            .header(HEADER_ACCEPT, CONTENT_TYPE_JSON)
            .timeout(self.tuning.request_timeout);

        if let Some(api_key) = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request = request.header(
                HEADER_AUTHORIZATION,
                format!("{HEADER_BEARER_PREFIX}{api_key}"),
            );
        }

        let response = request.send().await.map_err(map_reqwest_error)?;
        let status = response.status();
        let body = read_response_bytes_capped(response, self.max_response_bytes).await?;
        if !status.is_success() {
            let preview = truncate_chars(&String::from_utf8_lossy(&body), MAX_ERROR_BODY_CHARS);
            return Err(CoreError::Upstream(format!(
                "LLM models endpoint returned HTTP {}: {}",
                status.as_u16(),
                preview
            )));
        }

        parse_models_response(&body)
    }
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

struct ChatCompletionOutput {
    content: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    model_used: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StructuredSourceJson {
    #[serde(default, alias = "summary_markdown")]
    summary_markdown: String,
    #[serde(default, alias = "key_points")]
    key_points: Vec<String>,
}

fn parse_source_summary_output(raw: &str) -> (String, Vec<String>) {
    let trimmed = raw.trim();
    let json_candidate = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);

    if json_candidate.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<StructuredSourceJson>(json_candidate) {
            let summary = parsed.summary_markdown.trim().to_owned();
            let points: Vec<String> = parsed
                .key_points
                .into_iter()
                .map(|point| point.trim().to_owned())
                .filter(|point| !point.is_empty())
                .collect();
            if !summary.is_empty() || !points.is_empty() {
                return (
                    if summary.is_empty() {
                        points.join("\n")
                    } else {
                        summary
                    },
                    points,
                );
            }
        }
    }

    let key_points: Vec<String> = trimmed
        .lines()
        .filter_map(|line| {
            let t = line.trim();
            t.strip_prefix("- ")
                .or_else(|| t.strip_prefix("* "))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .take(5)
        .collect();

    (trimmed.to_owned(), key_points)
}

fn format_processed_item_for_prompt(index: usize, item: &ProcessedItem) -> String {
    let keywords = if item.match_result.matched_keywords.is_empty() {
        String::new()
    } else {
        format!(
            " [matched: {}]",
            item.match_result.matched_keywords.join(", ")
        )
    };
    let summary_snippet = item
        .summary
        .as_deref()
        .map(|s| truncate_chars(s.trim(), MAX_PROMPT_SUMMARY_CHARS))
        .unwrap_or_default();
    format!(
        "{index}. [{}]{keywords} {} ({}) — {}",
        item.match_result.priority.as_str(),
        item.title.trim(),
        item.canonical_url.trim(),
        summary_snippet
    )
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_owned();
    }
    let truncated: String = input.chars().take(max_chars).collect();
    format!("{truncated}...")
}

fn map_reqwest_error(err: reqwest::Error) -> CoreError {
    if err.is_timeout() {
        CoreError::UpstreamTimeout(format!("LLM request timed out: {err}"))
    } else {
        CoreError::Upstream(format!("LLM request failed: {err}"))
    }
}

fn parse_models_response(body: &[u8]) -> Result<Vec<AiModelOption>, CoreError> {
    let parsed: serde_json::Value = serde_json::from_slice(body).map_err(|err| {
        CoreError::Upstream(format!("failed to parse LLM models JSON response: {err}"))
    })?;

    let items = parsed
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| parsed.get("models").and_then(serde_json::Value::as_array))
        .or_else(|| parsed.as_array())
        .ok_or_else(|| {
            CoreError::Upstream("LLM models response did not contain a model list".to_owned())
        })?;

    let mut deduped: BTreeMap<String, AiModelOption> = BTreeMap::new();
    for entry in items {
        if let Some(raw_id) = entry.as_str() {
            let id = raw_id.trim();
            if !id.is_empty() {
                deduped.entry(id.to_owned()).or_insert(AiModelOption {
                    id: id.to_owned(),
                    owned_by: None,
                });
            }
            continue;
        }

        let Some(obj) = entry.as_object() else {
            continue;
        };

        let id = obj
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .or_else(|| {
                obj.get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .map(|name| name.strip_prefix("models/").unwrap_or(name))
            })
            .filter(|id| !id.is_empty());

        let Some(id) = id else {
            continue;
        };

        let owned_by = obj
            .get("owned_by")
            .or_else(|| obj.get("ownedBy"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|owner| !owner.is_empty())
            .map(str::to_owned);

        deduped
            .entry(id.to_owned())
            .and_modify(|existing| {
                if existing.owned_by.is_none() && owned_by.is_some() {
                    existing.owned_by = owned_by.clone();
                }
            })
            .or_insert(AiModelOption {
                id: id.to_owned(),
                owned_by,
            });
    }

    Ok(deduped.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_models_url_handles_bare_hosts_versioned_paths_and_suffixes() {
        assert_eq!(
            OpenAiCompatProvider::build_models_url("https://api.deepseek.com").unwrap(),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            OpenAiCompatProvider::build_models_url("https://api.openai.com/v1/").unwrap(),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            OpenAiCompatProvider::build_models_url(
                "https://generativelanguage.googleapis.com/v1beta/openai"
            )
            .unwrap(),
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );
        assert_eq!(
            OpenAiCompatProvider::build_models_url("https://ark.cn-beijing.volces.com/api/v3")
                .unwrap(),
            "https://ark.cn-beijing.volces.com/api/v3/models"
        );
        assert_eq!(
            OpenAiCompatProvider::build_models_url("https://api.deepseek.com/v1/chat/completions")
                .unwrap(),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            OpenAiCompatProvider::build_models_url("https://api.deepseek.com/v1/models").unwrap(),
            "https://api.deepseek.com/v1/models"
        );
        assert!(matches!(
            OpenAiCompatProvider::build_models_url("   "),
            Err(CoreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn parse_models_response_supports_openai_gemini_and_array_formats() {
        let openai_json = br#"{
            "object": "list",
            "data": [
                { "id": "deepseek-reasoner", "owned_by": "deepseek" },
                { "id": "deepseek-chat", "owned_by": "deepseek" },
                { "id": "deepseek-chat", "owned_by": "duplicate" },
                { "id": "  " }
            ]
        }"#;
        let parsed_openai = parse_models_response(openai_json).unwrap();
        assert_eq!(
            parsed_openai,
            vec![
                AiModelOption {
                    id: "deepseek-chat".to_owned(),
                    owned_by: Some("deepseek".to_owned()),
                },
                AiModelOption {
                    id: "deepseek-reasoner".to_owned(),
                    owned_by: Some("deepseek".to_owned()),
                },
            ]
        );

        let gemini_json = br#"{
            "models": [
                { "name": "models/gemini-2.5-pro" },
                { "id": "openai/gpt-4o", "ownedBy": "openrouter" }
            ]
        }"#;
        let parsed_gemini = parse_models_response(gemini_json).unwrap();
        assert_eq!(
            parsed_gemini,
            vec![
                AiModelOption {
                    id: "gemini-2.5-pro".to_owned(),
                    owned_by: None,
                },
                AiModelOption {
                    id: "openai/gpt-4o".to_owned(),
                    owned_by: Some("openrouter".to_owned()),
                },
            ]
        );

        let array_json = br#"["gpt-4o-mini", { "id": "claude-3-7-sonnet" }]"#;
        let parsed_array = parse_models_response(array_json).unwrap();
        assert_eq!(
            parsed_array,
            vec![
                AiModelOption {
                    id: "claude-3-7-sonnet".to_owned(),
                    owned_by: None,
                },
                AiModelOption {
                    id: "gpt-4o-mini".to_owned(),
                    owned_by: None,
                },
            ]
        );
    }
}
