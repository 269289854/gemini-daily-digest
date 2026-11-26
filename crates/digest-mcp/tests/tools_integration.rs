#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use digest_core::domain::{ErrorCode, McpAccessMode, McpToolName};
use digest_core::error::CoreError;
use digest_mcp::client::DigestApiClient;
use digest_mcp::config::McpSharedState;
use digest_mcp::desktop::{
    DesktopNavigationTarget, DesktopViewController, NoopDesktopViewController,
};
use digest_mcp::tools::{all_tool_descriptors, build_default_dispatcher};
use serde_json::{json, Value};
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct RecordingDesktopController {
    opened: Mutex<Vec<DesktopNavigationTarget>>,
}

#[async_trait]
impl DesktopViewController for RecordingDesktopController {
    async fn open_view(&self, mut target: DesktopNavigationTarget) -> Result<Value, CoreError> {
        target.validate()?;
        self.opened.lock().unwrap().push(target.clone());
        Ok(json!({
            "opened": true,
            "supported": true,
            "route": target.route_path(),
            "params": target.to_query_params(),
            "target": target,
        }))
    }
}

fn read_only_tools() -> [McpToolName; 7] {
    [
        McpToolName::DigestGetDaily,
        McpToolName::DigestListDates,
        McpToolName::DigestListSources,
        McpToolName::DigestGetSourceDigests,
        McpToolName::DigestSearchItems,
        McpToolName::DigestGetBookmarks,
        McpToolName::DigestGetRunsStats,
    ]
}

fn read_write_tools() -> [McpToolName; 7] {
    [
        McpToolName::DigestTriggerRun,
        McpToolName::DigestAddSource,
        McpToolName::DigestToggleSource,
        McpToolName::DigestBookmarkItem,
        McpToolName::DigestUpdateSchedule,
        McpToolName::DigestAddWatchlistRule,
        McpToolName::DigestOpenDesktopView,
    ]
}

fn sample_args_for_tool(tool: McpToolName, id: Uuid) -> Value {
    match tool {
        McpToolName::DigestGetDaily => json!({ "date": "2026-11-26" }),
        McpToolName::DigestListDates => json!({ "page": 1, "pageSize": 10 }),
        McpToolName::DigestListSources => json!({}),
        McpToolName::DigestGetSourceDigests => json!({ "sourceId": id }),
        McpToolName::DigestSearchItems => json!({ "q": "gemini", "page": 1 }),
        McpToolName::DigestGetBookmarks => json!({ "page": 1 }),
        McpToolName::DigestGetRunsStats => json!({ "page": 1 }),
        McpToolName::DigestTriggerRun => json!({ "forceRefresh": false, "notify": false }),
        McpToolName::DigestAddSource => {
            json!({ "sourceType": "rss", "handleOrUrl": "https://blog.google/rss" })
        }
        McpToolName::DigestToggleSource => json!({ "sourceId": id, "enabled": true }),
        McpToolName::DigestBookmarkItem => json!({ "itemId": id, "bookmarked": true }),
        McpToolName::DigestUpdateSchedule => json!({ "dailyTime": "09:15" }),
        McpToolName::DigestAddWatchlistRule => {
            json!({ "name": "Gemini", "keywords": ["gemini"] })
        }
        McpToolName::DigestOpenDesktopView => json!({ "view": "daily", "date": "2026-11-26" }),
    }
}

