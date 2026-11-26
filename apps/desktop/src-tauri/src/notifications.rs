//! Windows native toast notification validation and Tauri IPC command.

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

const MAX_NOTIFICATION_TITLE_CHARS: usize = 128;
const MAX_NOTIFICATION_BODY_CHARS: usize = 512;

/// Validates and normalizes notification title and body strings.
pub fn validate_notification_input(title: &str, body: &str) -> Result<(String, String), String> {
    let trimmed_title = title.trim();
    if trimmed_title.is_empty() {
        return Err("notification title must not be empty".to_owned());
    }
    let trimmed_body = body.trim();
    if trimmed_body.is_empty() {
        return Err("notification body must not be empty".to_owned());
    }

    Ok((
        truncate_chars(trimmed_title, MAX_NOTIFICATION_TITLE_CHARS),
        truncate_chars(trimmed_body, MAX_NOTIFICATION_BODY_CHARS),
    ))
}

fn truncate_chars(raw: &str, max_chars: usize) -> String {
    if raw.chars().count() <= max_chars {
        return raw.to_owned();
    }
    let keep = max_chars.saturating_sub(3);
    let mut out: String = raw.chars().take(keep).collect();
    out.push_str("...");
    out
}

/// Sends a Windows native notification via `tauri-plugin-notification`.
#[tauri::command]
pub async fn send_native_notification(
    app: AppHandle,
    title: String,
    body: String,
) -> Result<(), String> {
    let (safe_title, safe_body) = validate_notification_input(&title, &body)?;
    app.notification()
        .builder()
        .title(&safe_title)
        .body(&safe_body)
        .show()
        .map_err(|err| format!("failed to display native notification: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_non_empty_and_bounds_notification_text() {
        assert!(validate_notification_input("   ", "Body").is_err());
        assert!(validate_notification_input("Title", "  ").is_err());

        let (t, b) = validate_notification_input("  Daily Digest  ", "  3 new updates  ").unwrap();
        assert_eq!(t, "Daily Digest");
        assert_eq!(b, "3 new updates");

        let long_title = "A".repeat(200);
        let long_body = "B".repeat(700);
        let (bounded_t, bounded_b) = validate_notification_input(&long_title, &long_body).unwrap();
        assert_eq!(bounded_t.chars().count(), MAX_NOTIFICATION_TITLE_CHARS);
        assert!(bounded_t.ends_with("..."));
        assert_eq!(bounded_b.chars().count(), MAX_NOTIFICATION_BODY_CHARS);
        assert!(bounded_b.ends_with("..."));
    }
}
