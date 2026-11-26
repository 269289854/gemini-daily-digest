//! Windows system tray integration, show/hide toggling, and `desktop://navigate` emission.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::mcp_bridge::McpBridgeState;

/// Canonical label of the primary desktop WebView window.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Tauri event emitted when the tray menu or MCP tool requests frontend navigation.
pub const EVENT_DESKTOP_NAVIGATE: &str = "desktop://navigate";

const ROUTE_DAILY: &str = "/daily";
const ROUTE_SOURCES: &str = "/sources";
const ROUTE_SETTINGS: &str = "/settings";

const TRAY_ID: &str = "main-tray";
const MENU_TOGGLE_ID: &str = "tray_toggle";
const MENU_DAILY_ID: &str = "tray_daily";
const MENU_SOURCES_ID: &str = "tray_sources";
const MENU_SETTINGS_ID: &str = "tray_settings";
const MENU_QUIT_ID: &str = "tray_quit";

/// Tracks intentional application termination so close-to-tray only intercepts window closes.
#[derive(Debug, Default)]
pub struct AppExitState {
    quitting: AtomicBool,
}

impl AppExitState {
    /// Returns `true` when the user initiated an explicit Quit action.
    #[must_use]
    pub fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    /// Marks the process as intentionally exiting.
    pub fn mark_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }
}

/// Event payload matching `DesktopNavigateEventPayload` in `apps/web/src/platform/bridge.ts`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNavigatePayload {
    pub route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, String>>,
}

/// Restores, unhides, and focuses the main application window.
pub fn show_and_focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if window.is_minimized().unwrap_or(false) {
            if let Err(err) = window.unminimize() {
                tracing::warn!("failed to unminimize main window: {err}");
            }
        }
        if let Err(err) = window.show() {
            tracing::warn!("failed to show main window: {err}");
        }
        if let Err(err) = window.set_focus() {
            tracing::warn!("failed to focus main window: {err}");
        }
    }
}

/// Toggles visibility of the main window, restoring it when hidden or minimized.
pub fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let is_visible = window.is_visible().unwrap_or(false);
        let is_minimized = window.is_minimized().unwrap_or(false);
        if is_visible && !is_minimized {
            if let Err(err) = window.hide() {
                tracing::warn!("failed to hide main window: {err}");
            }
        } else {
            show_and_focus_main_window(app);
        }
    }
}

/// Focuses the main window and emits `desktop://navigate` to the React router.
pub fn emit_desktop_navigate(
    app: &AppHandle,
    route: &str,
    params: Option<BTreeMap<String, String>>,
) -> Result<(), String> {
    show_and_focus_main_window(app);
    let payload = DesktopNavigatePayload {
        route: route.to_owned(),
        params: params.filter(|map| !map.is_empty()),
    };
    app.emit(EVENT_DESKTOP_NAVIGATE, payload)
        .map_err(|err| format!("failed to emit {EVENT_DESKTOP_NAVIGATE}: {err}"))
}

/// Builds and registers the Windows system tray icon and context menu.
pub fn setup_tray(app: &AppHandle) -> Result<(), tauri::Error> {
    let toggle_item = MenuItem::with_id(app, MENU_TOGGLE_ID, "Show / Hide", true, None::<&str>)?;
    let daily_item =
        MenuItem::with_id(app, MENU_DAILY_ID, "Open Daily Digest", true, None::<&str>)?;
    let sources_item =
        MenuItem::with_id(app, MENU_SOURCES_ID, "Sources Explorer", true, None::<&str>)?;
    let settings_item =
        MenuItem::with_id(app, MENU_SETTINGS_ID, "Settings & MCP", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT_ID, "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &toggle_item,
            &daily_item,
            &sources_item,
            &settings_item,
            &separator,
            &quit_item,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Gemini Daily Digest")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            MENU_TOGGLE_ID => toggle_main_window(app),
            MENU_DAILY_ID => {
                if let Err(err) = emit_desktop_navigate(app, ROUTE_DAILY, None) {
                    tracing::warn!("{err}");
                }
            }
            MENU_SOURCES_ID => {
                if let Err(err) = emit_desktop_navigate(app, ROUTE_SOURCES, None) {
                    tracing::warn!("{err}");
                }
            }
            MENU_SETTINGS_ID => {
                if let Err(err) = emit_desktop_navigate(app, ROUTE_SETTINGS, None) {
                    tracing::warn!("{err}");
                }
            }
            MENU_QUIT_ID => {
                app.state::<AppExitState>().mark_quitting();
                let bridge = app.state::<McpBridgeState>().inner().clone();
                tauri::async_runtime::block_on(bridge.shutdown());
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    let _tray = builder.build(app)?;
    Ok(())
}
