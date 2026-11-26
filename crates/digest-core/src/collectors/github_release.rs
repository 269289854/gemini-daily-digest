//! GitHub Releases API collector strategy.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::collectors::{execute_conditional_get, CollectorContext, RawHttpOutcome};
use crate::constants::{
    ENV_GITHUB_TOKEN, GITHUB_API_VERSION, HEADER_ACCEPT, HEADER_AUTHORIZATION,
    HEADER_BEARER_PREFIX, HEADER_X_GITHUB_API_VERSION,
};
use crate::domain::{FetchOutcome, SourceType};
use crate::error::CoreError;
use crate::traits::{
    ConditionalFetchHeaders, FetchResult, RawCollectedItem, SourceCollector, SourceDescriptor,
};

const GITHUB_ACCEPT_HEADER: &str = "application/vnd.github+json";

/// Collector for GitHub repository releases (`/repos/{owner}/{repo}/releases`).
#[derive(Debug, Clone)]
pub struct GitHubReleaseCollector {
    ctx: CollectorContext,
}

impl GitHubReleaseCollector {
    #[must_use]
    pub fn new(ctx: CollectorContext) -> Self {
        Self { ctx }
    }
}

#[derive(Debug, Deserialize)]
struct GitHubAuthor {
    login: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseDto {
    id: Option<u64>,
    tag_name: Option<String>,
    name: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    draft: Option<bool>,
    published_at: Option<DateTime<Utc>>,
    created_at: Option<DateTime<Utc>>,
    author: Option<GitHubAuthor>,
}

fn parse_owner_repo(handle_or_url: &str) -> Result<(String, String), CoreError> {
    let trimmed = handle_or_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "GitHub repository handle_or_url must not be empty".to_owned(),
        ));
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let parsed = url::Url::parse(trimmed).map_err(|err| {
            CoreError::InvalidInput(format!("invalid GitHub repository URL: {err}"))
        })?;
        let segments: Vec<&str> = parsed
            .path_segments()
            .map(|iter| iter.filter(|seg| !seg.is_empty()).collect())
            .unwrap_or_default();

        let (owner, repo_raw) = if segments.first().copied() == Some("repos") && segments.len() >= 3
        {
            (segments[1], segments[2])
        } else if segments.len() >= 2 {
            (segments[0], segments[1])
        } else {
            return Err(CoreError::InvalidInput(format!(
                "unable to extract owner/repo from GitHub URL: {trimmed}"
            )));
        };
        let repo = repo_raw.strip_suffix(".git").unwrap_or(repo_raw);
        return Ok((owner.to_owned(), repo.to_owned()));
    }

    let cleaned = trimmed.trim_matches('/');
    let parts: Vec<&str> = cleaned.split('/').filter(|seg| !seg.is_empty()).collect();
    if parts.len() != 2 || parts.iter().any(|p| p.contains(char::is_whitespace)) {
        return Err(CoreError::InvalidInput(format!(
            "GitHub handle must be owner/repo, got: {trimmed}"
        )));
    }
    let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    Ok((parts[0].to_owned(), repo.to_owned()))
}

fn resolve_github_token(credentials_json: Option<&serde_json::Value>) -> Option<String> {
    if let Some(obj) = credentials_json.and_then(|v| v.as_object()) {
        for key in [
            "token",
            "bearer_token",
            "bearerToken",
            "github_token",
            "api_key",
        ] {
            if let Some(val) = obj
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(val.to_owned());
            }
        }
    }
    std::env::var(ENV_GITHUB_TOKEN)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

