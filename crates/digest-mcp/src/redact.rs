//! Defense-in-depth sanitization applied to all MCP tool outputs and error messages.
//!
//! Ensures decrypted credentials, credential hints, URL userinfo, sensitive query
//! parameters, Bearer tokens, and API key prefixes are never returned to MCP callers.

use digest_core::constants::SECRET_MASK_REDACTED_PLACEHOLDER;
use serde_json::{Map, Value};
use url::form_urlencoded;
use url::Url;

const STRIPPED_KEYS: &[&str] = &[
    "configHint",
    "config_hint",
    "hasCredentials",
    "has_credentials",
    "apiKeyHint",
    "api_key_hint",
    "hasApiKey",
    "has_api_key",
    "credentials",
];

const URL_KEYS: &[&str] = &["handleOrUrl", "handle_or_url"];

const ERROR_KEYS: &[&str] = &[
    "lastError",
    "last_error",
    "errorSummary",
    "error_summary",
    "errorMessage",
    "error_message",
];

const SENSITIVE_QUERY_KEYS: &[&str] = &[
    "token",
    "key",
    "secret",
    "password",
    "passwd",
    "sig",
    "signature",
    "auth",
    "api_key",
    "apikey",
    "access_token",
    "client_secret",
    "bearer",
    "credential",
    "credentials",
];

/// Recursively sanitizes any JSON value returned by an MCP tool.
#[must_use]
pub fn sanitize_mcp_output(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_mcp_output).collect()),
        Value::Object(map) => {
            let mut clean = Map::with_capacity(map.len());
            for (key, val) in map {
                if STRIPPED_KEYS.iter().any(|blocked| *blocked == key) {
                    continue;
                }
                let sanitized_val = if URL_KEYS.iter().any(|k| *k == key) {
                    match val {
                        Value::String(raw) => Value::String(sanitize_url_or_handle(&raw)),
                        other => sanitize_mcp_output(other),
                    }
                } else if ERROR_KEYS.iter().any(|k| *k == key) {
                    match val {
                        Value::String(raw) => Value::String(sanitize_error_text(&raw)),
                        other => sanitize_mcp_output(other),
                    }
                } else {
                    sanitize_mcp_output(val)
                };
                clean.insert(key, sanitized_val);
            }
            Value::Object(clean)
        }
        other => other,
    }
}

/// Sanitizes a `handleOrUrl` string: strips `user:pass@` userinfo and masks
/// sensitive query parameters with `****`. Non-URL handles are returned unchanged.
#[must_use]
pub fn sanitize_url_or_handle(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.contains("://") {
        return raw.to_owned();
    }

    let Ok(mut parsed) = Url::parse(trimmed) else {
        return raw.to_owned();
    };

    if !parsed.username().is_empty() {
        let _ = parsed.set_username("");
    }
    if parsed.password().is_some() {
        let _ = parsed.set_password(None);
    }

    if let Some(raw_query) = parsed.query().map(str::to_owned) {
        let pairs: Vec<(String, String)> = form_urlencoded::parse(raw_query.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if !pairs.is_empty() {
            let mut rebuilt_parts = Vec::with_capacity(pairs.len());
            for (key, value) in pairs {
                let encoded_key: String = form_urlencoded::byte_serialize(key.as_bytes()).collect();
                if is_sensitive_query_key(&key) {
                    rebuilt_parts.push(format!("{encoded_key}={SECRET_MASK_REDACTED_PLACEHOLDER}"));
                } else {
                    let encoded_val: String =
                        form_urlencoded::byte_serialize(value.as_bytes()).collect();
                    rebuilt_parts.push(format!("{encoded_key}={encoded_val}"));
                }
            }
            let joined = rebuilt_parts.join("&");
            parsed.set_query(Some(&joined));
        }
    }

    parsed.into()
}

/// Masks `Bearer <token>`, `sk-...` keys, embedded URL userinfo/query secrets,
/// and inline `key=value` secret pairs inside error/diagnostic strings.
#[must_use]
pub fn sanitize_error_text(raw: &str) -> String {
    let step1 = mask_embedded_urls(raw);
    let step2 = mask_bearer_tokens(&step1);
    let step3 = mask_sk_tokens(&step2);
    mask_inline_kv_secrets(&step3)
}

fn is_sensitive_query_key(key: &str) -> bool {
    let lower = key.trim().to_ascii_lowercase();
    SENSITIVE_QUERY_KEYS
        .iter()
        .any(|candidate| *candidate == lower)
}

fn mask_embedded_urls(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(pos) = find_next_url_start(rest) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        let end = after
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ']' | '}' | ',')
            })
            .unwrap_or(after.len());
        let raw_url = &after[..end];
        let trimmed_url = raw_url.trim_end_matches(['.', ';', ':']);
        out.push_str(&sanitize_url_or_handle(trimmed_url));
        out.push_str(&raw_url[trimmed_url.len()..]);
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn find_next_url_start(haystack: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    for idx in 0..bytes.len() {
        let rem = &bytes[idx..];
        if ((rem.len() >= 7 && rem[..7].eq_ignore_ascii_case(b"http://"))
            || (rem.len() >= 8 && rem[..8].eq_ignore_ascii_case(b"https://")))
            && haystack.is_char_boundary(idx)
        {
            return Some(idx);
        }
    }
    None
}

