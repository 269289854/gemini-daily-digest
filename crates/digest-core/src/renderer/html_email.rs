//! Gmail light/dark safe HTML email renderer with strict XSS escaping and URL scheme filtering.

use crate::constants::SSRF_ALLOWED_SCHEMES;
use crate::domain::{is_zh_locale, AlertPriority, DigestSection, ProcessedItem, RenderInput};
use crate::error::CoreError;
use crate::renderer::markdown::render_markdown_body;
use crate::renderer::plain_text::render_plain_text_body;
use crate::renderer::{build_render_input_from_raw, format_subject};
use crate::traits::{DigestRenderer, RawCollectedItem, RenderedNotification};

/// Renders Gmail light/dark safe HTML email notifications.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlEmailDigestRenderer;

impl HtmlEmailDigestRenderer {
    /// Creates a new HTML email digest renderer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Renders a complete Gmail light/dark safe HTML email document.
    #[must_use]
    pub fn render_html(&self, input: &RenderInput) -> String {
        render_html_body(input)
    }
}

impl DigestRenderer for HtmlEmailDigestRenderer {
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

/// Escapes untrusted text for safe inclusion in HTML text nodes and double-quoted attributes.
#[must_use]
pub fn escape_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Returns `Some(canonical_url)` only if `raw` parses as `http://` or `https://`.
#[must_use]
pub fn sanitize_http_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let parsed = url::Url::parse(trimmed).ok()?;
    if SSRF_ALLOWED_SCHEMES.contains(&parsed.scheme()) {
        Some(parsed.to_string())
    } else {
        None
    }
}