async fn mount_happy_path_mocks(server: &MockServer, id: Uuid) {
    Mock::given(method("GET"))
        .and(path("/api/v1/digests"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": id, "digestDate": "2026-11-26", "title": "Daily" }],
            "page": 1, "pageSize": 20, "total": 1, "totalPages": 1
        })))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/digests/2026-11-26"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id,
            "digestDate": "2026-11-26",
            "title": "Gemini Daily",
            "overviewMarkdown": "# Overview",
            "sections": []
        })))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/settings/sources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/sources/{id}/digests")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [], "total": 0 })))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/sources/{id}/items")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [], "total": 0 })))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [], "total": 0 })))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/runs/stats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "totalRuns": 1 })))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/runs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [], "total": 0 })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/runs/trigger"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": id, "status": "running" })),
        )
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/settings/sources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id, "handleOrUrl": "https://blog.google/rss"
        })))
        .mount(server)
        .await;

    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/settings/sources/{id}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": id, "enabled": true })),
        )
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/bookmarks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": id, "itemId": id })))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/settings/schedule"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": id,
            "timezone": "Asia/Shanghai",
            "dailyTime": "08:30",
            "emailEnabled": true,
            "sendWhenEmpty": false,
            "alertScanEnabled": true,
            "alertIntervalMinutes": 60
        })))
        .mount(server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/api/v1/settings/schedule"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": id,
            "timezone": "Asia/Shanghai",
            "dailyTime": "09:15",
            "emailEnabled": true,
            "sendWhenEmpty": false,
            "alertScanEnabled": true,
            "alertIntervalMinutes": 60
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/settings/watchlist"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": id, "name": "Gemini" })),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn capability_guard_enforces_all_14_tools_across_all_modes() {
    let server = MockServer::start().await;
    let id = Uuid::new_v4();
    mount_happy_path_mocks(&server, id).await;

    let shared = McpSharedState::from_parts(
        server.uri(),
        Some("oidc-test-token".to_owned()),
        McpAccessMode::ReadOnly,
        "local-mcp-token",
    )
    .unwrap();
    let client = Arc::new(DigestApiClient::from_shared_state(shared).unwrap());
    let desktop = Arc::new(NoopDesktopViewController);
    let dispatcher = build_default_dispatcher(client, desktop);

    assert_eq!(dispatcher.handler_count(), 14);
    assert_eq!(all_tool_descriptors().len(), 14);

    for &tool in McpToolName::all() {
        let args = sample_args_for_tool(tool, id);
        let err = dispatcher
            .dispatch(McpAccessMode::Disabled, tool, args)
            .await
            .unwrap_err();
        assert_eq!(
            err.error_code(),
            ErrorCode::Forbidden,
            "tool {tool} must be forbidden in Disabled mode"
        );
    }

    for tool in read_only_tools() {
        let args = sample_args_for_tool(tool, id);
        let ok = dispatcher
            .dispatch(McpAccessMode::ReadOnly, tool, args)
            .await;
        assert!(
            ok.is_ok(),
            "read tool {tool} should succeed in ReadOnly: {ok:?}"
        );
    }

    for tool in read_write_tools() {
        let args = sample_args_for_tool(tool, id);
        let err = dispatcher
            .dispatch(McpAccessMode::ReadOnly, tool, args)
            .await
            .unwrap_err();
        assert_eq!(
            err.error_code(),
            ErrorCode::Forbidden,
            "write tool {tool} must be forbidden in ReadOnly mode"
        );
    }

    for &tool in McpToolName::all() {
        let args = sample_args_for_tool(tool, id);
        let ok = dispatcher
            .dispatch(McpAccessMode::ReadWrite, tool, args)
            .await;
        assert!(
            ok.is_ok(),
            "tool {tool} should succeed in ReadWrite: {ok:?}"
        );
    }
}