fn mask_bearer_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        let rem = &bytes[idx..];
        if rem.len() >= 7 && rem[..7].eq_ignore_ascii_case(b"bearer ") {
            let boundary_ok =
                idx == 0 || !(bytes[idx - 1].is_ascii_alphanumeric() || bytes[idx - 1] == b'_');
            if boundary_ok {
                let mut tok_start = idx + 7;
                while tok_start < bytes.len() && matches!(bytes[tok_start], b' ' | b'\t') {
                    tok_start += 1;
                }
                if let Some(&q) = bytes.get(tok_start).filter(|&&b| b == b'"' || b == b'\'') {
                    let mut cursor = tok_start + 1;
                    while cursor < bytes.len()
                        && bytes[cursor] != q
                        && bytes[cursor] != b'\r'
                        && bytes[cursor] != b'\n'
                    {
                        cursor += 1;
                    }
                    if cursor > tok_start + 1 {
                        out.push_str("Bearer ");
                        out.push(q as char);
                        out.push_str(SECRET_MASK_REDACTED_PLACEHOLDER);
                        if bytes.get(cursor) == Some(&q) {
                            out.push(q as char);
                            idx = cursor + 1;
                        } else {
                            idx = cursor;
                        }
                        continue;
                    }
                } else {
                    let mut cursor = tok_start;
                    while cursor < bytes.len()
                        && !bytes[cursor].is_ascii_whitespace()
                        && !matches!(
                            bytes[cursor],
                            b'"' | b'\'' | b',' | b';' | b')' | b']' | b'}'
                        )
                    {
                        cursor += 1;
                    }
                    if cursor > tok_start {
                        out.push_str("Bearer ");
                        out.push_str(SECRET_MASK_REDACTED_PLACEHOLDER);
                        idx = cursor;
                        continue;
                    }
                }
            }
        }
        if let Some(ch) = input.get(idx..).and_then(|s| s.chars().next()) {
            out.push(ch);
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    out
}

fn mask_sk_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        let rem = &bytes[idx..];
        if rem.len() >= 3 && rem[..3].eq_ignore_ascii_case(b"sk-") {
            let boundary_ok =
                idx == 0 || !(bytes[idx - 1].is_ascii_alphanumeric() || bytes[idx - 1] == b'_');
            if boundary_ok {
                let mut cursor = idx + 3;
                while cursor < bytes.len()
                    && (bytes[cursor].is_ascii_alphanumeric()
                        || bytes[cursor] == b'_'
                        || bytes[cursor] == b'-')
                {
                    cursor += 1;
                }
                if cursor.saturating_sub(idx + 3) >= 4 {
                    out.push_str("sk-");
                    out.push_str(SECRET_MASK_REDACTED_PLACEHOLDER);
                    idx = cursor;
                    continue;
                }
            }
        }
        if let Some(ch) = input.get(idx..).and_then(|s| s.chars().next()) {
            out.push(ch);
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    out
}

