#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::body::Body;
use digest_core::constants::{
    CONTENT_TYPE_JSON, HEADER_AUTHORIZATION, HEADER_CONTENT_TYPE, HEADER_MCP_TOKEN,
    HEALTH_STATUS_OK, ROUTE_HEALTH,
};
use digest_core::domain::{ErrorCode, McpAccessMode};
use digest_mcp::client::DigestApiClient;
use digest_mcp::config::McpSharedState;
use digest_mcp::desktop::NoopDesktopViewController;
use digest_mcp::tools::build_default_dispatcher;
use digest_mcp::transport::http_bridge::{build_http_router, spawn_http_bridge, McpHttpState};
use digest_mcp::transport::stdio::serve_stdio_with_io;
use http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, ORIGIN};
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn http_bridge_enforces_constant_time_auth_cors_and_access_modes() {
    let backend = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/settings/sources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "id": "00000000-0000-0000-0000-000000000001",
                "handleOrUrl": "https://u:p@example.com/rss?token=abc",
                "hasCredentials": true,
                "configHint": "hint****1234"
            }
        ])))
        .mount(&backend)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/runs/trigger"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "running" })))
        .mount(&backend)
        .await;

    let local_token = "csprng-local-bridge-secret-token";
    let shared = McpSharedState::from_parts(
        backend.uri(),
        Some("backend-token".to_owned()),
        McpAccessMode::ReadOnly,
        local_token,
    )
    .unwrap();
    let client = Arc::new(DigestApiClient::from_shared_state(shared.clone()).unwrap());
    let desktop = Arc::new(NoopDesktopViewController);
    let dispatcher = Arc::new(build_default_dispatcher(client, desktop.clone()));

    let app = build_http_router(McpHttpState {
        dispatcher,
        shared_state: shared.clone(),
        desktop,
    });

    // 1. /health returns 200 ok
    let health_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(ROUTE_HEALTH)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health_resp.status(), StatusCode::OK);
    let health_json: Value =
        serde_json::from_slice(&health_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(health_json["status"], HEALTH_STATUS_OK);

    // 2. Missing or wrong token -> 401 UNAUTHORIZED
    let unauth_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/control/tools")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauth_resp.status(), StatusCode::UNAUTHORIZED);
    let unauth_json: Value =
        serde_json::from_slice(&unauth_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(unauth_json["code"], ErrorCode::Unauthorized.as_str());

    let wrong_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(HEADER_AUTHORIZATION, "Bearer wrong-token")
                .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_resp.status(), StatusCode::UNAUTHORIZED);

    // 3. Disallowed Origin must NOT get Access-Control-Allow-Origin: *
    let evil_origin_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/control/tools")
                .header(ORIGIN, "https://evil.example.com")
                .header(HEADER_MCP_TOKEN, local_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        evil_origin_resp
            .headers()
            .get(ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
    assert!(evil_origin_resp
        .headers()
        .get(ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());

    // 4. x-digest-mcp-token accepted on GET /control/tools
    let tools_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/control/tools")
                .header(HEADER_MCP_TOKEN, local_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tools_resp.status(), StatusCode::OK);
    let tools_json: Value =
        serde_json::from_slice(&tools_resp.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(tools_json["tools"].as_array().unwrap().len(), 14);

    // 5. POST /mcp Streamable HTTP JSON-RPC (Bearer auth)
    let mcp_call = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "digest_list_sources",
            "arguments": {}
        }
    });
    let mcp_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(HEADER_AUTHORIZATION, format!("Bearer {local_token}"))
                .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                .body(Body::from(mcp_call.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mcp_resp.status(), StatusCode::OK);
    let mcp_json: Value =
        serde_json::from_slice(&mcp_resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(mcp_json["result"]["isError"], false);
    let text = mcp_json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("configHint"));
    assert!(!text.contains("hasCredentials"));
    assert!(!text.contains("u:p@"));
    assert!(!text.contains("token=abc"));

    // 6. GET /mcp -> 405, DELETE /mcp -> 204
    let get_mcp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/mcp")
                .header(HEADER_MCP_TOKEN, local_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_mcp.status(), StatusCode::METHOD_NOT_ALLOWED);

    let del_mcp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/mcp")
                .header(HEADER_MCP_TOKEN, local_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del_mcp.status(), StatusCode::NO_CONTENT);

    // 7. POST /control/tools/{name} enforces ReadOnly vs ReadWrite vs Disabled (403)
    let ro_write = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/tools/digest_trigger_run")
                .header(HEADER_AUTHORIZATION, format!("Bearer {local_token}"))
                .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ro_write.status(), StatusCode::FORBIDDEN);

    shared.set_access_mode(McpAccessMode::Disabled);
    let disabled_read = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/tools/digest_list_sources")
                .header(HEADER_AUTHORIZATION, format!("Bearer {local_token}"))
                .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disabled_read.status(), StatusCode::FORBIDDEN);

    shared.set_access_mode(McpAccessMode::ReadWrite);
    let rw_write = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/tools/digest_trigger_run")
                .header(HEADER_AUTHORIZATION, format!("Bearer {local_token}"))
                .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rw_write.status(), StatusCode::OK);

    // 8. POST /control/desktop/open succeeds in ReadWrite and blocks unsafe target
    let desktop_ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/control/desktop/open")
                .header(HEADER_MCP_TOKEN, local_token)
                .header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON)
                .body(Body::from(r#"{"view":"daily","date":"2026-11-26"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(desktop_ok.status(), StatusCode::OK);
}

#[tokio::test]
async fn spawn_http_bridge_binds_loopback_and_shuts_down_cleanly() {
    let shared = McpSharedState::from_parts(
        "http://127.0.0.1:8080",
        None,
        McpAccessMode::ReadOnly,
        "bridge-tok",
    )
    .unwrap();
    let client = Arc::new(DigestApiClient::from_shared_state(shared.clone()).unwrap());
    let desktop = Arc::new(NoopDesktopViewController);
    let dispatcher = Arc::new(build_default_dispatcher(client, desktop.clone()));

    let handle = spawn_http_bridge(0, dispatcher, shared, desktop)
        .await
        .unwrap();
    assert!(handle.addr().ip().is_loopback());

    let url = format!("http://{}{ROUTE_HEALTH}", handle.addr());
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    handle.shutdown().await;
}

#[tokio::test]
async fn stdio_json_rpc_framing_handles_lifecycle_and_tool_calls_over_duplex() {
    let backend = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/digests"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [], "page": 1, "pageSize": 20, "total": 0, "totalPages": 0
        })))
        .mount(&backend)
        .await;

    let shared = McpSharedState::from_parts(
        backend.uri(),
        Some("api-tok".to_owned()),
        McpAccessMode::ReadOnly,
        "unused-local-tok",
    )
    .unwrap();
    let client = Arc::new(DigestApiClient::from_shared_state(shared.clone()).unwrap());
    let desktop = Arc::new(NoopDesktopViewController);
    let dispatcher = Arc::new(build_default_dispatcher(client, desktop));

    let (client_in, server_in) = tokio::io::duplex(8192);
    let (server_out, client_out) = tokio::io::duplex(8192);

    let server_task = tokio::spawn(async move {
        serve_stdio_with_io(BufReader::new(server_in), server_out, dispatcher, shared).await
    });

    let mut writer = client_in;
    let mut reader = BufReader::new(client_out).lines();

    // initialize
    writer
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\"}}\n",
        )
        .await
        .unwrap();
    writer.flush().await.unwrap();
    let init_line = reader.next_line().await.unwrap().unwrap();
    let init_json: Value = serde_json::from_str(&init_line).unwrap();
    assert_eq!(init_json["id"], 1);
    assert_eq!(init_json["result"]["protocolVersion"], "2025-03-26");

    // notification (MUST NOT emit a response line)
    writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();

    // tools/list
    writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n")
        .await
        .unwrap();
    writer.flush().await.unwrap();
    let list_line = reader.next_line().await.unwrap().unwrap();
    let list_json: Value = serde_json::from_str(&list_line).unwrap();
    assert_eq!(list_json["id"], 2);
    assert_eq!(list_json["result"]["tools"].as_array().unwrap().len(), 14);

    // tools/call read tool
    writer
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"digest_list_dates\",\"arguments\":{}}}\n",
        )
        .await
        .unwrap();
    writer.flush().await.unwrap();
    let call_line = reader.next_line().await.unwrap().unwrap();
    let call_json: Value = serde_json::from_str(&call_line).unwrap();
    assert_eq!(call_json["id"], 3);
    assert_eq!(call_json["result"]["isError"], false);

    // tools/call write tool in ReadOnly -> isError: true with FORBIDDEN
    writer
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"digest_trigger_run\",\"arguments\":{}}}\n",
        )
        .await
        .unwrap();
    writer.flush().await.unwrap();
    let denied_line = reader.next_line().await.unwrap().unwrap();
    let denied_json: Value = serde_json::from_str(&denied_line).unwrap();
    assert_eq!(denied_json["id"], 4);
    assert_eq!(denied_json["result"]["isError"], true);
    assert!(denied_json["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("FORBIDDEN"));

    // malformed JSON -> -32700 parse error
    writer.write_all(b"{bad json\n").await.unwrap();
    writer.flush().await.unwrap();
    let parse_err_line = reader.next_line().await.unwrap().unwrap();
    let parse_err_json: Value = serde_json::from_str(&parse_err_line).unwrap();
    assert_eq!(parse_err_json["error"]["code"], -32700);

    drop(writer);
    server_task.await.unwrap().unwrap();
}
