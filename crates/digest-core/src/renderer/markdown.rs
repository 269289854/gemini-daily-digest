//! Structured Markdown digest renderer.

use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

use crate::constants::{MARKDOWN_SAFE_REF_PLACEHOLDER, MARKDOWN_SANITIZE_MAX_PASSES};
use crate::domain::{is_zh_locale, AlertPriority, RenderInput};
use crate::error::CoreError;
use crate::renderer::html_email::{render_html_body, sanitize_http_url};
use crate::renderer::plain_text::render_plain_text_body;
use crate::renderer::{build_render_input_from_raw, format_subject};
use crate::traits::{DigestRenderer, RawCollectedItem, RenderedNotification};

/// Renders digests as structured Markdown documents.
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownDigestRenderer;

impl MarkdownDigestRenderer {
    /// Creates a new Markdown digest renderer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Renders a structured Markdown string from `RenderInput`.
    #[must_use]
    pub fn render_markdown(&self, input: &RenderInput) -> String {
        render_markdown_body(input)
    }
}

impl DigestRenderer for MarkdownDigestRenderer {
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

/// Renders structured Markdown for a `RenderInput`.
#[must_use]
pub fn render_markdown_body(input: &RenderInput) -> String {
    let zh_mode = is_zh_locale(&input.locale);
    let mut lines = Vec::new();
    lines.push(format!(
        "# {}",
        escape_markdown_metadata(input.digest.title.trim())
    ));
    lines.push(if zh_mode {
        format!(
            "_日期: {} | 优先级: {}_",
            input.digest_date,
            input.priority.as_str().to_uppercase()
        )
    } else {
        format!(
            "_Date: {} | Priority: {}_",
            input.digest_date,
            input.priority.as_str().to_uppercase()
        )
    });

    if input.digest.degraded_fallback {
        lines.push(String::new());
        lines.push(if zh_mode {
            "> **提示：** 主大模型服务暂不可用，当前展示离线降级摘要。".to_owned()
        } else {
            "> **Notice:** Primary AI summarization was unavailable; deterministic fallback summary shown."
                .to_owned()
        });
    }

    lines.push(String::new());
    lines.push(if zh_mode {
        "## 核心综述".to_owned()
    } else {
        "## Executive Overview".to_owned()
    });
    lines.push(sanitize_markdown_content(
        input.digest.overview_markdown.trim(),
    ));

    if !input.highlighted_items.is_empty() {
        lines.push(String::new());
        lines.push(if zh_mode {
            format!("## 重点关注提醒 ({})", input.highlighted_items.len())
        } else {
            format!("## Highlighted Alerts ({})", input.highlighted_items.len())
        });
        for item in &input.highlighted_items {
            lines.push(format_markdown_item(item));
        }
    }

    if !input.sections.is_empty() {
        lines.push(String::new());
        lines.push(if zh_mode {
            "## 各来源独立汇总".to_owned()
        } else {
            "## Per-Source Digests".to_owned()
        });
        for section in &input.sections {
            lines.push(String::new());
            lines.push(format!(
                "### {} ({})",
                escape_markdown_metadata(section.source_name.trim()),
                escape_markdown_metadata(section.category.trim())
            ));
            if !section.summary_markdown.trim().is_empty() {
                lines.push(sanitize_markdown_content(section.summary_markdown.trim()));
            }
            if !section.key_points.is_empty() {
                for point in &section.key_points {
                    let list_item = format!("- {}", escape_markdown_raw_html(point.trim()));
                    lines.push(sanitize_markdown_content(&list_item));
                }
            }
            for item in &section.items {
                lines.push(format_markdown_item(item));
            }
        }
    } else if !input.all_items.is_empty() {
        lines.push(String::new());
        lines.push(if zh_mode {
            "## 采集条目".to_owned()
        } else {
            "## Collected Items".to_owned()
        });
        for item in &input.all_items {
            lines.push(format_markdown_item(item));
        }
    }

    lines.push(String::new());
    lines.push("---".to_owned());
    let fallback_tag = match (input.digest.degraded_fallback, zh_mode) {
        (true, true) => "（降级摘要）",
        (true, false) => " (degraded fallback)",
        (false, _) => "",
    };
    lines.push(if zh_mode {
        format!(
            "模型: `{}`{} | Token 消耗: {} 输入 / {} 输出",
            sanitize_markdown_inline_code_metadata(input.digest.model_used.trim()),
            fallback_tag,
            input.digest.prompt_tokens,
            input.digest.completion_tokens
        )
    } else {
        format!(
            "Model: `{}`{} | Tokens: {} prompt / {} completion",
            sanitize_markdown_inline_code_metadata(input.digest.model_used.trim()),
            fallback_tag,
            input.digest.prompt_tokens,
            input.digest.completion_tokens
        )
    });

    let assembled = lines.join("\n");
    sanitize_markdown_content(&assembled)
}

/// Escapes raw HTML metacharacters (`&`, `<`, `>`) in untrusted metadata strings
/// without mangling normal quotes or Markdown punctuation.
#[must_use]
pub fn escape_markdown_raw_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Escapes raw HTML and Markdown link/reference metacharacters in plain-text metadata.
fn escape_markdown_metadata(raw: &str) -> String {
    escape_markdown_raw_html(raw)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// Keeps plain-text metadata safely contained within a single-line inline code span.
fn sanitize_markdown_inline_code_metadata(raw: &str) -> String {
    let stripped: String = raw
        .chars()
        .filter(|c| !matches!(c, '`' | '[' | ']' | '\r' | '\n'))
        .collect();
    escape_markdown_raw_html(&stripped)
}

/// Sanitizes Markdown prose (`overview_markdown` / `summary_markdown`) using a
/// spec-complete CommonMark 0.31.2 + GFM/Wikilinks parser (`pulldown-cmark`) in a
/// bounded fixed-point span rewriter. Escapes raw HTML outside code spans/blocks
/// while preserving blockquote/list container prefixes, and neutralizes dangerous
/// link destinations (`javascript:`, `vbscript:`, `data:`, `file:`, and
/// unallowlisted schemes) across inline, autolink, wikilink, and
/// reference-definition forms.
#[must_use]
pub fn sanitize_markdown_content(raw: &str) -> String {
    let mut current = raw.to_owned();
    let gfm_options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let wikilink_options = Options::ENABLE_WIKILINKS;
    let option_sets = [
        Options::empty(),
        gfm_options,
        wikilink_options,
        gfm_options | wikilink_options,
    ];

    'passes: for _ in 0..MARKDOWN_SANITIZE_MAX_PASSES {
        for options in option_sets {
            let edits = collect_sanitization_edits(&current, options);
            if !edits.is_empty() {
                current = apply_span_edits(&current, edits);
                continue 'passes;
            }
        }
        return current;
    }

    // Fail-closed fallback if pathological nesting exhausts max passes:
    // escape raw HTML and bracketed link/reference syntax so zero HTML or
    // Markdown links survive re-parsing.
    escape_markdown_raw_html(&current)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

#[derive(Debug, Clone)]
struct SpanEdit {
    start: usize,
    end: usize,
    replacement: String,
}

#[derive(Debug, Clone)]
struct OpenLink {
    range: Range<usize>,
    link_type: LinkType,
    is_image: bool,
    dangerous: bool,
    last_child_end: Option<usize>,
}

fn collect_sanitization_edits(current: &str, options: Options) -> Vec<SpanEdit> {
    let mut edits = Vec::new();
    let iter = Parser::new_ext(current, options).into_offset_iter();

    for (label, link_def) in iter.reference_definitions().iter() {
        if is_dangerous_link_destination(link_def.dest.as_ref()) {
            edits.push(SpanEdit {
                start: link_def.span.start,
                end: link_def.span.end,
                replacement: format!(
                    "[{}]: {MARKDOWN_SAFE_REF_PLACEHOLDER}",
                    escape_angles_only(label)
                ),
            });
        }
    }

    let mut link_stack: Vec<OpenLink> = Vec::new();

    for (event, range) in iter {
        if !matches!(event, Event::End(TagEnd::Link | TagEnd::Image)) {
            if let Some(top) = link_stack.last_mut() {
                top.last_child_end =
                    Some(top.last_child_end.map_or(range.end, |p| p.max(range.end)));
            }
        }

        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                let dangerous = !matches!(link_type, LinkType::Email)
                    && is_dangerous_link_destination(dest_url.as_ref());
                link_stack.push(OpenLink {
                    range,
                    link_type,
                    is_image: false,
                    dangerous,
                    last_child_end: None,
                });
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) => {
                let dangerous = !matches!(link_type, LinkType::Email)
                    && is_dangerous_link_destination(dest_url.as_ref());
                link_stack.push(OpenLink {
                    range,
                    link_type,
                    is_image: true,
                    dangerous,
                    last_child_end: None,
                });
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                let Some(open) = link_stack.pop() else {
                    continue;
                };
                if let Some(parent) = link_stack.last_mut() {
                    parent.last_child_end = Some(
                        parent
                            .last_child_end
                            .map_or(open.range.end, |p| p.max(open.range.end)),
                    );
                }
                if open.dangerous {
                    push_dangerous_link_edits(current, &open, &mut edits);
                }
            }
            Event::Html(_) => {
                if let Some(slice) = current.get(range.clone()) {
                    if slice.contains('<') || slice.contains('>') {
                        edits.push(SpanEdit {
                            start: range.start,
                            end: range.end,
                            replacement: escape_angles_only(slice),
                        });
                    }
                }
            }
            Event::InlineHtml(_) => {
                push_inline_html_edits(current, range, &mut edits);
            }
            _ => {}
        }
    }

