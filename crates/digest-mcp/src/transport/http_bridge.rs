//! Local loopback (`127.0.0.1`) Streamable HTTP MCP + REST Control Bridge.
//!
//! Protected by constant-time CSPRNG Bearer token middleware (`subtle::ConstantTimeEq`),
//! accepting either `Authorization: Bearer <token>` or `x-digest-mcp-token: <token>`.

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use digest_core::constants::{
    DEFAULT_MCP_BIND_HOST, HEADER_ACCEPT, HEADER_AUTHORIZATION, HEADER_BEARER_PREFIX,
    HEADER_CONTENT_TYPE, HEADER_MCP_TOKEN, HEADER_REQUEST_ID, HEALTH_STATUS_OK, ROUTE_HEALTH,
    SERVICE_NAME,
};
use digest_core::domain::{ErrorCode, McpAccessMode, McpToolName};
use digest_core::error::CoreError;
use http::header::HeaderName;
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower_http::cors::{AllowOrigin, CorsLayer};
use uuid::Uuid;

use crate::config::McpSharedState;
use crate::desktop::{DesktopNavigationTarget, DesktopViewController};
use crate::redact::sanitize_error_text;
use crate::tools::all_control_tool_descriptors;
use crate::transport::protocol::{
    handle_json_rpc_value, JsonRpcResponse, JSONRPC_INVALID_REQUEST, JSONRPC_PARSE_ERROR,
};
use crate::McpDispatcher;

const ROUTE_MCP: &str = "/mcp";
const ROUTE_CONTROL_TOOLS: &str = "/control/tools";
const ROUTE_CONTROL_TOOL_BY_NAME: &str = "/control/tools/{toolName}";
const ROUTE_CONTROL_DESKTOP_OPEN: &str = "/control/desktop/open";

/// Shared Axum router state for the local MCP & control bridge.
#[derive(Clone)]
pub struct McpHttpState {
    pub dispatcher: Arc<McpDispatcher>,
    pub shared_state: McpSharedState,
    pub desktop: Arc<dyn DesktopViewController>,
}

/// Lifecycle handle for a spawned local loopback HTTP MCP server.
pub struct McpHttpServerHandle {
    addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl McpHttpServerHandle {
    /// Returns the bound loopback `SocketAddr` (`127.0.0.1:<port>`).
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Triggers graceful shutdown and awaits the server task.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEnvelope {
    code: ErrorCode,
    message: String,
    request_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlToolsResponse {
    mode: McpAccessMode,
    tools: Vec<crate::tools::ControlToolDescriptor>,
}

/// Builds the Axum router with strict loopback CORS and constant-time Bearer auth.
pub fn build_http_router(state: McpHttpState) -> Router {
    let protected = Router::new()
        .route(
            ROUTE_MCP,
            post(mcp_post_handler)
                .get(mcp_method_not_allowed)
                .delete(mcp_delete_handler),
        )
        .route(ROUTE_CONTROL_TOOLS, get(list_control_tools_handler))
        .route(
            ROUTE_CONTROL_TOOL_BY_NAME,
            post(invoke_control_tool_handler),
        )
        .route(ROUTE_CONTROL_DESKTOP_OPEN, post(open_desktop_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_local_bearer_auth,
        ));

    let cors = build_strict_loopback_cors();

    Router::new()
        .route(ROUTE_HEALTH, get(health_handler))
        .merge(protected)
        .layer(cors)
        .with_state(state)
}

/// Binds strictly to `127.0.0.1:<port>` (`DEFAULT_MCP_BIND_HOST`) and spawns the HTTP bridge.
pub async fn spawn_http_bridge(
    port: u16,
    dispatcher: Arc<McpDispatcher>,
    shared_state: McpSharedState,
    desktop: Arc<dyn DesktopViewController>,
) -> Result<McpHttpServerHandle, CoreError> {
    let bind_ip: IpAddr = DEFAULT_MCP_BIND_HOST
        .parse()
        .map_err(|err| CoreError::InvalidConfig(format!("invalid DEFAULT_MCP_BIND_HOST: {err}")))?;
    let bind_addr = SocketAddr::new(bind_ip, port);
    let listener = TcpListener::bind(bind_addr).await.map_err(|err| {
        CoreError::Internal(format!(
            "failed binding MCP HTTP bridge on {bind_addr}: {err}"
        ))
    })?;
    let actual_addr = listener.local_addr().map_err(|err| {
        CoreError::Internal(format!("failed reading MCP HTTP bridge local addr: {err}"))
    })?;

    let state = McpHttpState {
        dispatcher,
        shared_state,
        desktop,
    };
    let app = build_http_router(state);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let task = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        });
        if let Err(err) = server.await {
            tracing::error!("MCP HTTP bridge server error: {err}");
        }
    });

    Ok(McpHttpServerHandle {
        addr: actual_addr,
        shutdown_tx: Some(shutdown_tx),
        task: Some(task),
    })
}

