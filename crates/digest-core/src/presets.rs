//! Built-in Gemini Ecosystem Preset Pack definitions
//! (official blogs, changelogs, GitHub releases, Reddit, HN, Google News, V2EX, X templates).

use crate::constants::{
    PRESET_CATEGORY_COMMUNITY, PRESET_CATEGORY_NEWS, PRESET_CATEGORY_OFFICIAL,
    PRESET_CATEGORY_RELEASES, PRESET_CATEGORY_SOCIAL,
};
use crate::domain::{PresetSourceSpec, SourceScope, SourceType};

/// Returns the built-in Gemini Ecosystem Preset Pack source catalog.
#[must_use]
pub fn gemini_ecosystem_presets() -> Vec<PresetSourceSpec> {
    vec![
        // Official Google / DeepMind / Gemini blogs & RSS feeds
        PresetSourceSpec {
            canonical_key: "google-ai-blog".to_owned(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            default_name: "Google AI Blog".to_owned(),
            category: PRESET_CATEGORY_OFFICIAL.to_owned(),
            handle_or_url: "https://blog.google/technology/ai/rss/".to_owned(),
            description: "Official Google AI and Gemini announcements from The Keyword."
                .to_owned(),
            requires_user_credentials: false,
        },
        PresetSourceSpec {
            canonical_key: "google-deepmind-blog".to_owned(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            default_name: "Google DeepMind Blog".to_owned(),
            category: PRESET_CATEGORY_OFFICIAL.to_owned(),
            handle_or_url: "https://deepmind.google/blog/rss.xml".to_owned(),
            description: "Research breakthroughs and Gemini model releases from Google DeepMind."
                .to_owned(),
            requires_user_credentials: false,
        },
        PresetSourceSpec {
            canonical_key: "google-developers-blog".to_owned(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            default_name: "Google Developers Blog".to_owned(),
            category: PRESET_CATEGORY_OFFICIAL.to_owned(),
            handle_or_url: "https://developers.googleblog.com/feeds/posts/default".to_owned(),
            description: "Official developer guides, Gemini API updates, and SDK news."
                .to_owned(),
            requires_user_credentials: false,
        },
        // Gemini API & SDK changelogs / GitHub Releases
        PresetSourceSpec {
            canonical_key: "github-googleapis-python-genai".to_owned(),
            source_type: SourceType::GithubRelease,
            scope: SourceScope::SharedPublic,
            default_name: "Google GenAI Python SDK".to_owned(),
            category: PRESET_CATEGORY_RELEASES.to_owned(),
            handle_or_url: "googleapis/python-genai".to_owned(),
            description: "Official Python SDK releases for the Gemini Developer API and Vertex AI."
                .to_owned(),
            requires_user_credentials: false,
        },
        PresetSourceSpec {
            canonical_key: "github-google-gemini-generative-ai-js".to_owned(),
            source_type: SourceType::GithubRelease,
            scope: SourceScope::SharedPublic,
            default_name: "Google Gemini JS SDK".to_owned(),
            category: PRESET_CATEGORY_RELEASES.to_owned(),
            handle_or_url: "google-gemini/generative-ai-js".to_owned(),
            description: "JavaScript/TypeScript SDK releases and changelog notes.".to_owned(),
            requires_user_credentials: false,
        },
        PresetSourceSpec {
            canonical_key: "github-google-gemini-cookbook".to_owned(),
            source_type: SourceType::GithubRelease,
            scope: SourceScope::SharedPublic,
            default_name: "Gemini Cookbook".to_owned(),
            category: PRESET_CATEGORY_RELEASES.to_owned(),
            handle_or_url: "google-gemini/cookbook".to_owned(),
            description: "Official Gemini API recipes, examples, and release updates.".to_owned(),
            requires_user_credentials: false,
        },
        PresetSourceSpec {
            canonical_key: "github-google-adk-python".to_owned(),
            source_type: SourceType::GithubRelease,
            scope: SourceScope::SharedPublic,
            default_name: "Google Agent Development Kit (Python)".to_owned(),
            category: PRESET_CATEGORY_RELEASES.to_owned(),
            handle_or_url: "google/adk-python".to_owned(),
            description: "Releases for Google's open-source Agent Development Kit for Gemini."
                .to_owned(),
            requires_user_credentials: false,
        },
        // Reddit communities
        PresetSourceSpec {
            canonical_key: "reddit-geminiai".to_owned(),
            source_type: SourceType::Reddit,
            scope: SourceScope::SharedPublic,
            default_name: "r/GeminiAI".to_owned(),
            category: PRESET_CATEGORY_COMMUNITY.to_owned(),
            handle_or_url: "GeminiAI".to_owned(),
            description: "Community discussions, benchmarks, and workflows on r/GeminiAI."
                .to_owned(),
            requires_user_credentials: false,
        },
        PresetSourceSpec {
            canonical_key: "reddit-bard".to_owned(),
            source_type: SourceType::Reddit,
            scope: SourceScope::SharedPublic,
            default_name: "r/Bard".to_owned(),
            category: PRESET_CATEGORY_COMMUNITY.to_owned(),
            handle_or_url: "Bard".to_owned(),
            description: "Community news and Gemini app feedback on r/Bard.".to_owned(),
            requires_user_credentials: false,
        },
        // Hacker News query
        PresetSourceSpec {
            canonical_key: "hn-gemini".to_owned(),
            source_type: SourceType::HackerNews,
            scope: SourceScope::SharedPublic,
            default_name: "Hacker News: Gemini".to_owned(),
            category: PRESET_CATEGORY_COMMUNITY.to_owned(),
            handle_or_url: "Gemini".to_owned(),
            description: "Hacker News discussions matching Gemini.".to_owned(),
            requires_user_credentials: false,
        },
        // Google News query
        PresetSourceSpec {
            canonical_key: "gnews-google-gemini".to_owned(),
            source_type: SourceType::GoogleNews,
            scope: SourceScope::SharedPublic,
            default_name: "Google News: Google Gemini".to_owned(),
            category: PRESET_CATEGORY_NEWS.to_owned(),
            handle_or_url: "Google Gemini".to_owned(),
            description: "Google News RSS coverage for Google Gemini.".to_owned(),
            requires_user_credentials: false,
        },
        // V2EX RSS
        PresetSourceSpec {
            canonical_key: "v2ex-tech-rss".to_owned(),
            source_type: SourceType::Rss,
            scope: SourceScope::SharedPublic,
            default_name: "V2EX Tech Feed".to_owned(),
            category: PRESET_CATEGORY_COMMUNITY.to_owned(),
            handle_or_url: "https://www.v2ex.com/feed/tab/tech.xml".to_owned(),
            description: "V2EX tech community Atom/RSS feed for AI and developer discussions."
                .to_owned(),
            requires_user_credentials: false,
        },
        // Pluggable compliant X account/search templates (user-supplied credentials)
        PresetSourceSpec {
            canonical_key: "x-account-googledeepmind".to_owned(),
            source_type: SourceType::XAccount,
            scope: SourceScope::UserPrivate,
            default_name: "@GoogleDeepMind (X)".to_owned(),
            category: PRESET_CATEGORY_SOCIAL.to_owned(),
            handle_or_url: "GoogleDeepMind".to_owned(),
            description:
                "Official Google DeepMind X timeline template (requires user X API credentials)."
                    .to_owned(),
            requires_user_credentials: true,
        },
        PresetSourceSpec {
            canonical_key: "x-account-geminiapp".to_owned(),
            source_type: SourceType::XAccount,
            scope: SourceScope::UserPrivate,
            default_name: "@GeminiApp (X)".to_owned(),
            category: PRESET_CATEGORY_SOCIAL.to_owned(),
            handle_or_url: "GeminiApp".to_owned(),
            description:
                "Official Gemini App X timeline template (requires user X API credentials)."
                    .to_owned(),
            requires_user_credentials: true,
        },
        PresetSourceSpec {
            canonical_key: "x-search-gemini-api".to_owned(),
            source_type: SourceType::XSearch,
            scope: SourceScope::UserPrivate,
            default_name: "X Search: Gemini API".to_owned(),
            category: PRESET_CATEGORY_SOCIAL.to_owned(),
            handle_or_url: "\"Gemini API\" OR \"Google DeepMind\"".to_owned(),
            description:
                "Compliant X recent search template for Gemini API updates (requires user X API credentials)."
                    .to_owned(),
            requires_user_credentials: true,
        },
    ]
}

/// Looks up a preset specification by its `canonical_key`.
#[must_use]
pub fn find_preset_by_key(canonical_key: &str) -> Option<PresetSourceSpec> {
    let normalized = canonical_key.trim();
    if normalized.is_empty() {
        return None;
    }
    gemini_ecosystem_presets()
        .into_iter()
        .find(|preset| preset.canonical_key.eq_ignore_ascii_case(normalized))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn preset_pack_is_non_empty_with_unique_keys_and_all_required_types() {
        let presets = gemini_ecosystem_presets();
        assert!(!presets.is_empty());

        let mut keys = HashSet::new();
        let mut types = HashSet::new();
        for preset in &presets {
            assert!(
                keys.insert(preset.canonical_key.clone()),
                "duplicate canonical_key: {}",
                preset.canonical_key
            );
            types.insert(preset.source_type);
            assert!(!preset.default_name.trim().is_empty());
            assert!(!preset.handle_or_url.trim().is_empty());
        }

        for required in [
            SourceType::Rss,
            SourceType::GithubRelease,
            SourceType::Reddit,
            SourceType::HackerNews,
            SourceType::GoogleNews,
            SourceType::XAccount,
            SourceType::XSearch,
        ] {
            assert!(
                types.contains(&required),
                "missing required source type {required:?}"
            );
        }

        assert!(presets.iter().any(|p| p.canonical_key == "v2ex-tech-rss"));
    }

    #[test]
    fn x_templates_are_user_private_and_require_credentials() {
        let presets = gemini_ecosystem_presets();
        let x_presets: Vec<_> = presets
            .iter()
            .filter(|p| matches!(p.source_type, SourceType::XAccount | SourceType::XSearch))
            .collect();
        assert!(!x_presets.is_empty());

        for preset in x_presets {
            assert_eq!(preset.scope, SourceScope::UserPrivate);
            assert!(preset.requires_user_credentials);
        }

        for public_preset in presets
            .iter()
            .filter(|p| !matches!(p.source_type, SourceType::XAccount | SourceType::XSearch))
        {
            assert_eq!(public_preset.scope, SourceScope::SharedPublic);
            assert!(!public_preset.requires_user_credentials);
        }
    }

    #[test]
    fn find_preset_by_key_resolves_case_insensitively() {
        let found = find_preset_by_key("GOOGLE-AI-BLOG").unwrap();
        assert_eq!(found.canonical_key, "google-ai-blog");
        assert!(find_preset_by_key("unknown-preset-key").is_none());
        assert!(find_preset_by_key("").is_none());
    }
}