    edits
}

fn push_dangerous_link_edits(current: &str, open: &OpenLink, edits: &mut Vec<SpanEdit>) {
    if matches!(open.link_type, LinkType::Autolink | LinkType::Email) {
        if let Some(slice) = current.get(open.range.clone()) {
            edits.push(SpanEdit {
                start: open.range.start,
                end: open.range.end,
                replacement: escape_angles_only(slice),
            });
        }
        return;
    }

    if matches!(open.link_type, LinkType::WikiLink { .. }) {
        if let Some(slice) = current.get(open.range.clone()) {
            edits.push(SpanEdit {
                start: open.range.start,
                end: open.range.end,
                replacement: escape_angles_only(slice).replace(['[', ']'], ""),
            });
        }
        return;
    }

    let open_len = if open.is_image { 2 } else { 1 };
    let label_start = open.range.start.saturating_add(open_len);
    let label_end = match open.link_type {
        LinkType::Shortcut | LinkType::ShortcutUnknown => Some(open.range.end.saturating_sub(1)),
        LinkType::Collapsed | LinkType::CollapsedUnknown => Some(open.range.end.saturating_sub(3)),
        LinkType::Inline | LinkType::Reference | LinkType::ReferenceUnknown => {
            let search_from = open.last_child_end.unwrap_or(label_start).max(label_start);
            find_unescaped_closing_bracket(
                current.as_bytes(),
                label_start,
                search_from,
                open.range.end,
            )
        }
        LinkType::Autolink | LinkType::Email | LinkType::WikiLink { .. } => None,
    };

    if let Some(label_end) = label_end {
        if label_start <= label_end
            && label_end < open.range.end
            && current.is_char_boundary(open.range.start)
            && current.is_char_boundary(label_start)
            && current.is_char_boundary(label_end)
            && current.is_char_boundary(open.range.end)
        {
            edits.push(SpanEdit {
                start: open.range.start,
                end: label_start,
                replacement: String::new(),
            });
            edits.push(SpanEdit {
                start: label_end,
                end: open.range.end,
                replacement: String::new(),
            });
            return;
        }
    }

    edits.push(SpanEdit {
        start: open.range.start,
        end: open.range.end,
        replacement: String::new(),
    });
}

