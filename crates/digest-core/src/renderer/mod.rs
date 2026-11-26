//! Multi-format digest renderers (`HtmlEmailDigestRenderer`, `MarkdownDigestRenderer`,
//! `PlainTextDigestRenderer`) and `RendererRegistry`.

pub mod html_email;
pub mod markdown;
pub mod plain_text;

use chrono::Utc;
pub use html_email::HtmlEmailDigestRenderer;
pub use markdown::MarkdownDigestRenderer;
pub use plain_text::PlainTextDigestRenderer;

use crate::domain::{AlertPriority, ItemMatchResult, ProcessedItem, RenderInput};
use crate::error::CoreError;
use crate::traits::{DigestRenderer, RawCollectedItem, RenderedNotification, SummarizedDigest};

/// Registry holding HTML email, Markdown, and plain-text digest renderers.
#[derive(Debug, Default, Clone, Copy)]
pub struct RendererRegistry {
    html: HtmlEmailDigestRenderer,
    markdown: MarkdownDigestRenderer,
    plain_text: PlainTextDigestRenderer,
}

/// Alias matching the standard multi-format renderer role.
pub type StandardDigestRenderer = RendererRegistry;

impl RendererRegistry {
    /// Creates a `RendererRegistry` with default HTML, Markdown, and plain-text renderers.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            html: HtmlEmailDigestRenderer::new(),
            markdown: MarkdownDigestRenderer::new(),
            plain_text: PlainTextDigestRenderer::new(),
        }
    }

    /// Returns the HTML email renderer.
    #[must_use]
    pub const fn html(&self) -> &HtmlEmailDigestRenderer {
        &self.html
    }

    /// Returns the Markdown renderer.
    #[must_use]
    pub const fn markdown(&self) -> &MarkdownDigestRenderer {
        &self.markdown
    }

    /// Returns the plain-text renderer.
    #[must_use]
    pub const fn plain_text(&self) -> &PlainTextDigestRenderer {
        &self.plain_text
    }
}

impl DigestRenderer for RendererRegistry {
    fn render(
        &self,
        digest: &SummarizedDigest,
        items: &[RawCollectedItem],
        priority: AlertPriority,
    ) -> Result<RenderedNotification, CoreError> {
        let input = build_render_input_from_raw(digest, items, priority);
        self.render_structured(&input)
    }

    fn render_structured(&self, input: &RenderInput) -> Result<RenderedNotification, CoreError> {
        Ok(RenderedNotification {
            subject: format_subject(input),
            text_body: self.plain_text.render_plain_text(input),
            html_body: self.html.render_html(input),
            markdown_body: self.markdown.render_markdown(input),
            priority: input.priority,
        })
    }
}

pub(crate) fn format_subject(input: &RenderInput) -> String {
    let base = input.digest.title.trim();
    match input.priority {
        AlertPriority::Urgent => format!("[URGENT] {base}"),
        AlertPriority::High => format!("[HIGH] {base}"),
        AlertPriority::Normal => base.to_owned(),
    }
}

