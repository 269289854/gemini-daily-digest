//! Watchlist keyword and exclude-keyword matching with priority escalation.

use std::collections::HashSet;

use crate::domain::{AlertPriority, WatchlistRuleInput};

/// Result of evaluating all enabled watchlist rules against a single item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchlistMatch {
    pub matched_keywords: Vec<String>,
    pub priority: AlertPriority,
    pub notify_immediately: bool,
}

/// Numeric priority ordering (`Urgent > High > Normal`).
#[must_use]
pub const fn priority_rank(priority: AlertPriority) -> u8 {
    match priority {
        AlertPriority::Normal => 0,
        AlertPriority::High => 1,
        AlertPriority::Urgent => 2,
    }
}

/// Evaluates enabled watchlist rules case-insensitively across `title`, `summary`, and `author`.
#[must_use]
pub fn match_watchlist(
    title: &str,
    summary: Option<&str>,
    author: Option<&str>,
    rules: &[WatchlistRuleInput],
) -> WatchlistMatch {
    let haystack = format!(
        "{}\n{}\n{}",
        title.to_lowercase(),
        summary.unwrap_or("").to_lowercase(),
        author.unwrap_or("").to_lowercase()
    );

    let mut matched_keywords = Vec::new();
    let mut seen_lower = HashSet::new();
    let mut priority = AlertPriority::Normal;
    let mut notify_immediately = false;

    for rule in rules.iter().filter(|r| r.enabled) {
        let excluded = rule.exclude_keywords.iter().any(|kw| {
            let needle = kw.trim().to_lowercase();
            !needle.is_empty() && haystack.contains(&needle)
        });
        if excluded {
            continue;
        }

        let mut rule_matched = false;
        for kw in &rule.keywords {
            let trimmed = kw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let needle = trimmed.to_lowercase();
            if haystack.contains(&needle) {
                rule_matched = true;
                if seen_lower.insert(needle) {
                    matched_keywords.push(trimmed.to_owned());
                }
            }
        }

        if rule_matched {
            if priority_rank(rule.priority) > priority_rank(priority) {
                priority = rule.priority;
            }
            notify_immediately |= rule.notify_immediately;
        }
    }

    WatchlistMatch {
        matched_keywords,
        priority,
        notify_immediately,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_watchlist_respects_exclude_keywords_and_escalates_priority() {
        let rules = vec![
            WatchlistRuleInput {
                id: None,
                name: "General Gemini".to_owned(),
                keywords: vec!["Gemini".to_owned()],
                exclude_keywords: vec!["rumor".to_owned()],
                priority: AlertPriority::High,
                notify_immediately: false,
                enabled: true,
            },
            WatchlistRuleInput {
                id: None,
                name: "Breaking Security".to_owned(),
                keywords: vec!["security".to_owned()],
                exclude_keywords: Vec::new(),
                priority: AlertPriority::Urgent,
                notify_immediately: true,
                enabled: true,
            },
            WatchlistRuleInput {
                id: None,
                name: "Disabled Rule".to_owned(),
                keywords: vec!["patch".to_owned()],
                exclude_keywords: Vec::new(),
                priority: AlertPriority::Urgent,
                notify_immediately: true,
                enabled: false,
            },
        ];

        let suppressed = match_watchlist(
            "Gemini 3.0 rumor leaks",
            Some("Unverified rumor"),
            None,
            &rules,
        );
        assert!(suppressed.matched_keywords.is_empty());
        assert_eq!(suppressed.priority, AlertPriority::Normal);
        assert!(!suppressed.notify_immediately);

        let escalated = match_watchlist(
            "Gemini API security patch",
            Some("Critical update"),
            Some("GoogleSecurity"),
            &rules,
        );
        assert_eq!(escalated.matched_keywords, vec!["Gemini", "security"]);
        assert_eq!(escalated.priority, AlertPriority::Urgent);
        assert!(escalated.notify_immediately);
    }
}
