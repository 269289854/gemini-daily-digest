//! Embedded `digest-mcp` Streamable HTTP bridge lifecycle and `DesktopViewController`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use digest_core::constants::DEFAULT_MCP_BIND_HOST;
use digest_core::domain::{AuthMode, McpAccessMode};
use digest_core::error::CoreError;
use digest_mcp::client::DigestApiClient;
use digest_mcp::config::{
    generate_local_bearer_token, mask_token_hint, validate_server_base_url, McpRuntimeConfig,
    McpSharedState, DEFAULT_MCP_HTTP_PORT, DEFAULT_SERVER_URL,
};
use digest_mcp::desktop::{DesktopNavigationTarget, DesktopViewController};
use digest_mcp::redact::sanitize_error_text;
use digest_mcp::tools::build_default_dispatcher;
use digest_mcp::transport::http_bridge::{spawn_http_bridge, McpHttpServerHandle};
use digest_mcp::McpDispatcher;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, State};
use tokio::sync::Mutex;

use crate::tray;

/// CamelCase status DTO matching `McpBridgeStatus` in `apps/web/src/platform/bridge.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpBridgeStatus {
    pub running: bool,
    pub endpoint_url: String,
    pub bind_host: String,
    pub port: u16,
    pub mode: McpAccessMode,
    pub token_hint: String,
    pub last_error: Option<String>,
}