#[tokio::test]
async fn wiremock_integration_verifies_all_14_tools_and_edge_cases() {
    let server = MockServer::start().await;
    let source_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();

    // 1. digest_get_daily omitted date fallback + markdown
    Mock::given(method("GET"))
        .and(path("/api/v1/digests"))
        .and(query_param("page", "1"))
        .and(query_param("pageSize", "1"))
        .and(header("authorization", "Bearer api-secret-tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": Uuid::new_v4(), "digestDate": "2026-11-25" }],
            "page": 1, "pageSize": 1, "total": 1, "totalPages": 1
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/digests/2026-11-25"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "digestDate": "2026-11-25",
            "title": "Latest Digest"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/digests/2026-11-25/markdown"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# Latest Digest"))
        .mount(&server)
        .await;

    // 2. digest_list_dates
    Mock::given(method("GET"))
        .and(path("/api/v1/digests"))
        .and(query_param("page", "2"))
        .and(query_param("pageSize", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [], "page": 2, "pageSize": 5, "total": 0, "totalPages": 0
        })))
        .mount(&server)
        .await;

    // 3. digest_list_sources (with sensitive hints/urls/errors to verify redaction)
    let fake_sk = ["sk-", "proj", "0123456789abcdef"].concat();
    Mock::given(method("GET"))
        .and(path("/api/v1/settings/sources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "id": source_id,
                "handleOrUrl": "https://alice:pw@feeds.example.com/feed.xml?token=secret123&tag=ai",
                "hasCredentials": true,
                "configHint": "alic****cret",
                "lastError": format!("Bearer jwt_secret_123 {fake_sk}")
            }
        ])))
        .mount(&server)
        .await;

    // 4. digest_get_source_digests
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/sources/{source_id}/digests")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "digestDate": "2026-11-25", "summaryMarkdown": "Summary" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/sources/{source_id}/items")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": item_id, "title": "Release 2.5" }]
        })))
        .mount(&server)
        .await;

    // 5 & 6. digest_search_items & digest_get_bookmarks
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .and(query_param("q", "gemini"))
        .and(query_param("dateFrom", "2026-11-01"))
        .and(query_param("dateTo", "2026-11-26"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": item_id, "title": "Gemini Flash" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items"))
        .and(query_param("bookmarked", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": item_id, "bookmarked": true, "bookmarkNote": "important" }]
        })))
        .mount(&server)
        .await;

    // 7. digest_get_runs_stats
    Mock::given(method("GET"))
        .and(path("/api/v1/runs/stats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "totalRuns": 3,
            "sourceHealth": [{
                "sourceId": source_id,
                "lastError": "https://u:p@host.example.com/api?api_key=leak99"
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/runs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": Uuid::new_v4(),
                "errorSummary": "Bearer tok_xyz",
                "deliveries": [{ "errorMessage": format!("failed {fake_sk}") }]
            }]
        })))
        .mount(&server)
        .await;

    // 8. digest_trigger_run
    Mock::given(method("POST"))
        .and(path("/api/v1/runs/trigger"))
        .and(body_json(json!({
            "digestDate": "2026-11-26",
            "forceRefresh": true,
            "notify": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "running" })))
        .mount(&server)
        .await;

    // 9. digest_add_source
    Mock::given(method("POST"))
        .and(path("/api/v1/settings/sources"))
        .and(body_json(json!({
            "sourceType": "rss",
            "handleOrUrl": "https://ai.google.dev/feed.xml",
            "enabled": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": source_id,
            "handleOrUrl": "https://ai.google.dev/feed.xml",
            "hasCredentials": false,
            "configHint": null
        })))
        .mount(&server)
        .await;

    // 10. digest_toggle_source
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/settings/sources/{source_id}")))
        .and(body_json(json!({ "enabled": false })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": source_id, "enabled": false
        })))
        .mount(&server)
        .await;

    // 11. digest_bookmark_item (POST + DELETE)
    Mock::given(method("POST"))
        .and(path("/api/v1/bookmarks"))
        .and(body_json(
            json!({ "itemId": item_id, "note": "read later" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": Uuid::new_v4(), "itemId": item_id, "note": "read later"
        })))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/bookmarks/{item_id}")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    // 12. digest_update_schedule (GET -> merge -> PUT all 6 required fields)
    Mock::given(method("GET"))
        .and(path("/api/v1/settings/schedule"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": Uuid::new_v4(),
            "timezone": "Asia/Shanghai",
            "dailyTime": "08:30",
            "emailEnabled": true,
            "sendWhenEmpty": false,
            "alertScanEnabled": true,
            "alertIntervalMinutes": 60
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/settings/schedule"))
        .and(body_json(json!({
            "timezone": "Asia/Shanghai",
            "dailyTime": "07:45",
            "emailEnabled": false,
            "sendWhenEmpty": false,
            "alertScanEnabled": true,
            "alertIntervalMinutes": 60
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "dailyTime": "07:45",
            "emailEnabled": false
        })))
        .mount(&server)
        .await;

    // 13. digest_add_watchlist_rule (must ALWAYS send "excludeKeywords": [])
    Mock::given(method("POST"))
        .and(path("/api/v1/settings/watchlist"))
        .and(body_json(json!({
            "name": "Gemini 3",
            "keywords": ["gemini", "flash"],
            "excludeKeywords": [],
            "priority": "normal",
            "notifyImmediately": false,
            "enabled": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": Uuid::new_v4(), "name": "Gemini 3"
        })))
        .mount(&server)
        .await;

    let client =
        Arc::new(DigestApiClient::new(server.uri(), Some("api-secret-tok".to_owned())).unwrap());
    let desktop = Arc::new(RecordingDesktopController::default());
    let dispatcher = build_default_dispatcher(client, desktop.clone());

    // Verify 1..7 read tools
    let daily_latest = dispatcher
        .dispatch(
            McpAccessMode::ReadOnly,
            McpToolName::DigestGetDaily,
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(daily_latest["digestDate"], "2026-11-25");

    let daily_md = dispatcher
        .dispatch(
            McpAccessMode::ReadOnly,
            McpToolName::DigestGetDaily,
            json!({ "date": "2026-11-25", "format": "markdown" }),
        )
        .await
        .unwrap();
    assert_eq!(daily_md["markdown"], "# Latest Digest");

    let dates = dispatcher
        .dispatch(
            McpAccessMode::ReadOnly,
            McpToolName::DigestListDates,
            json!({ "page": 2, "pageSize": 5 }),
        )
        .await
        .unwrap();
    assert_eq!(dates["page"], 2);

    let sources = dispatcher
        .dispatch(
            McpAccessMode::ReadOnly,
            McpToolName::DigestListSources,
            json!({}),
        )
        .await
        .unwrap();
    let src_str = sources.to_string();
    assert!(!src_str.contains("configHint"));
    assert!(!src_str.contains("hasCredentials"));
    assert!(!src_str.contains("alice:pw"));
    assert!(!src_str.contains("secret123"));
    assert!(!src_str.contains("jwt_secret_123"));
    assert!(!src_str.contains(&fake_sk));

    let src_digests = dispatcher
        .dispatch(
            McpAccessMode::ReadOnly,
            McpToolName::DigestGetSourceDigests,
            json!({ "sourceId": source_id }),
        )
        .await
        .unwrap();
    assert_eq!(src_digests["items"]["items"][0]["title"], "Release 2.5");

    let search = dispatcher
        .dispatch(
            McpAccessMode::ReadOnly,
            McpToolName::DigestSearchItems,
            json!({ "q": "gemini", "dateFrom": "2026-11-01", "dateTo": "2026-11-26" }),
        )
        .await
        .unwrap();
    assert_eq!(search["items"][0]["title"], "Gemini Flash");

    let bookmarks = dispatcher
        .dispatch(
            McpAccessMode::ReadOnly,
            McpToolName::DigestGetBookmarks,
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(bookmarks["items"][0]["bookmarked"], true);

    let stats = dispatcher
        .dispatch(
            McpAccessMode::ReadOnly,
            McpToolName::DigestGetRunsStats,
            json!({}),
        )
        .await
        .unwrap();
    let stats_str = stats.to_string();
    assert!(!stats_str.contains("leak99"));
    assert!(!stats_str.contains("tok_xyz"));
    assert!(!stats_str.contains(&fake_sk));

    // Verify 8..14 write tools
    let trig = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestTriggerRun,
            json!({ "digestDate": "2026-11-26", "forceRefresh": true, "notify": true }),
        )
        .await
        .unwrap();
    assert_eq!(trig["status"], "running");

    // Reject credentials on digest_add_source
    let cred_err = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestAddSource,
            json!({
                "sourceType": "x_account",
                "handleOrUrl": "@GoogleAI",
                "credentials": { "bearerToken": "secret" }
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(cred_err.error_code(), ErrorCode::InvalidInput);

    let added = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestAddSource,
            json!({
                "sourceType": "rss",
                "handleOrUrl": "https://ai.google.dev/feed.xml"
            }),
        )
        .await
        .unwrap();
    assert!(added.get("configHint").is_none());
    assert!(added.get("hasCredentials").is_none());

    let toggled = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestToggleSource,
            json!({ "sourceId": source_id, "enabled": false }),
        )
        .await
        .unwrap();
    assert_eq!(toggled["enabled"], false);

    let bm_add = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestBookmarkItem,
            json!({ "itemId": item_id, "note": "read later" }),
        )
        .await
        .unwrap();
    assert_eq!(bm_add["note"], "read later");

    let bm_del = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestBookmarkItem,
            json!({ "itemId": item_id, "bookmarked": false }),
        )
        .await
        .unwrap();
    assert_eq!(bm_del["bookmarked"], false);

    let sched = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestUpdateSchedule,
            json!({ "dailyTime": "07:45", "emailEnabled": false }),
        )
        .await
        .unwrap();
    assert_eq!(sched["dailyTime"], "07:45");

    let watch = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestAddWatchlistRule,
            json!({ "name": "Gemini 3", "keywords": ["gemini", "flash"] }),
        )
        .await
        .unwrap();
    assert_eq!(watch["name"], "Gemini 3");

    // 14. digest_open_desktop_view validation & execution
    let bad_nav = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestOpenDesktopView,
            json!({ "view": "settings", "tab": "../secrets?x=1" }),
        )
        .await
        .unwrap_err();
    assert_eq!(bad_nav.error_code(), ErrorCode::InvalidInput);

    let nav_ok = dispatcher
        .dispatch(
            McpAccessMode::ReadWrite,
            McpToolName::DigestOpenDesktopView,
            json!({ "view": "history", "q": "gemini", "bookmarked": true }),
        )
        .await
        .unwrap();
    assert_eq!(nav_ok["opened"], true);
    assert_eq!(nav_ok["route"], "/history");
    assert_eq!(desktop.opened.lock().unwrap().len(), 1);
}