pub(crate) fn build_render_input_from_raw(
    digest: &SummarizedDigest,
    items: &[RawCollectedItem],
    priority: AlertPriority,
) -> RenderInput {
    let all_items: Vec<ProcessedItem> = items
        .iter()
        .map(|raw| ProcessedItem {
            external_id: raw.external_id.clone(),
            canonical_url: raw.url.clone(),
            content_hash: raw.url.clone(),
            title: raw.title.clone(),
            author: raw.author.clone(),
            summary: raw.summary.clone(),
            published_at: raw.published_at,
            source_id: None,
            source_name: None,
            category: None,
            match_result: ItemMatchResult {
                matched_keywords: Vec::new(),
                priority,
                importance_score: 0,
                notify_immediately: priority == AlertPriority::Urgent,
            },
        })
        .collect();

    let highlighted_items = if priority == AlertPriority::Normal {
        Vec::new()
    } else {
        all_items.clone()
    };

    RenderInput {
        digest_date: Utc::now().date_naive(),
        digest: digest.clone(),
        sections: Vec::new(),
        highlighted_items,
        all_items,
        priority,
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use uuid::Uuid;

    use super::*;
    use crate::domain::{DigestSection, ItemMatchResult, ProcessedItem, RenderInput};
    use crate::traits::SummarizedDigest;

    fn sample_render_input() -> RenderInput {
        let date = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        let xss_item = ProcessedItem {
            external_id: Some("1".to_owned()),
            canonical_url: "javascript:alert('xss')".to_owned(),
            content_hash: "h1".to_owned(),
            title: "Test <script>alert(1)</script> Title".to_owned(),
            author: Some("<b>Mallory</b>".to_owned()),
            summary: Some("Body with <img src=x onerror=alert(1)> payload.".to_owned()),
            published_at: None,
            source_id: Some(Uuid::new_v4()),
            source_name: Some("Security Feed".to_owned()),
            category: Some("Official".to_owned()),
            match_result: ItemMatchResult {
                matched_keywords: vec!["gemini-2.5".to_owned()],
                priority: AlertPriority::Urgent,
                importance_score: 90,
                notify_immediately: true,
            },
        };
        let safe_item = ProcessedItem {
            external_id: Some("2".to_owned()),
            canonical_url: "https://example.com/posts/gemini-sdk".to_owned(),
            content_hash: "h2".to_owned(),
            title: "Gemini SDK 2.5 Released".to_owned(),
            author: Some("Gemini Team".to_owned()),
            summary: Some("Official release announcement.".to_owned()),
            published_at: None,
            source_id: Some(Uuid::new_v4()),
            source_name: Some("Official Blog".to_owned()),
            category: Some("Official".to_owned()),
            match_result: ItemMatchResult {
                matched_keywords: vec!["sdk".to_owned()],
                priority: AlertPriority::High,
                importance_score: 60,
                notify_immediately: false,
            },
        };

        RenderInput {
            digest_date: date,
            digest: SummarizedDigest {
                title: "Gemini Daily Digest — 2026-11-26".to_owned(),
                overview_markdown: "Key highlights across the Gemini ecosystem.".to_owned(),
                prompt_tokens: 100,
                completion_tokens: 50,
                model_used: "deepseek-chat".to_owned(),
                degraded_fallback: false,
            },
            sections: vec![DigestSection {
                category: "Official".to_owned(),
                source_id: None,
                source_name: "Official Blog".to_owned(),
                summary_markdown: "Official releases and SDK updates.".to_owned(),
                key_points: vec!["SDK 2.5 released".to_owned()],
                items: vec![safe_item.clone(), xss_item.clone()],
            }],
            highlighted_items: vec![xss_item.clone()],
            all_items: vec![safe_item, xss_item],
            priority: AlertPriority::Urgent,
        }
    }

    #[test]
    fn html_renderer_escapes_scripts_neutralizes_javascript_links_and_has_gmail_dark_meta() {
        let input = sample_render_input();
        let registry = RendererRegistry::new();
        let rendered = registry.render_structured(&input).unwrap();

        assert!(rendered
            .html_body
            .contains(r#"<meta name="color-scheme" content="light dark">"#));
        assert!(rendered
            .html_body
            .contains(r#"<meta name="supported-color-schemes" content="light dark">"#));

        // XSS tags must be escaped, never injected raw.
        assert!(!rendered.html_body.contains("<script>alert(1)</script>"));
        assert!(rendered
            .html_body
            .contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!rendered.html_body.contains("<img src=x"));
        assert!(rendered.html_body.contains("&lt;img src=x"));

        // javascript: link must NEVER be rendered as an anchor href.
        assert!(!rendered.html_body.contains("javascript:alert"));
        assert!(rendered
            .html_body
            .contains(r#"href="https://example.com/posts/gemini-sdk""#));
    }

    #[test]
    fn markdown_and_plain_text_renderers_include_overview_alerts_and_items() {
        let mut input = sample_render_input();
        input.digest.degraded_fallback = true;
        let registry = RendererRegistry::new();
        let rendered = registry.render_structured(&input).unwrap();

        assert!(rendered.subject.starts_with("[URGENT]"));
        assert!(rendered
            .markdown_body
            .contains("Key highlights across the Gemini ecosystem."));
        assert!(rendered.markdown_body.contains("Highlighted Alerts (1)"));
        assert!(rendered.markdown_body.contains("Gemini SDK 2.5 Released"));
        assert!(rendered.markdown_body.contains("degraded fallback"));
        assert!(!rendered.markdown_body.contains("javascript:alert"));

        assert!(rendered.text_body.contains("EXECUTIVE OVERVIEW"));
        assert!(rendered.text_body.contains("HIGHLIGHTED ALERTS (1)"));
        assert!(rendered.text_body.contains("Gemini SDK 2.5 Released"));
        assert!(rendered.text_body.contains("[non-http link omitted]"));
    }

    #[test]
    fn markdown_renderer_escapes_raw_html_and_neutralizes_dangerous_links() {
        let mut input = sample_render_input();
        input.digest.title = "Digest <script>alert(1)</script> & Q1".to_owned();
        input.digest.overview_markdown = concat!(
            "> Valid blockquote with [Gemini](https://ai.google.dev) and `<script>in_code()</script>`\n",
            "Injected <script>alert(1)</script> and <img src=x onerror=alert(1)>\n",
            "Bad links: [click](javascript:alert(1)), [img](  JaVaScRiPt:alert(2) ), ",
            "[data](data:text/html,<script>alert(3)</script>), [vb](vbscript:msgbox(4))\n",
            "Good wiki: [Wiki](https://en.wikipedia.org/wiki/Gemini_(language_model))"
        )
        .to_owned();
        input.sections[0].source_name = "Feed <img src=x onerror=alert(1)>".to_owned();
        input.sections[0].category = "Cat & <script>x</script>".to_owned();
        input.sections[0].summary_markdown =
            "> Section quote\nSummary <script>alert(1)</script> [safe](http://example.com) [bad](javascript:alert(1))"
                .to_owned();
        input.sections[0].key_points = vec![
            "Point with <img src=x onerror=alert(1)> & [link](javascript:alert(1))".to_owned(),
            "Safe point with [Gemini](https://ai.google.dev)".to_owned(),
        ];
        input.highlighted_items[0].match_result.matched_keywords =
            vec!["<script>kw</script>".to_owned()];

        let rendered = MarkdownDigestRenderer::new().render_markdown(&input);

        // Raw HTML tags must never appear unescaped in metadata or markdown sections.
        assert!(!rendered.contains("<script>alert(1)</script>"));
        assert!(!rendered.contains("<script>x</script>"));
        assert!(!rendered.contains("<script>kw</script>"));
        assert!(!rendered.contains("<img src=x onerror=alert(1)>"));
        assert!(rendered.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(rendered.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(rendered.contains("Digest &lt;script&gt;alert(1)&lt;/script&gt; &amp; Q1"));

        // Dangerous link schemes must be neutralized while preserving labels.
        assert!(!rendered.contains("javascript:alert"));
        assert!(!rendered.contains("JaVaScRiPt:alert"));
        assert!(!rendered.contains("data:text/html"));
        assert!(!rendered.contains("vbscript:msgbox"));
        assert!(rendered.contains("Bad links: click, img, data, vb"));

        // Valid blockquotes, inline code, and http/https links must remain intact.
        assert!(rendered.contains("> Valid blockquote with [Gemini](https://ai.google.dev)"));
        assert!(rendered.contains("`<script>in_code()</script>`"));
        assert!(rendered.contains("> Section quote"));
        assert!(rendered.contains("[safe](http://example.com)"));
        assert!(rendered.contains("[Wiki](https://en.wikipedia.org/wiki/Gemini_(language_model))"));
    }

    #[test]
    fn markdown_renderer_neutralizes_entity_multiline_and_reference_def_bypasses() {
        let mut input = sample_render_input();
        input.digest.overview_markdown = concat!(
            "E1: [ent1](javascript&#58;alert(1))\n",
            "E2: [ent2](javascript&#x3a;alert(1))\n",
            "E3: [ent3](javascript&colon;alert(1))\n",
            "E4: [hexj](&#x6a;avascript:alert(1))\n",
            "E5: [angle](<javascript:alert(1)>)\n",
            "E6: <javascript:alert(1)>\n",
            "E7: [tabent](java&tab;script&colon;alert&lpar;1&rpar;)\n",
            "E8: [nlent](java&newline;script:alert(1))\n",
            "E9: [nosemi](javascript&#58alert(1))\n",
            "E10: [unkent](javascript&unknown;alert(1))\n",
            "E11: [bsol](java&bsol;script:alert(1))\n",
            "E12: [custom](custom:payload)\n",
            "M1: [multiline\n](javascript:alert(1))\n",
            "M2: [multiline2](\njavascript:alert(1)\n)\n",
            "R1: [reflink][r1]\n\n",
            "[r1]:\njavascript:alert(1)\n\n",
            "[multi_ref]:\n  data:text/html,<script>alert(1)</script>\n\n",
            "> > Nested quote with [Gemini](https://ai.google.dev) and [badq\n",
            "> > ](javascript:alert(1))\n",
            "> [qr1]:\n",
            "> javascript:alert(1)\n\n",
            "Safe inline code `[keep](javascript:alert(1))` and `<script>keep()</script>`.\n\n",
            "```markdown\n",
            "[fenced_keep](javascript:alert(1))\n",
            "<script>fenced_keep()</script>\n",
            "```\n\n",
            "Safe links: [Gemini](https://ai.google.dev), [Rel](/path?a=1&b=2), ",
            "[Frag](#section), [Mail](mailto:digest@example.com), ",
            "[Amp](https://example.com/?a=1&amp;b=2), ",
            "[MultiSafe\nLink](https://ai.google.dev/docs)\n\n",
            "[safe_ref]:\n  https://ai.google.dev"
        )
        .to_owned();

        let rendered = MarkdownDigestRenderer::new().render_markdown(&input);

        // Extract the Executive Overview section (before Per-Source Digests / Highlighted Alerts)
        // so fenced/inline code exceptions can be verified alongside dangerous-link absence.
        let overview_end = rendered
            .find("\n## Highlighted Alerts")
            .unwrap_or(rendered.len());
        let overview = &rendered[..overview_end];

        // Labels from neutralized inline links are preserved without dangerous URIs.
        assert!(overview.contains("E1: ent1\n"));
        assert!(overview.contains("E2: ent2\n"));
        assert!(overview.contains("E3: ent3\n"));
        assert!(overview.contains("E4: hexj\n"));
        assert!(overview.contains("E5: angle\n"));
        assert!(overview.contains("E6: &lt;javascript:alert(1)&gt;\n"));
        assert!(overview.contains("E7: tabent\n"));
        assert!(overview.contains("E8: nlent\n"));
        assert!(overview.contains("E9: nosemi\n"));
        assert!(overview.contains("E10: unkent\n"));
        assert!(overview.contains("E11: bsol\n"));
        assert!(overview.contains("E12: custom\n"));
        assert!(overview.contains("M1: multiline\n\n"));
        assert!(overview.contains("M2: multiline2\n"));

        // Reference link definitions with dangerous destinations are neutralized to `#`.
        assert!(overview.contains("[r1]: #\n"));
        assert!(overview.contains("[multi_ref]: #\n"));
        assert!(overview.contains("> [qr1]: #\n"));

        // None of the dangerous destinations / entities survive outside code blocks and escaped prose.
        let outside_code = overview
            .replace("`[keep](javascript:alert(1))`", "")
            .replace(
                "```markdown\n[fenced_keep](javascript:alert(1))\n<script>fenced_keep()</script>\n```",
                "",
            )
            .replace("&lt;javascript:alert(1)&gt;", "");
        assert!(!outside_code.contains("javascript&#58;"));
        assert!(!outside_code.contains("javascript&#x3a;"));
        assert!(!outside_code.contains("javascript&colon;"));
        assert!(!outside_code.contains("&#x6a;avascript"));
        assert!(!outside_code.contains("javascript:"));
        assert!(!outside_code.contains("data:text/html"));
        assert!(!outside_code.contains("custom:payload"));

        // Benign quotes, inline/fenced code, and safe links remain intact.
        assert!(overview.contains("> > Nested quote with [Gemini](https://ai.google.dev)"));
        assert!(overview.contains("`[keep](javascript:alert(1))`"));
        assert!(overview.contains("`<script>keep()</script>`"));
        assert!(overview.contains(
            "```markdown\n[fenced_keep](javascript:alert(1))\n<script>fenced_keep()</script>\n```"
        ));
        assert!(overview.contains("[Gemini](https://ai.google.dev)"));
        assert!(overview.contains("[Rel](/path?a=1&b=2)"));
        assert!(overview.contains("[Frag](#section)"));
        assert!(overview.contains("[Mail](mailto:digest@example.com)"));
        assert!(overview.contains("[Amp](https://example.com/?a=1&amp;b=2)"));
        assert!(overview.contains("[MultiSafe\nLink](https://ai.google.dev/docs)"));
        assert!(overview.contains("[safe_ref]:\n  https://ai.google.dev"));
    }

    fn is_http_https_mailto_or_relative(dest: &str) -> bool {
        let trimmed = dest.trim();
        let compact: String = trimmed
            .chars()
            .filter(|c| !c.is_whitespace() && !c.is_control() && *c != '\\')
            .collect();
        let lower = compact.to_ascii_lowercase();
        let scheme_prefix = match lower.find(['/', '?', '#', ':']) {
            Some(idx) => &lower[..idx],
            None => lower.as_str(),
        };
        if scheme_prefix.contains('&') {
            return false;
        }
        if let Some(colon_idx) = lower.find(':') {
            let first_delim = lower.find(['/', '?', '#']).unwrap_or(lower.len());
            if colon_idx < first_delim {
                return lower.starts_with("http://")
                    || lower.starts_with("https://")
                    || lower.starts_with("mailto:");
            }
        }
        true
    }

    fn assert_commonmark_ast_safe(markdown: &str, options: pulldown_cmark::Options) {
        use pulldown_cmark::{Event, LinkType, Parser, Tag};

        let parser = Parser::new_ext(markdown, options);
        for (label, link_def) in parser.reference_definitions().iter() {
            assert!(
                is_http_https_mailto_or_relative(link_def.dest.as_ref()),
                "unsafe reference definition [{label}] survived: {:?}",
                link_def.dest
            );
        }

        for event in parser {
            match event {
                Event::Html(raw) => panic!("raw Event::Html survived: {raw:?}"),
                Event::InlineHtml(raw) => panic!("raw Event::InlineHtml survived: {raw:?}"),
                Event::Start(Tag::HtmlBlock) => panic!("Tag::HtmlBlock survived"),
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    ..
                })
                | Event::Start(Tag::Image {
                    link_type,
                    dest_url,
                    ..
                }) if !matches!(link_type, LinkType::Email) => {
                    assert!(
                        is_http_https_mailto_or_relative(dest_url.as_ref()),
                        "unsafe link/image destination survived: {dest_url:?}"
                    );
                }
                _ => {}
            }
        }
    }

    #[test]
    fn markdown_renderer_neutralizes_challenger_vectors_nesting_and_preserves_ast_invariants() {
        let mut input = sample_render_input();
        input.digest.overview_markdown = concat!(
            // Q1-Q4: unclosed quotes in unbracketed destinations & adjacent evil links
            "Q1: [safe](https://example.com/a'b)[evil](javascript:alert(1)//')\n",
            "Q2: [safe2](https://example.com/a\"b)[evil2](javascript:alert(2)//\")\n",
            "Q3: [q3](javascript:alert('xss))\n",
            "Q4: [q4](javascript:alert(\"xss))\n",
            // L1-L3: list-container reference definitions
            "- [lr1]: javascript:alert(1)\n\n",
            "[x1][lr1]\n\n",
            "1. [lr2]: data:text/html,<script>alert(1)</script>\n\n",
            "[x2][lr2]\n\n",
            "> - [lr3]: javascript:alert(1)\n\n",
            "[x3][lr3]\n\n",
            // Nested / multi-pass vectors
            "N1: [[foo](javascript:alert(1))](javascript:alert(2))\n",
            "N2: ![[evil](javascript:alert(1))](https://safe.com/img.png)\n",
            "N3: [![safe](https://safe.com/img.png)](javascript:alert(1))\n",
            "N4: <[script](javascript:alert(1))>alert(1)</script>\n",
            // Multi-line InlineHtml and HtmlBlock inside blockquotes
            "> <img src=x\n",
            "> onerror=alert(1)>\n\n",
            "> <script>\n",
            "> alert(1)\n",
            "> </script>\n\n",
            // Duplicate refdefs (first shadows second)
            "[dup]: javascript:alert(1)\n",
            "[dup]: javascript:alert(2)\n\n",
            "[duplink][dup]\n\n",
            // Indented code block must remain untouched
            "    <script>alert(1)</script>\n",
            "    [x](javascript:alert(1))\n\n",
            // Backslash, HTML-entity/Unicode control chars, and Unicode whitespace schemes
            "B1: [bs](javascript\\:alert(1))\n",
            "B2: [ctrl](&#x01;javascript:alert(1))\n",
            "B3: [ctrl_angle](<\u{0001}javascript:alert(1)>)\n",
            "B4: [ctrl_c1](\u{0080}javascript:alert(1))\n",
            "B5: [nbsp](java\u{00A0}script:alert(1))\n",
            // Table cell containing dangerous link + HTML
            "| Col |\n",
            "| --- |\n",
            "| [tbl](javascript:alert(1)) <b>ok</b> |\n"
        )
        .to_owned();

        let rendered = MarkdownDigestRenderer::new().render_markdown(&input);

        let overview_start = rendered
            .find("## Executive Overview\n")
            .expect("missing overview header")
            + "## Executive Overview\n".len();
        let overview_end = rendered
            .find("\n## Highlighted Alerts")
            .unwrap_or(rendered.len());
        let overview = &rendered[overview_start..overview_end];

        // Q1-Q4 expectations
        assert!(overview.contains("Q1: [safe](https://example.com/a'b)evil\n"));
        assert!(overview.contains("Q2: [safe2](https://example.com/a\"b)evil2\n"));
        assert!(overview.contains("Q3: q3\n"));
        assert!(overview.contains("Q4: q4\n"));

        // L1-L3 expectations
        assert!(overview.contains("- [lr1]: #\n"));
        assert!(overview.contains("1. [lr2]: #\n"));
        assert!(overview.contains("> - [lr3]: #\n"));
        assert!(overview.contains("\nx1\n"));
        assert!(overview.contains("\nx2\n"));
        assert!(overview.contains("\nx3\n"));

        // Nested / multi-pass expectations
        assert!(overview.contains("N1: foo\n"));
        assert!(overview.contains("N2: ![evil](https://safe.com/img.png)\n"));
        assert!(overview.contains("N3: ![safe](https://safe.com/img.png)\n"));
        assert!(overview.contains("N4: &lt;script&gt;alert(1)&lt;/script&gt;\n"));

        // Blockquote prefixes preserved on multi-line InlineHtml & HtmlBlock
        assert!(overview.contains("> &lt;img src=x\n> onerror=alert(1)&gt;\n"));
        assert!(overview.contains("> &lt;script&gt;\n> alert(1)\n> &lt;/script&gt;\n"));

        // Duplicate refdef & link neutralized
        assert!(overview.contains("[dup]: #\n"));
        assert!(overview.contains("\nduplink\n"));

        // Indented code block preserved verbatim
        assert!(overview.contains("    <script>alert(1)</script>\n    [x](javascript:alert(1))\n"));

        // Backslash & Unicode control/whitespace schemes neutralized
        assert!(overview.contains("B1: bs\n"));
        assert!(overview.contains("B2: ctrl\n"));
        assert!(overview.contains("B3: ctrl_angle\n"));
        assert!(overview.contains("B4: ctrl_c1\n"));
        assert!(overview.contains("B5: nbsp\n"));

        // Re-parse AST invariants on both sanitized overview and full rendered Markdown
        // under CommonMark `Options::empty()` and GFM extensions.
        let gfm_opts = pulldown_cmark::Options::ENABLE_TABLES
            | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
            | pulldown_cmark::Options::ENABLE_TASKLISTS;
        assert_commonmark_ast_safe(overview, pulldown_cmark::Options::empty());
        assert_commonmark_ast_safe(overview, gfm_opts);
        assert_commonmark_ast_safe(&rendered, pulldown_cmark::Options::empty());
        assert_commonmark_ast_safe(&rendered, gfm_opts);
    }

    #[test]
    fn markdown_sanitizer_fail_closes_when_nesting_exceeds_max_passes() {
        let depth = crate::constants::MARKDOWN_SANITIZE_MAX_PASSES + 8;
        let mut payload = String::from("x");
        for _ in 0..depth {
            payload = format!("[{payload}](javascript:alert(1))");
        }
        payload.push_str("<script>alert(1)</script>");

        let sanitized = crate::renderer::markdown::sanitize_markdown_content(&payload);
        assert_commonmark_ast_safe(&sanitized, pulldown_cmark::Options::empty());
        assert_commonmark_ast_safe(
            &sanitized,
            pulldown_cmark::Options::ENABLE_TABLES
                | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
                | pulldown_cmark::Options::ENABLE_TASKLISTS,
        );
        assert_commonmark_ast_safe(&sanitized, pulldown_cmark::Options::ENABLE_WIKILINKS);
    }

    #[test]
    fn markdown_renderer_neutralizes_gfm_table_pipe_code_wikilink_key_points_and_metadata_bypasses()
    {
        let mut input = sample_render_input();
        input.digest.title = "Title [title_evil](javascript:alert(1))".to_owned();
        input.digest.model_used =
            "deepseek` [model_evil](javascript:alert(1)) <img src=x>".to_owned();
        input.digest.overview_markdown = concat!(
            "| h1 | h2 |\n",
            "| -- | -- |\n",
            "| `a | [evil](javascript:alert(1))` |\n",
            "| `a | <img src=x onerror=alert(1)>` |\n",
            "| `a | <javascript:alert(1)>` |\n",
            "| `a | [[javascript:alert(1)]]` |\n\n",
            "Wiki1: [[javascript:alert(1)]]\n",
            "Wiki2: [[javascript:alert(1)|evil_label]]\n",
            "FnRef: [fnlink][^fn]\n\n",
            "[^fn]: javascript:alert(1)\n\n",
            "CrossRef: [cross_link][xref]\n\n",
            "```"
        )
        .to_owned();

        input.sections[0].source_name = "Source [src_evil](javascript:alert(1))".to_owned();
        input.sections[0].category = "Cat [cat_evil](https://evil.example)".to_owned();
        input.sections[0].summary_markdown = concat!(
            "```\n",
            "[fence_break](javascript:alert(1))\n",
            "```\n\n",
            "[xref]: javascript:alert(1)"
        )
        .to_owned();
        input.sections[0].key_points = vec![
            "Point A:\n\n    [kp_link](javascript:alert(1))".to_owned(),
            "Safe point with [Gemini](https://ai.google.dev)".to_owned(),
        ];
        input.highlighted_items[0].match_result.matched_keywords = vec![
            "[kw_evil](javascript:alert(1))".to_owned(),
            r"\[kw_escaped\](javascript:alert(2))".to_owned(),
        ];

        let rendered = MarkdownDigestRenderer::new().render_markdown(&input);

        // B1-B3, wikilinks, footnote-like refdef, B4 key_points, B5 matched_keywords, and cross-section vectors
        assert!(rendered.contains("evil` |"));
        assert!(rendered.contains("&lt;img src=x onerror=alert(1)&gt;` |"));
        assert!(rendered.contains("&lt;javascript:alert(1)&gt;` |"));
        assert!(rendered.contains("[^fn]: #\n"));
        assert!(rendered.contains("[xref]: #"));
        assert!(rendered.contains("Point A:\n\n    kp_link"));
        assert!(rendered.contains("Safe point with [Gemini](https://ai.google.dev)"));
        assert!(!rendered.contains("<img src=x"));
        assert!(!rendered.contains("href=\"javascript:"));
        assert!(!rendered.contains("title_evil](javascript:"));
        assert!(!rendered.contains("model_evil](javascript:"));
        assert!(!rendered.contains("src_evil](javascript:"));
        assert!(!rendered.contains("cat_evil](https://evil.example)"));
        assert!(!rendered.contains("[kw_evil](javascript:alert(1))"));

        let gfm_opts = pulldown_cmark::Options::ENABLE_TABLES
            | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
            | pulldown_cmark::Options::ENABLE_TASKLISTS;
        let wiki_opts = pulldown_cmark::Options::ENABLE_WIKILINKS;
        let combined_opts = gfm_opts | wiki_opts;

        for opts in [
            pulldown_cmark::Options::empty(),
            gfm_opts,
            wiki_opts,
            combined_opts,
        ] {
            assert_commonmark_ast_safe(&rendered, opts);
            let parser = pulldown_cmark::Parser::new_ext(&rendered, opts);
            for event in parser {
                if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link {
                    dest_url, ..
                }) = event
                {
                    assert!(
                        !dest_url.as_ref().contains("evil.example"),
                        "Metadata link leaked into AST under {opts:?}: {dest_url:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn markdown_renderer_neutralizes_wikilinks_inside_gfm_table_rows() {
        let mut input = sample_render_input();
        input.digest.overview_markdown = concat!(
            "| Col |\n",
            "| --- |\n",
            "| [[javascript:alert(1)|evil]] |\n",
            "| [[javascript:alert(1) | evil]] |\n",
            "| [[safe_note]] |\n",
            "| [[safe_note|Safe Label]] |"
        )
        .to_owned();

        let sanitized_overview =
            crate::renderer::markdown::sanitize_markdown_content(&input.digest.overview_markdown);
        let rendered = MarkdownDigestRenderer::new().render_markdown(&input);

        let gfm_opts = pulldown_cmark::Options::ENABLE_TABLES
            | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
            | pulldown_cmark::Options::ENABLE_TASKLISTS;
        let wiki_opts = pulldown_cmark::Options::ENABLE_WIKILINKS;
        let combined_opts = gfm_opts | wiki_opts;

        for candidate in [sanitized_overview.as_str(), rendered.as_str()] {
            assert!(!candidate.contains("[[javascript:"));
            assert!(candidate.contains("[[safe_note]]"));
            assert!(candidate.contains("[[safe_note|Safe Label]]"));

            for opts in [
                pulldown_cmark::Options::empty(),
                gfm_opts,
                wiki_opts,
                combined_opts,
            ] {
                assert_commonmark_ast_safe(candidate, opts);
            }
        }
    }
}