fn mask_inline_kv_secrets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        let rem = &bytes[idx..];
        let mut matched_val_start = None;
        for key in SENSITIVE_QUERY_KEYS {
            if rem.len() > key.len() && rem[..key.len()].eq_ignore_ascii_case(key.as_bytes()) {
                let next_byte = bytes[idx + key.len()];
                if next_byte == b'=' || next_byte == b':' {
                    let prev_ok = idx == 0
                        || !(bytes[idx - 1].is_ascii_alphanumeric() || bytes[idx - 1] == b'_');
                    if prev_ok {
                        let mut val_start = idx + key.len() + 1;
                        while val_start < bytes.len() && matches!(bytes[val_start], b' ' | b'\t') {
                            val_start += 1;
                        }
                        matched_val_start = Some(val_start);
                        break;
                    }
                }
            }
        }

        if let Some(val_start) = matched_val_start {
            if let Some(&q) = bytes.get(val_start).filter(|&&b| b == b'"' || b == b'\'') {
                let mut cursor = val_start + 1;
                while cursor < bytes.len()
                    && bytes[cursor] != q
                    && bytes[cursor] != b'\r'
                    && bytes[cursor] != b'\n'
                {
                    cursor += 1;
                }
                if cursor > val_start + 1 {
                    out.push_str(&input[idx..val_start]);
                    out.push(q as char);
                    out.push_str(SECRET_MASK_REDACTED_PLACEHOLDER);
                    if bytes.get(cursor) == Some(&q) {
                        out.push(q as char);
                        idx = cursor + 1;
                    } else {
                        idx = cursor;
                    }
                    continue;
                }
            } else {
                let mut cursor = val_start;
                while cursor < bytes.len()
                    && !bytes[cursor].is_ascii_whitespace()
                    && !matches!(
                        bytes[cursor],
                        b'&' | b',' | b';' | b')' | b']' | b'}' | b'"' | b'\''
                    )
                {
                    cursor += 1;
                }
                if cursor > val_start {
                    out.push_str(&input[idx..val_start]);
                    out.push_str(SECRET_MASK_REDACTED_PLACEHOLDER);
                    idx = cursor;
                    continue;
                }
            }
        }

        if let Some(ch) = input.get(idx..).and_then(|s| s.chars().next()) {
            out.push(ch);
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn sanitize_mcp_output_strips_credential_fields_and_sanitizes_urls_and_errors() {
        let fake_sk = ["sk-", "test", "0123456789abcdef"].concat();
        let raw = json!({
            "sources": [
                {
                    "id": "src-1",
                    "handleOrUrl": "https://user:secretpass@feeds.example.com/rss?token=topsecret&page=2&api_key=abc123",
                    "hasCredentials": true,
                    "configHint": "test****9999",
                    "apiKeyHint": "sk-****1234",
                    "hasApiKey": true,
                    "credentials": { "apiKey": "plain-secret" },
                    "lastError": format!("auth failed with Bearer my-jwt-token and {fake_sk} at https://u:p@api.example.com/v1?access_token=xyz")
                }
            ],
            "nested": {
                "errorSummary": "upstream returned token=secret_val_99",
                "errorMessage": format!("Bearer leaked_tok_abc; key={fake_sk}")
            }
        });

        let clean = sanitize_mcp_output(raw);
        let src = &clean["sources"][0];
        assert!(src.get("hasCredentials").is_none());
        assert!(src.get("configHint").is_none());
        assert!(src.get("apiKeyHint").is_none());
        assert!(src.get("hasApiKey").is_none());
        assert!(src.get("credentials").is_none());

        let url = src["handleOrUrl"].as_str().unwrap();
        assert!(!url.contains("user"));
        assert!(!url.contains("secretpass"));
        assert!(!url.contains("topsecret"));
        assert!(!url.contains("abc123"));
        assert!(url.contains("token=****"));
        assert!(url.contains("api_key=****"));
        assert!(url.contains("page=2"));

        let err = src["lastError"].as_str().unwrap();
        assert!(!err.contains("my-jwt-token"));
        assert!(!err.contains(&fake_sk));
        assert!(!err.contains("u:p@"));
        assert!(!err.contains("access_token=xyz"));
        assert!(err.contains("Bearer ****"));
        assert!(err.contains("sk-****"));
        assert!(err.contains("access_token=****"));

        let summary = clean["nested"]["errorSummary"].as_str().unwrap();
        assert_eq!(summary, "upstream returned token=****");

        let msg = clean["nested"]["errorMessage"].as_str().unwrap();
        assert!(!msg.contains("leaked_tok_abc"));
        assert!(!msg.contains(&fake_sk));
    }

    #[test]
    fn sanitize_url_or_handle_preserves_plain_handles() {
        assert_eq!(sanitize_url_or_handle("google/gemini"), "google/gemini");
        assert_eq!(sanitize_url_or_handle("@GoogleDeepMind"), "@GoogleDeepMind");
    }

    #[test]
    fn sanitize_error_text_masks_short_and_uppercase_sk_tokens_without_over_redacting_words() {
        let out = sanitize_error_text(
            "failed: Bearer super-secret, sk-123456, SK-abcd, \"sk-123456\", (sk-123456), key=sk-123456",
        );
        assert!(!out.contains("super-secret"));
        assert!(!out.contains("123456"));
        assert!(!out.contains("abcd"));
        assert!(out.contains("Bearer ****"));
        assert!(out.contains("sk-****"));
        assert!(out.contains("\"sk-****\""));
        assert!(out.contains("(sk-****)"));
        assert!(out.contains("key=****"));

        let preserved = sanitize_error_text("task-123456 and disk-123456 and my_sk-123456");
        assert_eq!(preserved, "task-123456 and disk-123456 and my_sk-123456");

        let mcp_out = sanitize_mcp_output(json!({
            "lastError": "failed: Bearer super-secret and sk-123456 and https://user:pass@feed.example/rss?token=abc&api_key=xyz"
        }));
        let last_error = mcp_out["lastError"].as_str().unwrap();
        assert!(!last_error.contains("super-secret"));
        assert!(!last_error.contains("123456"));
        assert!(!last_error.contains("user:pass"));
        assert!(!last_error.contains("token=abc"));
        assert!(!last_error.contains("api_key=xyz"));
        assert!(last_error.contains("Bearer ****"));
        assert!(last_error.contains("sk-****"));
        assert!(last_error.contains("https://feed.example/rss?token=****&api_key=****"));
    }

    #[test]
    fn sanitize_error_text_strips_userinfo_and_masks_query_for_mixed_case_urls() {
        let upper = sanitize_error_text("HTTPS://user:pass@feed.example/rss?token=abc&api_key=xyz");
        assert!(!upper.contains("user"));
        assert!(!upper.contains("pass"));
        assert!(!upper.contains("abc"));
        assert!(!upper.contains("xyz"));
        assert_eq!(upper, "https://feed.example/rss?token=****&api_key=****");

        let mixed = sanitize_error_text(
            "upstream at Http://alice:secret@feed.example/rss?token=abc&keep=1 failed",
        );
        assert!(!mixed.contains("alice"));
        assert!(!mixed.contains("secret"));
        assert!(!mixed.contains("abc"));
        assert_eq!(
            mixed,
            "upstream at http://feed.example/rss?token=****&keep=1 failed"
        );
    }

    #[test]
    fn sanitize_error_text_handles_multibyte_utf8_boundaries_without_panicking() {
        assert_eq!(sanitize_error_text("bearer中"), "bearer中");
        assert_eq!(sanitize_error_text("beareré"), "beareré");
        assert_eq!(sanitize_error_text("ke中"), "ke中");
        assert_eq!(sanitize_error_text("tokeé=1"), "tokeé=1");

        let mixed = sanitize_error_text("错误: bearer中 ke中文 sk-1234");
        assert_eq!(mixed, "错误: bearer中 ke中文 sk-****");

        let utf8_secret = sanitize_error_text("错误: Bearer 中文密码 token=\"秘密值\" key=中");
        assert!(!utf8_secret.contains("中文密码"));
        assert!(!utf8_secret.contains("秘密值"));
        assert_eq!(utf8_secret, "错误: Bearer **** token=\"****\" key=****");
    }

    #[test]
    fn sanitize_error_text_masks_quoted_and_colon_secrets_without_prefix_duplication() {
        let quoted = sanitize_error_text(
            "token=\"super-secret-val\" password='hunter2' api_key=\"xyz999\" empty_tok= token=&keep=1 Bearer \"super-secret\" Bearer \"\" Bearer 'tok1'",
        );
        assert!(!quoted.contains("super-secret-val"));
        assert!(!quoted.contains("hunter2"));
        assert!(!quoted.contains("xyz999"));
        assert!(!quoted.contains("super-secret"));
        assert!(!quoted.contains("tok1"));
        assert!(!quoted.contains("token=token="));
        assert!(!quoted.contains("password=password="));
        assert!(!quoted.contains("api_key=api_key="));
        assert!(!quoted.contains("Bearer Bearer"));
        assert_eq!(
            quoted,
            "token=\"****\" password='****' api_key=\"****\" empty_tok= token=&keep=1 Bearer \"****\" Bearer \"\" Bearer '****'"
        );

        let punct = sanitize_error_text(
            "url=(HTTPS://user:pass@feed.example/rss?token=abc&api_key=xyz), key=\"sk-987654\", upper=SK-abcd, keep=task-123456.",
        );
        assert!(!punct.contains("user"));
        assert!(!punct.contains("pass"));
        assert!(!punct.contains("abc"));
        assert!(!punct.contains("xyz"));
        assert!(!punct.contains("987654"));
        assert!(!punct.contains("abcd"));
        assert!(!punct.contains("key=key="));
        assert_eq!(
            punct,
            "url=(https://feed.example/rss?token=****&api_key=****), key=\"****\", upper=sk-****, keep=task-123456."
        );

        let colon_and_unclosed = sanitize_error_text(
            "token: \"abc-123\" api_key: xyz_999 Bearer \"unclosed-tok\nnext=ok",
        );
        assert!(!colon_and_unclosed.contains("abc-123"));
        assert!(!colon_and_unclosed.contains("xyz_999"));
        assert!(!colon_and_unclosed.contains("unclosed-tok"));
        assert_eq!(
            colon_and_unclosed,
            "token: \"****\" api_key: **** Bearer \"****\nnext=ok"
        );
    }

    #[test]
    fn sanitize_error_text_preserves_non_secret_word_boundaries() {
        assert_eq!(
            sanitize_error_text("monkey=123 task-123456 disk-123456 my_bearer=abc my_token=xyz"),
            "monkey=123 task-123456 disk-123456 my_bearer=abc my_token=xyz"
        );
    }
}