fn push_inline_html_edits(current: &str, range: Range<usize>, edits: &mut Vec<SpanEdit>) {
    let Some(slice) = current.get(range.clone()) else {
        return;
    };

    if !slice.contains('\n') {
        edits.push(SpanEdit {
            start: range.start,
            end: range.end,
            replacement: escape_angles_only(slice),
        });
        return;
    }

    // Multi-line InlineHtml inside blockquotes spans across `\n> ` continuation
    // prefixes; escape only outer `<` and `>` so container prefixes stay intact
    // and any inner tags are caught on the next fixed-point pass.
    let bytes = current.as_bytes();
    if bytes.get(range.start) == Some(&b'<')
        && range.end > range.start + 1
        && bytes.get(range.end - 1) == Some(&b'>')
    {
        edits.push(SpanEdit {
            start: range.start,
            end: range.start + 1,
            replacement: "&lt;".to_owned(),
        });
        edits.push(SpanEdit {
            start: range.end - 1,
            end: range.end,
            replacement: "&gt;".to_owned(),
        });
    } else {
        edits.push(SpanEdit {
            start: range.start,
            end: range.end,
            replacement: escape_angles_only(slice),
        });
    }
}

fn find_unescaped_closing_bracket(
    bytes: &[u8],
    label_start: usize,
    search_from: usize,
    range_end: usize,
) -> Option<usize> {
    let start = search_from.max(label_start).min(bytes.len());
    let end = range_end.min(bytes.len());
    (start..end).find(|&idx| bytes[idx] == b']' && !is_backslash_escaped(bytes, label_start, idx))
}