fn build_strict_loopback_cors() -> CorsLayer {
    let allow_origin = AllowOrigin::predicate(|origin, _| {
        let Ok(raw) = origin.to_str() else {
            return false;
        };
        let Ok(parsed) = url::Url::parse(raw) else {
            return false;
        };
        let scheme_ok = matches!(parsed.scheme(), "http" | "https" | "tauri");
        let host_ok = matches!(
            parsed.host_str(),
            Some("127.0.0.1" | "localhost" | "tauri.localhost")
        );
        scheme_ok && host_ok && parsed.username().is_empty() && parsed.password().is_none()
    });

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            HeaderName::from_static(HEADER_AUTHORIZATION),
            HeaderName::from_static(HEADER_MCP_TOKEN),
            HeaderName::from_static(HEADER_CONTENT_TYPE),
            HeaderName::from_static(HEADER_ACCEPT),
            HeaderName::from_static(HEADER_REQUEST_ID),
        ])
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HEALTH_STATUS_OK,
        service: SERVICE_NAME,
    })
}

async fn require_local_bearer_auth(
    State(state): State<McpHttpState>,
    req: Request,
    next: Next,
) -> Response {
    let request_id = extract_request_id(req.headers());
    let Some(candidate) = extract_local_token(req.headers()) else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "Missing or malformed local MCP Bearer token",
            &request_id,
        );
    };

    if !state.shared_state.verify_local_bearer_token(&candidate) {
        return error_response(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "Invalid local MCP Bearer token",
            &request_id,
        );
    }

    next.run(req).await
}

fn extract_local_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth_val) = headers
        .get(HEADER_AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let trimmed = auth_val.trim();
        if trimmed.len() >= HEADER_BEARER_PREFIX.len()
            && trimmed[..HEADER_BEARER_PREFIX.len()].eq_ignore_ascii_case(HEADER_BEARER_PREFIX)
        {
            let token = trimmed[HEADER_BEARER_PREFIX.len()..].trim();
            if !token.is_empty() {
                return Some(token.to_owned());
            }
        }
        return None;
    }

    headers
        .get(HEADER_MCP_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

async fn mcp_method_not_allowed() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn mcp_delete_handler() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn mcp_post_handler(
    State(state): State<McpHttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = extract_request_id(&headers);
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(val) => val,
        Err(err) => {
            let rpc_err = JsonRpcResponse::err(
                Value::Null,
                JSONRPC_PARSE_ERROR,
                format!("Parse error: {err}"),
                None,
            );
            return with_request_id(
                (StatusCode::BAD_REQUEST, Json(rpc_err)).into_response(),
                &request_id,
            );
        }
    };

    let granted_mode = state.shared_state.access_mode();
    if let Value::Array(batch) = parsed {
        if batch.is_empty() {
            let rpc_err = JsonRpcResponse::err(
                Value::Null,
                JSONRPC_INVALID_REQUEST,
                "Invalid Request: empty JSON-RPC batch",
                None,
            );
            return with_request_id(
                (StatusCode::BAD_REQUEST, Json(rpc_err)).into_response(),
                &request_id,
            );
        }
        let mut responses = Vec::new();
        for item in batch {
            if let Some(resp) = handle_json_rpc_value(item, &state.dispatcher, granted_mode).await {
                responses.push(resp);
            }
        }
        if responses.is_empty() {
            return with_request_id(StatusCode::ACCEPTED.into_response(), &request_id);
        }
        return with_request_id(
            (StatusCode::OK, Json(responses)).into_response(),
            &request_id,
        );
    }

    match handle_json_rpc_value(parsed, &state.dispatcher, granted_mode).await {
        Some(resp) => with_request_id((StatusCode::OK, Json(resp)).into_response(), &request_id),
        None => with_request_id(StatusCode::ACCEPTED.into_response(), &request_id),
    }
}