/// Route path and optional string query parameters emitted via `desktop://navigate`.
pub type DesktopNavigationEvent = (&'static str, Option<BTreeMap<String, String>>);

/// Converts a validated `DesktopNavigationTarget` into `(route, params)` for `desktop://navigate`.
pub fn navigation_target_to_event(
    target: &DesktopNavigationTarget,
) -> Result<DesktopNavigationEvent, CoreError> {
    let mut validated = target.clone();
    validated.validate()?;
    let route = validated.route_path();
    let params = validated.to_query_params();
    let params_opt = if params.is_empty() {
        None
    } else {
        Some(params)
    };
    Ok((route, params_opt))
}

/// Tauri implementation of `DesktopViewController` focusing the main window and emitting `desktop://navigate`.
pub struct TauriDesktopViewController {
    app: AppHandle,
}

impl TauriDesktopViewController {
    /// Constructs a controller bound to the Tauri `AppHandle`.
    #[must_use]
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl DesktopViewController for TauriDesktopViewController {
    async fn open_view(&self, mut target: DesktopNavigationTarget) -> Result<Value, CoreError> {
        target.validate()?;
        let (route, params_opt) = navigation_target_to_event(&target)?;
        tray::emit_desktop_navigate(&self.app, route, params_opt.clone())
            .map_err(CoreError::Internal)?;

        Ok(json!({
            "opened": true,
            "supported": true,
            "route": route,
            "params": params_opt.unwrap_or_default(),
            "target": target
        }))
    }
}

struct McpBridgeInner {
    server_base_url: String,
    api_token: Option<String>,
    auth_mode: Option<AuthMode>,
    mcp_mode: McpAccessMode,
    configured_port: u16,
    bound_port: Option<u16>,
    active_listen_port: Option<u16>,
    local_bearer_token: String,
    shared_state: McpSharedState,
    dispatcher: Arc<McpDispatcher>,
    desktop: Arc<dyn DesktopViewController>,
    server_handle: Option<McpHttpServerHandle>,
    last_error: Option<String>,
}

impl McpBridgeInner {
    fn status(&self) -> McpBridgeStatus {
        let running = self.server_handle.is_some();
        let display_port = self.bound_port.unwrap_or(self.configured_port);
        McpBridgeStatus {
            running,
            endpoint_url: format!("http://{DEFAULT_MCP_BIND_HOST}:{display_port}/mcp"),
            bind_host: DEFAULT_MCP_BIND_HOST.to_owned(),
            port: display_port,
            mode: self.mcp_mode,
            token_hint: mask_token_hint(&self.local_bearer_token),
            last_error: self.last_error.clone(),
        }
    }
}

/// Thread-safe Tauri state managing the embedded local Streamable HTTP MCP bridge.
#[derive(Clone)]
pub struct McpBridgeState {
    inner: Arc<Mutex<McpBridgeInner>>,
}

impl McpBridgeState {
    /// Creates an initially stopped bridge state with a fresh CSPRNG local Bearer token.
    pub fn new(desktop: Arc<dyn DesktopViewController>) -> Result<Self, String> {
        let local_bearer_token = generate_local_bearer_token();
        let cfg = McpRuntimeConfig {
            server_base_url: DEFAULT_SERVER_URL.to_owned(),
            api_token: None,
            access_mode: McpAccessMode::Disabled,
            http_port: DEFAULT_MCP_HTTP_PORT,
            local_bearer_token: local_bearer_token.clone(),
            ..McpRuntimeConfig::default()
        };
        let shared_state = McpSharedState::new(&cfg)
            .map_err(|err| format!("failed to initialize MCP shared state: {err}"))?;
        let client = Arc::new(
            DigestApiClient::from_shared_state(shared_state.clone())
                .map_err(|err| format!("failed to initialize MCP API client: {err}"))?,
        );
        let dispatcher = Arc::new(build_default_dispatcher(client, Arc::clone(&desktop)));

        Ok(Self {
            inner: Arc::new(Mutex::new(McpBridgeInner {
                server_base_url: DEFAULT_SERVER_URL.to_owned(),
                api_token: None,
                auth_mode: None,
                mcp_mode: McpAccessMode::Disabled,
                configured_port: DEFAULT_MCP_HTTP_PORT,
                bound_port: None,
                active_listen_port: None,
                local_bearer_token,
                shared_state,
                dispatcher,
                desktop,
                server_handle: None,
                last_error: None,
            })),
        })
    }

    /// Synchronizes server URL, auth session, MCP access mode, and HTTP listener port.
    pub async fn sync_runtime_config(
        &self,
        server_base_url: String,
        access_token: Option<String>,
        auth_mode: Option<AuthMode>,
        mcp_mode: McpAccessMode,
        mcp_port: u16,
    ) -> McpBridgeStatus {
        let mut guard = self.inner.lock().await;

        let resolved_url = if server_base_url.trim().is_empty() {
            DEFAULT_SERVER_URL.to_owned()
        } else {
            match validate_server_base_url(&server_base_url) {
                Ok(normalized) => normalized,
                Err(err) => {
                    guard.last_error = Some(sanitize_error_text(&err.to_string()));
                    return guard.status();
                }
            }
        };

        let normalized_token = access_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        if let Err(err) = guard
            .shared_state
            .update_connection(resolved_url.clone(), normalized_token.clone())
        {
            guard.last_error = Some(sanitize_error_text(&err.to_string()));
            return guard.status();
        }

        guard.server_base_url = resolved_url;
        guard.api_token = normalized_token;
        guard.auth_mode = auth_mode;
        guard.mcp_mode = mcp_mode;
        guard.configured_port = mcp_port;
        guard.shared_state.set_access_mode(mcp_mode);

        if mcp_mode == McpAccessMode::Disabled {
            if let Some(handle) = guard.server_handle.take() {
                handle.shutdown().await;
            }
            guard.bound_port = None;
            guard.active_listen_port = None;
            guard.last_error = None;
            return guard.status();
        }

        let needs_restart =
            guard.server_handle.is_none() || guard.active_listen_port != Some(mcp_port);
        if needs_restart {
            if let Some(handle) = guard.server_handle.take() {
                handle.shutdown().await;
            }
            guard.bound_port = None;
            guard.active_listen_port = None;

            match spawn_http_bridge(
                mcp_port,
                Arc::clone(&guard.dispatcher),
                guard.shared_state.clone(),
                Arc::clone(&guard.desktop),
            )
            .await
            {
                Ok(handle) => {
                    guard.bound_port = Some(handle.addr().port());
                    guard.active_listen_port = Some(mcp_port);
                    guard.server_handle = Some(handle);
                    guard.last_error = None;
                }
                Err(err) => {
                    guard.last_error = Some(sanitize_error_text(&err.to_string()));
                }
            }
        } else {
            guard.last_error = None;
        }

        guard.status()
    }

    /// Returns current non-secret bridge status.
    pub async fn status(&self) -> McpBridgeStatus {
        self.inner.lock().await.status()
    }

    /// Returns the raw local Bearer token solely for explicit clipboard copy in Desktop settings.
    pub async fn local_token(&self) -> String {
        self.inner.lock().await.local_bearer_token.clone()
    }

    /// Rotates the CSPRNG local Bearer token and restarts the HTTP bridge if currently active.
    pub async fn rotate_token(&self) -> Result<McpBridgeStatus, String> {
        let mut guard = self.inner.lock().await;
        let new_token = generate_local_bearer_token();
        let new_shared_state = McpSharedState::from_parts(
            guard.server_base_url.clone(),
            guard.api_token.clone(),
            guard.mcp_mode,
            new_token.clone(),
        )
        .map_err(|err| format!("failed to rebuild MCP shared state: {err}"))?;

        let new_client = Arc::new(
            DigestApiClient::from_shared_state(new_shared_state.clone())
                .map_err(|err| format!("failed to rebuild MCP API client: {err}"))?,
        );
        let new_dispatcher = Arc::new(build_default_dispatcher(
            new_client,
            Arc::clone(&guard.desktop),
        ));

        guard.local_bearer_token = new_token;
        guard.shared_state = new_shared_state;
        guard.dispatcher = new_dispatcher;

        let was_running_or_enabled =
            guard.server_handle.is_some() || guard.mcp_mode != McpAccessMode::Disabled;
        if let Some(handle) = guard.server_handle.take() {
            handle.shutdown().await;
        }
        guard.bound_port = None;
        guard.active_listen_port = None;

        if was_running_or_enabled {
            let port = guard.configured_port;
            match spawn_http_bridge(
                port,
                Arc::clone(&guard.dispatcher),
                guard.shared_state.clone(),
                Arc::clone(&guard.desktop),
            )
            .await
            {
                Ok(handle) => {
                    guard.bound_port = Some(handle.addr().port());
                    guard.active_listen_port = Some(port);
                    guard.server_handle = Some(handle);
                    guard.last_error = None;
                }
                Err(err) => {
                    guard.last_error = Some(sanitize_error_text(&err.to_string()));
                }
            }
        } else {
            guard.last_error = None;
        }

        Ok(guard.status())
    }

    /// Gracefully stops the embedded HTTP bridge listener if running.
    pub async fn shutdown(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(handle) = guard.server_handle.take() {
            handle.shutdown().await;
        }
        guard.bound_port = None;
        guard.active_listen_port = None;
    }
}

/// Syncs server URL, OIDC token, auth mode, and MCP guard/port from the Desktop frontend.
#[tauri::command]
pub async fn sync_desktop_runtime_config(
    state: State<'_, McpBridgeState>,
    server_base_url: String,
    access_token: Option<String>,
    auth_mode: Option<AuthMode>,
    mcp_mode: McpAccessMode,
    mcp_port: u16,
) -> Result<McpBridgeStatus, String> {
    if mcp_port == 0 {
        return Err("mcpPort must be between 1 and 65535".to_owned());
    }
    Ok(state
        .sync_runtime_config(server_base_url, access_token, auth_mode, mcp_mode, mcp_port)
        .await)
}

/// Returns current MCP bridge status without ever exposing the raw Bearer token.
#[tauri::command]
pub async fn get_mcp_bridge_status(
    state: State<'_, McpBridgeState>,
) -> Result<McpBridgeStatus, String> {
    Ok(state.status().await)
}

/// Returns the local Bearer token for user-triggered clipboard copy in Desktop settings.
#[tauri::command]
pub async fn get_mcp_local_token(state: State<'_, McpBridgeState>) -> Result<String, String> {
    Ok(state.local_token().await)
}

/// Rotates the local CSPRNG Bearer token and returns updated bridge status.
#[tauri::command]
pub async fn rotate_mcp_token(state: State<'_, McpBridgeState>) -> Result<McpBridgeStatus, String> {
    state.rotate_token().await
}

#[cfg(test)]
mod tests {
    use digest_mcp::desktop::NoopDesktopViewController;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn navigation_target_converts_cleanly_to_desktop_navigate_route_and_params() {
        let source_id = Uuid::new_v4();
        let target = DesktopNavigationTarget {
            view: "Sources".to_owned(),
            date: Some("2026-11-26".to_owned()),
            source_id: Some(source_id),
            q: Some("gemini".to_owned()),
            bookmarked: Some(true),
            tab: None,
        };

        let (route, params) = navigation_target_to_event(&target).unwrap();
        assert_eq!(route, "/sources");
        let params = params.unwrap();
        assert_eq!(params.get("date").unwrap(), "2026-11-26");
        assert_eq!(params.get("sourceId").unwrap(), &source_id.to_string());
        assert_eq!(params.get("q").unwrap(), "gemini");
        assert_eq!(params.get("bookmarked").unwrap(), "true");
        assert!(!params.contains_key("tab"));

        let settings_target = DesktopNavigationTarget {
            view: "settings".to_owned(),
            date: None,
            source_id: None,
            q: None,
            bookmarked: None,
            tab: Some("mcp".to_owned()),
        };
        let (settings_route, settings_params) =
            navigation_target_to_event(&settings_target).unwrap();
        assert_eq!(settings_route, "/settings");
        assert_eq!(settings_params.unwrap().get("tab").unwrap(), "mcp");

        let non_settings_with_tab = DesktopNavigationTarget {
            view: "Sources".to_owned(),
            date: None,
            source_id: None,
            q: None,
            bookmarked: None,
            tab: Some("mcp".to_owned()),
        };
        assert!(navigation_target_to_event(&non_settings_with_tab).is_err());

        let unknown_settings_tab = DesktopNavigationTarget {
            view: "settings".to_owned(),
            date: None,
            source_id: None,
            q: None,
            bookmarked: None,
            tab: Some("unknown_tab".to_owned()),
        };
        assert!(navigation_target_to_event(&unknown_settings_tab).is_err());

        let bare_daily = DesktopNavigationTarget {
            view: "daily".to_owned(),
            date: None,
            source_id: None,
            q: None,
            bookmarked: None,
            tab: None,
        };
        let (daily_route, daily_params) = navigation_target_to_event(&bare_daily).unwrap();
        assert_eq!(daily_route, "/daily");
        assert!(daily_params.is_none());
    }

    #[tokio::test]
    async fn bridge_status_never_exposes_raw_token_rotates_token_and_binds_loopback_until_disabled()
    {
        let state = McpBridgeState::new(Arc::new(NoopDesktopViewController)).unwrap();
        let initial_token = state.local_token().await;
        let initial_status = state.status().await;

        assert!(!initial_status.running);
        assert_eq!(initial_status.mode, McpAccessMode::Disabled);
        assert_eq!(initial_status.bind_host, "127.0.0.1");
        let status_json = serde_json::to_string(&initial_status).unwrap();
        assert!(!status_json.contains(&initial_token));
        assert!(initial_status.token_hint.starts_with("****"));

        // Start on ephemeral loopback port `0` in ReadOnly mode
        let started = state
            .sync_runtime_config(
                "http://127.0.0.1:8080".to_owned(),
                Some("oidc-access-token".to_owned()),
                Some(AuthMode::Oidc),
                McpAccessMode::ReadOnly,
                0,
            )
            .await;
        assert!(started.running);
        assert_eq!(started.bind_host, "127.0.0.1");
        assert_ne!(started.port, 0);
        assert_eq!(
            started.endpoint_url,
            format!("http://127.0.0.1:{}/mcp", started.port)
        );
        assert!(started.last_error.is_none());

        // Verify local HTTP bridge is actually listening on 127.0.0.1 and enforces Bearer token
        let http = reqwest::Client::new();
        let health_url = format!("http://127.0.0.1:{}/health", started.port);
        let health_resp = http.get(&health_url).send().await.unwrap();
        assert_eq!(health_resp.status(), 200);

        let tools_url = format!("http://127.0.0.1:{}/control/tools", started.port);
        let unauth_resp = http.get(&tools_url).send().await.unwrap();
        assert_eq!(unauth_resp.status(), 401);

        let auth_resp = http
            .get(&tools_url)
            .bearer_auth(&initial_token)
            .send()
            .await
            .unwrap();
        assert_eq!(auth_resp.status(), 200);

        // Rotate token while keeping the bridge running (update port to 0 for ephemeral rebind)
        {
            let mut guard = state.inner.lock().await;
            guard.configured_port = 0;
        }
        let rotated_status = state.rotate_token().await.unwrap();
        let rotated_token = state.local_token().await;
        assert_ne!(rotated_token, initial_token);
        assert_ne!(rotated_status.token_hint, initial_status.token_hint);
        assert!(!serde_json::to_string(&rotated_status)
            .unwrap()
            .contains(&rotated_token));
        assert!(rotated_status.running);

        let new_tools_url = format!("http://127.0.0.1:{}/control/tools", rotated_status.port);
        let old_tok_resp = http
            .get(&new_tools_url)
            .bearer_auth(&initial_token)
            .send()
            .await
            .unwrap();
        assert_eq!(old_tok_resp.status(), 401);

        let new_tok_resp = http
            .get(&new_tools_url)
            .bearer_auth(&rotated_token)
            .send()
            .await
            .unwrap();
        assert_eq!(new_tok_resp.status(), 200);

        // Switching to Disabled shuts down the HTTP listener cleanly
        let disabled = state
            .sync_runtime_config(
                "http://127.0.0.1:8080".to_owned(),
                None,
                Some(AuthMode::DevBypass),
                McpAccessMode::Disabled,
                8765,
            )
            .await;
        assert!(!disabled.running);
        assert_eq!(disabled.mode, McpAccessMode::Disabled);
        assert!(http.get(&health_url).send().await.is_err());
    }
}