/// Renders a Gmail light/dark safe HTML email body from a `RenderInput`.
#[must_use]
pub fn render_html_body(input: &RenderInput) -> String {
    let zh_mode = is_zh_locale(&input.locale);
    let html_lang = if zh_mode { "zh-CN" } else { "en" };
    let subject = escape_html(&format_subject(input));
    let title = escape_html(&input.digest.title);
    let date_str = escape_html(&input.digest_date.to_string());
    let priority_badge = render_priority_badge(input.priority);

    let degraded_banner = if input.digest.degraded_fallback {
        if zh_mode {
            r#"<tr><td style="padding:10px 20px;background-color:#FEF3C7;color:#92400E;border-bottom:1px solid #F59E0B;font-size:13px;" class="gdd-warn"><strong>降级模式：</strong> 主大模型服务暂不可用，当前展示离线降级摘要。</td></tr>"#
        } else {
            r#"<tr><td style="padding:10px 20px;background-color:#FEF3C7;color:#92400E;border-bottom:1px solid #F59E0B;font-size:13px;" class="gdd-warn"><strong>Fallback Mode:</strong> Primary AI summarization was unavailable; showing deterministic summary.</td></tr>"#
        }
    } else {
        ""
    };

    let alerts_html = render_alerts_section(&input.highlighted_items, zh_mode);
    let overview_html = render_overview_paragraphs(&input.digest.overview_markdown, zh_mode);
    let sections_html = if input.sections.is_empty() {
        render_flat_items_section(&input.all_items, zh_mode)
    } else {
        render_sections_html(&input.sections, zh_mode)
    };

    let model_used = escape_html(&input.digest.model_used);
    let prompt_tokens = input.digest.prompt_tokens;
    let completion_tokens = input.digest.completion_tokens;
    let degraded_label = match (input.digest.degraded_fallback, zh_mode) {
        (true, true) => "（降级摘要）",
        (true, false) => " (degraded fallback)",
        (false, _) => "",
    };
    let header_eyebrow = if zh_mode {
        "每日 AI 生态情报"
    } else {
        "Daily Intelligence Digest"
    };
    let overview_heading = if zh_mode {
        "核心综述"
    } else {
        "Executive Overview"
    };
    let footer_text = if zh_mode {
        format!(
            "模型: <strong>{model_used}</strong>{degraded_label} &middot; Token 消耗: {prompt_tokens} 输入 / {completion_tokens} 输出"
        )
    } else {
        format!(
            "Model: <strong>{model_used}</strong>{degraded_label} &middot; Tokens: {prompt_tokens} prompt / {completion_tokens} completion"
        )
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="{html_lang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light dark">
<meta name="supported-color-schemes" content="light dark">
<title>{subject}</title>
<style>
:root {{ color-scheme: light dark; supported-color-schemes: light dark; }}
@media (prefers-color-scheme: dark) {{
  body, .gdd-bg {{ background-color: #0F172A !important; color: #E2E8F0 !important; }}
  .gdd-card {{ background-color: #1E293B !important; border-color: #334155 !important; color: #E2E8F0 !important; }}
  .gdd-item {{ background-color: #0F172A !important; border-color: #334155 !important; }}
  .gdd-muted {{ color: #94A3B8 !important; }}
  .gdd-link {{ color: #60A5FA !important; }}
  .gdd-alert-box {{ background-color: #451A03 !important; border-color: #B45309 !important; }}
  .gdd-warn {{ background-color: #451A03 !important; color: #FDE68A !important; }}
}}
</style>
</head>
<body class="gdd-bg" style="margin:0;padding:0;background-color:#F8FAFC;color:#0F172A;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;line-height:1.5;">
<table role="presentation" width="100%" cellspacing="0" cellpadding="0" class="gdd-bg" style="background-color:#F8FAFC;padding:24px 12px;">
  <tr>
    <td align="center">
      <table role="presentation" width="100%" cellspacing="0" cellpadding="0" class="gdd-card" style="max-width:680px;background-color:#FFFFFF;border:1px solid #E2E8F0;border-radius:10px;overflow:hidden;">
        <tr>
          <td style="padding:20px 24px;background-color:#1E293B;color:#F8FAFC;">
            <div style="font-size:12px;letter-spacing:0.06em;text-transform:uppercase;color:#94A3B8;">{header_eyebrow} &middot; {date_str}</div>
            <h1 style="margin:6px 0 8px 0;font-size:20px;line-height:1.3;color:#FFFFFF;">{title}</h1>
            {priority_badge}
          </td>
        </tr>
        {degraded_banner}
        {alerts_html}
        <tr>
          <td style="padding:20px 24px;border-bottom:1px solid #E2E8F0;">
            <h2 style="margin:0 0 10px 0;font-size:16px;color:#0F172A;" class="gdd-card">{overview_heading}</h2>
            <div style="font-size:14px;color:#334155;" class="gdd-card">{overview_html}</div>
          </td>
        </tr>
        <tr>
          <td style="padding:20px 24px;">
            {sections_html}
          </td>
        </tr>
        <tr>
          <td style="padding:14px 24px;background-color:#F1F5F9;border-top:1px solid #E2E8F0;font-size:12px;color:#475569;" class="gdd-card gdd-muted">
            {footer_text}
          </td>
        </tr>
      </table>
    </td>
  </tr>
</table>
</body>
</html>"#
    )
}

fn render_priority_badge(priority: AlertPriority) -> &'static str {
    match priority {
        AlertPriority::Urgent => {
            r#"<span style="display:inline-block;padding:2px 8px;border-radius:4px;background-color:#DC2626;color:#FFFFFF;font-size:11px;font-weight:700;">URGENT</span>"#
        }
        AlertPriority::High => {
            r#"<span style="display:inline-block;padding:2px 8px;border-radius:4px;background-color:#D97706;color:#FFFFFF;font-size:11px;font-weight:700;">HIGH</span>"#
        }
        AlertPriority::Normal => {
            r#"<span style="display:inline-block;padding:2px 8px;border-radius:4px;background-color:#2563EB;color:#FFFFFF;font-size:11px;font-weight:700;">NORMAL</span>"#
        }
    }
}

fn render_alerts_section(alerts: &[ProcessedItem], zh_mode: bool) -> String {
    if alerts.is_empty() {
        return String::new();
    }
    let mut rows = String::new();
    for item in alerts {
        rows.push_str(&render_item_card(item));
    }
    let heading = if zh_mode {
        format!("重点与紧急提醒 ({})", alerts.len())
    } else {
        format!("Urgent &amp; High Alert Highlights ({})", alerts.len())
    };
    format!(
        r#"<tr>
          <td style="padding:18px 24px;border-bottom:1px solid #E2E8F0;">
            <table role="presentation" width="100%" cellspacing="0" cellpadding="0" class="gdd-alert-box" style="background-color:#FFF7ED;border:1px solid #FDBA74;border-radius:8px;padding:12px 14px;">
              <tr><td>
                <h2 style="margin:0 0 10px 0;font-size:15px;color:#9A3412;">{heading}</h2>
                {rows}
              </td></tr>
            </table>
          </td>
        </tr>"#
    )
}

fn render_overview_paragraphs(markdown: &str, zh_mode: bool) -> String {
    let trimmed = markdown.trim();
    if trimmed.is_empty() {
        return if zh_mode {
            "<p style=\"margin:0;\">暂无综述。</p>".to_owned()
        } else {
            "<p style=\"margin:0;\">No overview available.</p>".to_owned()
        };
    }
    let mut out = String::new();
    for line in trimmed.lines() {
        let line_trimmed = line.trim();
        if line_trimmed.is_empty() {
            continue;
        }
        let escaped = escape_html(line_trimmed);
        out.push_str(&format!(r#"<p style="margin:0 0 8px 0;">{escaped}</p>"#));
    }
    out
}

fn render_sections_html(sections: &[DigestSection], zh_mode: bool) -> String {
    let mut out = String::new();
    for section in sections {
        let source_name = escape_html(&section.source_name);
        let category = escape_html(&section.category);
        let summary_html = render_overview_paragraphs(&section.summary_markdown, zh_mode);
        let mut points_html = String::new();
        if !section.key_points.is_empty() {
            points_html
                .push_str(r#"<ul style="margin:8px 0 12px 18px;padding:0;font-size:13px;">"#);
            for pt in &section.key_points {
                let escaped_pt = escape_html(pt);
                points_html.push_str(&format!(
                    r#"<li style="margin-bottom:4px;">{escaped_pt}</li>"#
                ));
            }
            points_html.push_str("</ul>");
        }
        let mut items_html = String::new();
        for item in &section.items {
            items_html.push_str(&render_item_card(item));
        }

        out.push_str(&format!(
            r#"<div style="margin-bottom:22px;">
              <h3 style="margin:0 0 6px 0;font-size:15px;color:#0F172A;" class="gdd-card">{source_name} <span style="font-size:12px;font-weight:400;color:#64748B;" class="gdd-muted">[{category}]</span></h3>
              <div style="font-size:13px;color:#334155;" class="gdd-card">{summary_html}</div>
              {points_html}
              {items_html}
            </div>"#
        ));
    }
    out
}

fn render_flat_items_section(items: &[ProcessedItem], zh_mode: bool) -> String {
    if items.is_empty() {
        return if zh_mode {
            r#"<p style="margin:0;font-size:13px;color:#64748B;" class="gdd-muted">本周期内暂无新增采集条目。</p>"#.to_owned()
        } else {
            r#"<p style="margin:0;font-size:13px;color:#64748B;" class="gdd-muted">No items collected for this period.</p>"#.to_owned()
        };
    }
    let heading = if zh_mode {
        "采集条目"
    } else {
        "Collected Updates"
    };
    let mut out = format!(
        r#"<h2 style="margin:0 0 12px 0;font-size:16px;color:#0F172A;" class="gdd-card">{heading}</h2>"#,
    );
    for item in items {
        out.push_str(&render_item_card(item));
    }
    out
}

fn render_item_card(item: &ProcessedItem) -> String {
    let title_escaped = escape_html(item.title.trim());
    let link_html = match sanitize_http_url(&item.canonical_url) {
        Some(safe_url) => {
            let escaped_url = escape_html(&safe_url);
            format!(
                r#"<a href="{escaped_url}" target="_blank" rel="noopener noreferrer" class="gdd-link" style="color:#1D4ED8;text-decoration:none;font-weight:600;">{title_escaped}</a>"#
            )
        }
        None => {
            format!(r#"<span style="font-weight:600;">{title_escaped}</span>"#)
        }
    };

    let mut meta_parts = Vec::new();
    if let Some(source_name) = &item.source_name {
        meta_parts.push(escape_html(source_name));
    }
    if let Some(author) = &item.author {
        if !author.trim().is_empty() {
            meta_parts.push(format!("by {}", escape_html(author.trim())));
        }
    }
    if let Some(published_at) = item.published_at {
        meta_parts.push(escape_html(
            &published_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        ));
    }
    if !item.match_result.matched_keywords.is_empty() {
        let joined = item
            .match_result
            .matched_keywords
            .iter()
            .map(|kw| escape_html(kw))
            .collect::<Vec<_>>()
            .join(", ");
        meta_parts.push(format!("matched: <strong>{joined}</strong>"));
    }

    let meta_html = if meta_parts.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div style="margin-top:4px;font-size:12px;color:#64748B;" class="gdd-muted">{}</div>"#,
            meta_parts.join(" &middot; ")
        )
    };

    let summary_html = item
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let escaped = escape_html(s);
            format!(r#"<div style="margin-top:6px;font-size:13px;color:#334155;" class="gdd-card">{escaped}</div>"#)
        })
        .unwrap_or_default();

    format!(
        r#"<div class="gdd-item" style="margin-bottom:10px;padding:10px 12px;background-color:#F8FAFC;border:1px solid #E2E8F0;border-radius:6px;">
          <div style="font-size:14px;">{link_html}</div>
          {meta_html}
          {summary_html}
        </div>"#
    )
}