fn is_backslash_escaped(bytes: &[u8], min_start: usize, pos: usize) -> bool {
    let mut backslashes = 0usize;
    let mut cur = pos;
    while cur > min_start {
        cur -= 1;
        if bytes.get(cur) == Some(&b'\\') {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

fn apply_span_edits(current: &str, mut edits: Vec<SpanEdit>) -> String {
    edits.sort_by_key(|e| (e.start, e.end));
    let mut next = String::with_capacity(current.len());
    let mut cursor = 0usize;

    for edit in edits {
        if edit.start >= cursor
            && edit.start <= edit.end
            && edit.end <= current.len()
            && current.is_char_boundary(edit.start)
            && current.is_char_boundary(edit.end)
        {
            next.push_str(&current[cursor..edit.start]);
            next.push_str(&edit.replacement);
            cursor = edit.end;
        }
    }

    next.push_str(&current[cursor..]);
    next
}

fn decode_html_entities_for_scheme(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'&' {
            if let Some(ch) = raw[i..].chars().next() {
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
            break;
        }

        let rest = &raw[i + 1..];
        if let Some((consumed_after_amp, decoded_ch)) = parse_html_entity_after_amp(rest) {
            out.push(decoded_ch);
            i += 1 + consumed_after_amp;
        } else {
            out.push('&');
            i += 1;
        }
    }

    out
}

fn parse_html_entity_after_amp(rest: &str) -> Option<(usize, char)> {
    let bytes = rest.as_bytes();
    if let Some(hex_body) = rest.strip_prefix("#x").or_else(|| rest.strip_prefix("#X")) {
        let hex_len = hex_body.bytes().take_while(u8::is_ascii_hexdigit).count();
        if hex_len == 0 {
            return None;
        }
        let mut val: u64 = 0;
        for &b in &hex_body.as_bytes()[..hex_len] {
            let digit = match b {
                b'0'..=b'9' => u64::from(b - b'0'),
                b'a'..=b'f' => u64::from(b - b'a' + 10),
                b'A'..=b'F' => u64::from(b - b'A' + 10),
                _ => return None,
            };
            val = val.saturating_mul(16).saturating_add(digit).min(0x11_0000);
        }
        let ch = u32::try_from(val).ok().and_then(char::from_u32)?;
        let semi = usize::from(hex_body.as_bytes().get(hex_len) == Some(&b';'));
        return Some((2 + hex_len + semi, ch));
    }

    if let Some(dec_body) = rest.strip_prefix('#') {
        let dec_len = dec_body.bytes().take_while(u8::is_ascii_digit).count();
        if dec_len == 0 {
            return None;
        }
        let mut val: u64 = 0;
        for &b in &dec_body.as_bytes()[..dec_len] {
            let digit = u64::from(b - b'0');
            val = val.saturating_mul(10).saturating_add(digit).min(0x11_0000);
        }
        let ch = u32::try_from(val).ok().and_then(char::from_u32)?;
        let semi = usize::from(dec_body.as_bytes().get(dec_len) == Some(&b';'));
        return Some((1 + dec_len + semi, ch));
    }

    let name_len = bytes
        .iter()
        .take(32)
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    if name_len > 0 && bytes.get(name_len) == Some(&b';') {
        let name = &rest[..name_len];
        let ch = if name.eq_ignore_ascii_case("colon") {
            ':'
        } else if name.eq_ignore_ascii_case("tab") {
            '\t'
        } else if name.eq_ignore_ascii_case("newline") {
            '\n'
        } else if name.eq_ignore_ascii_case("amp") {
            '&'
        } else if name.eq_ignore_ascii_case("lpar") {
            '('
        } else if name.eq_ignore_ascii_case("rpar") {
            ')'
        } else if name.eq_ignore_ascii_case("sol") {
            '/'
        } else if name.eq_ignore_ascii_case("bsol") {
            '\\'
        } else {
            return None;
        };
        return Some((name_len + 1, ch));
    }

    None
}

fn is_dangerous_link_destination(dest: &str) -> bool {
    let trimmed = dest.trim();
    let unwrapped = if let Some(after_open) = trimmed.strip_prefix('<') {
        after_open.strip_suffix('>').unwrap_or(after_open)
    } else {
        trimmed
    };
    let decoded = decode_html_entities_for_scheme(unwrapped);
    let compact: String = decoded
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control() && *c != '\\')
        .collect();
    let lower = compact.to_ascii_lowercase();

    if lower.starts_with("javascript:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("data:")
        || lower.starts_with("file:")
    {
        return true;
    }

    let scheme_prefix = match lower.find(['/', '?', '#', ':']) {
        Some(idx) => &lower[..idx],
        None => lower.as_str(),
    };
    if scheme_prefix.contains('&') {
        return true;
    }

    if let Some(colon_idx) = lower.find(':') {
        let first_delim = lower.find(['/', '?', '#']).unwrap_or(lower.len());
        if colon_idx < first_delim {
            let allowed = lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("mailto:");
            if !allowed {
                return true;
            }
        }
    }

    false
}

fn escape_angles_only(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn format_markdown_item(item: &crate::domain::ProcessedItem) -> String {
    let raw_title = item.title.trim().replace(['[', ']'], "");
    let title = escape_markdown_raw_html(&raw_title);
    let link = match sanitize_http_url(&item.canonical_url) {
        Some(safe_url) => format!("[{title}]({safe_url})"),
        None => title,
    };
    let keywords = if item.match_result.matched_keywords.is_empty() {
        String::new()
    } else {
        let escaped: Vec<String> = item
            .match_result
            .matched_keywords
            .iter()
            .map(|kw| escape_markdown_metadata(kw))
            .collect();
        format!(" _(matched: {})_", escaped.join(", "))
    };
    format!(
        "- [{}] {link}{keywords}",
        item.match_result.priority.as_str().to_uppercase()
    )
}
