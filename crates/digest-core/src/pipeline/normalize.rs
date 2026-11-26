//! URL canonicalization and tracking parameter stripping.

use crate::constants::TRACKING_QUERY_PARAMS;
use crate::error::CoreError;

const UTM_QUERY_PREFIX: &str = "utm_";

fn is_tracking_param(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.starts_with(UTM_QUERY_PREFIX) || TRACKING_QUERY_PARAMS.contains(&lower.as_str())
}

/// Normalizes a candidate item URL:
/// - Lowercases scheme and host
/// - Strips default ports (`80` for `http`, `443` for `https`)
/// - Strips tracking query parameters (`TRACKING_QUERY_PARAMS` and `utm_*`)
/// - Sorts remaining query parameters deterministically
/// - Strips empty fragments (`#`)
pub fn normalize_url(raw_url: &str) -> Result<String, CoreError> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidInput(
            "item URL must not be empty".to_owned(),
        ));
    }

    let mut parsed = url::Url::parse(trimmed)
        .map_err(|err| CoreError::InvalidInput(format!("invalid item URL `{trimmed}`: {err}")))?;

    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(CoreError::InvalidInput(format!(
            "unsupported item URL scheme `{scheme}`"
        )));
    }

    if let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) {
        let _ = parsed.set_host(Some(&host));
    }

    if (scheme == "http" && parsed.port() == Some(80))
        || (scheme == "https" && parsed.port() == Some(443))
    {
        let _ = parsed.set_port(None);
    }

    let mut kept_pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !is_tracking_param(key))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    kept_pairs.sort();

    if kept_pairs.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept_pairs);
    }

    if parsed.fragment() == Some("") {
        parsed.set_fragment(None);
    }

    Ok(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_strips_tracking_ports_empty_fragment_and_sorts_params() {
        let input =
            "HTTPS://Example.COM:443/path/to/post?z=2&utm_source=x&fbclid=123&a=1&utm_custom=foo#";
        let normalized = normalize_url(input).unwrap();
        assert_eq!(normalized, "https://example.com/path/to/post?a=1&z=2");
    }

    #[test]
    fn normalize_url_removes_empty_query_after_stripping_all_params() {
        let input = "http://EXAMPLE.org:80/news?utm_medium=email&ref=hn#section-1";
        let normalized = normalize_url(input).unwrap();
        assert_eq!(normalized, "http://example.org/news#section-1");
    }
}
