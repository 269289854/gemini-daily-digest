//! Tauri 2 desktop application library entrypoint for Gemini Daily Digest.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod mcp_bridge;
pub mod notifications;
pub mod oidc_loopback;
pub mod tray;

use std::sync::Arc;

use tauri::{Manager, RunEvent, WindowEvent};

use crate::mcp_bridge::{McpBridgeState, TauriDesktopViewController};
use crate::oidc_loopback::OidcLoopbackState;
use crate::tray::AppExitState;

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .try_init();
}

/// Boots the Tauri 2 desktop runtime, plugins, tray, close-to-tray handler, and IPC commands.
pub fn run() {
    init_tracing();

    let oidc_state = match OidcLoopbackState::new() {
        Ok(state) => state,
        Err(err) => {
            tracing::error!("fatal OIDC loopback initialization error: {err}");
            return;
        }
    };

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_and_focus_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppExitState::default())
        .manage(oidc_state)
        .setup(|app| {
            let desktop_controller =
                Arc::new(TauriDesktopViewController::new(app.handle().clone()));
            let mcp_state = McpBridgeState::new(desktop_controller)?;
            app.manage(mcp_state);
            tray::setup_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == tray::MAIN_WINDOW_LABEL {
                    let exit_state = window.app_handle().state::<AppExitState>();
                    if !exit_state.is_quitting() {
                        api.prevent_close();
                        if let Err(err) = window.hide() {
                            tracing::warn!("failed to hide main window on close: {err}");
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            oidc_loopback::prepare_oidc_loopback,
            oidc_loopback::open_oidc_system_login,
            oidc_loopback::oidc_http_request,
            oidc_loopback::open_external_url,
            notifications::send_native_notification,
            mcp_bridge::sync_desktop_runtime_config,
            mcp_bridge::get_mcp_bridge_status,
            mcp_bridge::get_mcp_local_token,
            mcp_bridge::rotate_mcp_token,
        ]);

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(err) => {
            tracing::error!("failed to build Tauri desktop application: {err}");
            return;
        }
    };

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            if let Some(mcp_state) = app_handle.try_state::<McpBridgeState>() {
                let bridge = mcp_state.inner().clone();
                tauri::async_runtime::block_on(bridge.shutdown());
            }
        }
    });
}