async fn list_control_tools_handler(
    State(state): State<McpHttpState>,
    headers: HeaderMap,
) -> Response {
    let request_id = extract_request_id(&headers);
    let payload = ControlToolsResponse {
        mode: state.shared_state.access_mode(),
        tools: all_control_tool_descriptors(),
    };
    with_request_id((StatusCode::OK, Json(payload)).into_response(), &request_id)
}

async fn invoke_control_tool_handler(
    State(state): State<McpHttpState>,
    headers: HeaderMap,
    Path(tool_name): Path<String>,
    body: Bytes,
) -> Response {
    let request_id = extract_request_id(&headers);
    let tool = match McpToolName::from_str(&tool_name) {
        Ok(parsed) => parsed,
        Err(err) => {
            return core_error_response(&err, &request_id);
        }
    };

    let args: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(val) => val,
            Err(err) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::InvalidInput,
                    format!("invalid JSON body: {err}"),
                    &request_id,
                );
            }
        }
    };

    let granted_mode = state.shared_state.access_mode();
    match state.dispatcher.dispatch(granted_mode, tool, args).await {
        Ok(result) => with_request_id((StatusCode::OK, Json(result)).into_response(), &request_id),
        Err(err) => core_error_response(&err, &request_id),
    }
}

async fn open_desktop_handler(
    State(state): State<McpHttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = extract_request_id(&headers);
    let args: Value = match serde_json::from_slice(&body) {
        Ok(val) => val,
        Err(err) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidInput,
                format!("invalid JSON body: {err}"),
                &request_id,
            );
        }
    };

    // Validate early for clean error messages, then route through McpDispatcher so ReadWrite guard applies.
    if let Err(err) = DesktopNavigationTarget::from_json(args.clone()) {
        return core_error_response(&err, &request_id);
    }

    let granted_mode = state.shared_state.access_mode();
    match state
        .dispatcher
        .dispatch(granted_mode, McpToolName::DigestOpenDesktopView, args)
        .await
    {
        Ok(result) => with_request_id((StatusCode::OK, Json(result)).into_response(), &request_id),
        Err(err) => core_error_response(&err, &request_id),
    }
}

fn extract_request_id(headers: &HeaderMap) -> String {
    headers
        .get(HEADER_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| Uuid::new_v4().to_string(), str::to_owned)
}

fn core_error_response(err: &CoreError, request_id: &str) -> Response {
    let code = err.error_code();
    let status = match code {
        ErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict => StatusCode::CONFLICT,
        ErrorCode::SsrfBlocked => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorCode::UpstreamError => StatusCode::BAD_GATEWAY,
        ErrorCode::CryptoError | ErrorCode::DatabaseError | ErrorCode::InternalError => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    let safe_msg = sanitize_error_text(&err.to_string());
    error_response(status, code, safe_msg, request_id)
}

fn error_response(
    status: StatusCode,
    code: ErrorCode,
    message: impl Into<String>,
    request_id: &str,
) -> Response {
    let envelope = ErrorEnvelope {
        code,
        message: message.into(),
        request_id: request_id.to_owned(),
    };
    with_request_id((status, Json(envelope)).into_response(), request_id)
}

fn with_request_id(mut response: Response, request_id: &str) -> Response {
    if let Ok(val) = HeaderValue::from_str(request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(HEADER_REQUEST_ID), val);
    }
    response
}
