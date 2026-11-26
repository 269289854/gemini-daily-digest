//! Plain-text digest renderer for email text/plain parts and IM fallback channels.

use crate::domain::{AlertPriority, RenderInput};
use crate::error::CoreError;
use crate::renderer::html_email::{render_html_body, sanitize_http_url};
use crate::renderer::markdown::render_markdown_body;
use crate::renderer::{build_render_input_from_raw, format_subject};
use crate::traits::{DigestRenderer, RawCollectedItem, RenderedNotification};

/// Renders clean plain-text digest notifications.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainTextDigestRenderer;

impl PlainTextDigestRenderer {
    /// Creates a new plain-text digest renderer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Renders plain-text output from `RenderInput`.
    #[must_use]
    pub fn render_plain_text(&self, input: &RenderInput) -> String {
        render_plain_text_body(input)
    }
}

impl DigestRenderer for PlainTextDigestRenderer {
    fn render(
        &self,
        digest: &crate::traits::SummarizedDigest,
        items: &[RawCollectedItem],
        priority: AlertPriority,
    ) -> Result<RenderedNotification, CoreError> {
        let input = build_render_input_from_raw(digest, items, priority);
        self.render_structured(&input)
    }

    fn render_structured(&self, input: &RenderInput) -> Result<RenderedNotification, CoreError> {
        Ok(RenderedNotification {
            subject: format_subject(input),
            text_body: render_plain_text_body(input),
            html_body: render_html_body(input),
            markdown_body: render_markdown_body(input),
            priority: input.priority,
        })
    }
}

/// Renders plain-text output for a `RenderInput`.
#[must_use]
pub fn render_plain_text_body(input: &RenderInput) -> String {
    let mut lines = Vec::new();
    lines.push(input.digest.title.trim().to_owned());
    lines.push(format!(
        "Date: {} | Priority: {}",
        input.digest_date,
        input.priority.as_str().to_uppercase()
    ));

    if input.digest.degraded_fallback {
        lines.push(
            "[Notice] Primary AI summarization was unavailable; showing deterministic summary."
                .to_owned(),
        );
    }

    lines.push(String::new());
    lines.push("EXECUTIVE OVERVIEW".to_owned());
    lines.push(input.digest.overview_markdown.trim().to_owned());

    if !input.highlighted_items.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "HIGHLIGHTED ALERTS ({})",
            input.highlighted_items.len()
        ));
        for item in &input.highlighted_items {
            lines.push(format_plain_item(item));
        }
    }

    if !input.sections.is_empty() {
        lines.push(String::new());
        lines.push("PER-SOURCE SUMMARIES".to_owned());
        for section in &input.sections {
            lines.push(String::new());
            lines.push(format!("* {} [{}]", section.source_name, section.category));
            if !section.summary_markdown.trim().is_empty() {
                lines.push(section.summary_markdown.trim().to_owned());
            }
            for point in &section.key_points {
                lines.push(format!("  - {point}"));
            }
            for item in &section.items {
                lines.push(format!("  {}", format_plain_item(item)));
            }
        }
    } else if !input.all_items.is_empty() {
        lines.push(String::new());
        lines.push("COLLECTED ITEMS".to_owned());
        for item in &input.all_items {
            lines.push(format_plain_item(item));
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "Model: {} | Tokens: {} prompt / {} completion",
        input.digest.model_used, input.digest.prompt_tokens, input.digest.completion_tokens
    ));

    lines.join("\n")
}

fn format_plain_item(item: &crate::domain::ProcessedItem) -> String {
    let url_display = sanitize_http_url(&item.canonical_url)
        .unwrap_or_else(|| "[non-http link omitted]".to_owned());
    let keywords = if item.match_result.matched_keywords.is_empty() {
        String::new()
    } else {
        format!(" [{}]", item.match_result.matched_keywords.join(", "))
    };
    format!(
        "- [{}] {}{} -> {}",
        item.match_result.priority.as_str().to_uppercase(),
        item.title.trim(),
        keywords,
        url_display
    )
}