#[async_trait]
impl SourceCollector for GitHubReleaseCollector {
    fn source_type(&self) -> SourceType {
        SourceType::GithubRelease
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
        let (owner, repo) = parse_owner_repo(&source.handle_or_url)?;
        let base = self.ctx.github_api_base_url.trim_end_matches('/');
        let url = format!("{base}/repos/{owner}/{repo}/releases");

        let auth_header_val = resolve_github_token(credentials_json)
            .map(|tok| format!("{HEADER_BEARER_PREFIX}{tok}"));
        let mut headers: Vec<(&str, &str)> = vec![
            (HEADER_ACCEPT, GITHUB_ACCEPT_HEADER),
            (HEADER_X_GITHUB_API_VERSION, GITHUB_API_VERSION),
        ];
        if let Some(ref auth) = auth_header_val {
            headers.push((HEADER_AUTHORIZATION, auth.as_str()));
        }

        match execute_conditional_get(&self.ctx, &url, conditional, &headers, "github_release")
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
                let releases: Vec<GitHubReleaseDto> =
                    serde_json::from_slice(&bytes).map_err(|err| {
                        CoreError::Upstream(format!("failed parsing GitHub releases JSON: {err}"))
                    })?;

                let items = releases
                    .into_iter()
                    .filter(|r| !r.draft.unwrap_or(false))
                    .filter_map(|release| {
                        let html_url = release.html_url?.trim().to_owned();
                        if html_url.is_empty() {
                            return None;
                        }
                        let tag = release.tag_name.as_deref().map(str::trim).unwrap_or("");
                        let title = release
                            .name
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .or(if tag.is_empty() { None } else { Some(tag) })
                            .unwrap_or(&html_url)
                            .to_owned();
                        let external_id = release
                            .id
                            .map(|id| id.to_string())
                            .or_else(|| (!tag.is_empty()).then(|| tag.to_owned()));
                        let author = release
                            .author
                            .and_then(|a| a.login)
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty());
                        let summary = release
                            .body
                            .map(|b| b.trim().to_owned())
                            .filter(|b| !b.is_empty());
                        let published_at = release.published_at.or(release.created_at);

                        Some(RawCollectedItem {
                            external_id,
                            url: html_url,
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
                    etag: resp_headers.etag,
                    last_modified: resp_headers.last_modified,
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
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn github_release_collector_fetches_and_parses_releases() {
        let server = MockServer::start().await;
        let payload = serde_json::json!([
            {
                "id": 1001,
                "tag_name": "v0.21.0",
                "name": "SDK v0.21.0",
                "body": "Adds Gemini Live streaming support.",
                "html_url": "https://github.com/google-gemini/example-sdk/releases/tag/v0.21.0",
                "published_at": "2026-11-25T12:00:00Z",
                "draft": false,
                "author": { "login": "gemini-bot" }
            },
            {
                "id": 1002,
                "tag_name": "v0.22.0-draft",
                "name": "Draft",
                "html_url": "https://github.com/google-gemini/example-sdk/releases/tag/v0.22.0",
                "draft": true
            }
        ]);

        Mock::given(method("GET"))
            .and(path("/repos/google-gemini/example-sdk/releases"))
            .and(header("authorization", "Bearer test-gh-pat"))
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
        .with_github_api_base_url(server.uri());
        let collector = GitHubReleaseCollector::new(ctx);

        let source = SourceDescriptor {
            id: Uuid::new_v4(),
            source_type: SourceType::GithubRelease,
            scope: SourceScope::SharedPublic,
            canonical_key: "github_release:google-gemini/example-sdk".to_owned(),
            name: "Example SDK".to_owned(),
            handle_or_url: "google-gemini/example-sdk".to_owned(),
            category: "Releases".to_owned(),
        };
        let creds = serde_json::json!({ "token": "test-gh-pat" });
        let result = collector
            .fetch_with_credentials(&source, &ConditionalFetchHeaders::default(), Some(&creds))
            .await
            .unwrap();

        assert_eq!(result.outcome, FetchOutcome::Updated);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].external_id.as_deref(), Some("1001"));
        assert_eq!(result.items[0].title, "SDK v0.21.0");
        assert_eq!(result.items[0].author.as_deref(), Some("gemini-bot"));
    }
}
