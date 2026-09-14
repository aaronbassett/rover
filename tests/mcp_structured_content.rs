//! Issue #70: every tool sends its result once. By default the result is in
//! `structuredContent` and `content` is only a hint; `compatibility_mode:
//! "on"` adds the full JSON text to `content`, and `structuredContent` is
//! present in both modes.
//!
//! The stdio tests drive the real `rover mcp` binary; the HTTP tests drive
//! the Streamable HTTP transport, in-process and over a real socket. Search
//! goes to a `wiremock` provider — nothing here calls the real Brave API.

#![cfg(feature = "test-loopback")]

mod common;

use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, Tool};
use rmcp::service::RunningService;
use rover::mcp::response::STRUCTURED_CONTENT_HINT;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{seed_default_tokenizer, spawn_client_with_env};

type Client = RunningService<rmcp::RoleClient, ()>;

const SEARCH_KEY_ENV: &str = "ROVER_TEST_STRUCTURED_CONTENT_SEARCH_KEY";
const SEARCH_ENDPOINT: &str = "/res/v1/web/search";

/// Appears once in the origin page.
const SENTENCE: &str = "The quick amber lighthouse keeper counted seventeen herons at dawn.";

const ALL_TOOLS: [&str; 6] = [
    "fetch_tool",
    "batch_fetch_tool",
    "summarize_tool",
    "get_metadata_tool",
    "count_tokens_tool",
    "search_tool",
];

/// The origin page, the search provider, and a stdio client wired to both.
struct Fixture {
    origin: MockServer,
    provider: MockServer,
    client: Client,
    data_dir: tempfile::TempDir,
}

fn page_html() -> String {
    format!(
        "<html><head><title>Once</title></head><body><article>\
         <h1>Lighthouse</h1>\
         <p>{SENTENCE} The rest of this paragraph gives the extractor enough prose \
         to treat the article as real content rather than boilerplate.</p>\
         <p>A second paragraph, with different words, pads the article out a little \
         further so readability keeps it.</p>\
         </article></body></html>"
    )
}

async fn origin() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(page_html()),
        )
        .mount(&server)
        .await;
    server
}

async fn fixture() -> Fixture {
    let origin = origin().await;
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SEARCH_ENDPOINT))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "query": {"original": "lighthouse"},
            "web": {"results": [
                {"title": "Lighthouse", "url": "https://example.com/lighthouse",
                 "description": "Keepers and herons."}
            ]}
        })))
        .mount(&provider)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    seed_default_tokenizer(tmp.path());
    std::fs::write(
        tmp.path().join("rover.toml"),
        format!(
            "[robots]\nrespect = false\n\n[ssrf]\nlevel = \"loopback\"\n\n\
             [search]\napi_key_env = \"{SEARCH_KEY_ENV}\"\nbase_url = \"{}{SEARCH_ENDPOINT}\"\n\
             max_retries = 0\n",
            provider.uri()
        ),
    )
    .unwrap();
    let client =
        spawn_client_with_env(tmp.path(), &[(SEARCH_KEY_ENV, "test-key-do-not-use")]).await;
    Fixture {
        origin,
        provider,
        client,
        data_dir: tmp,
    }
}

/// Valid arguments for each tool, so every tool can be called for real.
fn valid_args(fx: &Fixture, tool: &str) -> Value {
    let url = format!("{}/p", fx.origin.uri());
    match tool {
        "fetch_tool" => json!({ "url": url }),
        "batch_fetch_tool" => json!({ "urls": [url] }),
        "summarize_tool" => json!({ "url": url, "mode": "extractive" }),
        "get_metadata_tool" => json!({ "url": url }),
        "count_tokens_tool" => json!({ "text": "hello world" }),
        "search_tool" => json!({ "query": "lighthouse" }),
        other => panic!("no valid arguments for {other}; add the new tool to this test"),
    }
}

/// The tools that can succeed in this build: `search` needs `web-search`.
fn succeeding_tools() -> Vec<&'static str> {
    ALL_TOOLS
        .into_iter()
        .filter(|t| cfg!(feature = "web-search") || *t != "search_tool")
        .collect()
}

fn with_mode(mut args: Value, mode: Option<Value>) -> Value {
    if let Some(mode) = mode {
        args["compatibility_mode"] = mode;
    }
    args
}

async fn call(client: &Client, tool: &str, args: Value) -> Result<CallToolResult, rmcp::ErrorData> {
    let params = CallToolRequestParams::new(tool.to_string())
        .with_arguments(args.as_object().cloned().expect("object args"));
    client.call_tool(params).await.map_err(|e| match e {
        rmcp::ServiceError::McpError(data) => data,
        other => panic!("unexpected client error: {other:?}"),
    })
}

async fn call_ok(client: &Client, tool: &str, args: Value) -> CallToolResult {
    let res = call(client, tool, args)
        .await
        .unwrap_or_else(|e| panic!("{tool} failed: {e:?}"));
    assert_ne!(
        res.is_error,
        Some(true),
        "{tool} returned an error: {res:?}"
    );
    res
}

fn texts(res: &CallToolResult) -> Vec<String> {
    res.content
        .iter()
        .map(|c| c.as_text().expect("text content block").text.clone())
        .collect()
}

/// A field every tool's full result carries, so a test can tell the real
/// result from an empty or placeholder object.
fn assert_is_full_result(tool: &str, structured: &Value) {
    let key = match tool {
        "fetch_tool" => "content",
        "batch_fetch_tool" => "task_id",
        "summarize_tool" => "metadata",
        "get_metadata_tool" => "content_hash",
        "count_tokens_tool" => "tokens",
        "search_tool" => "results",
        other => panic!("unknown tool {other}"),
    };
    assert!(
        structured.get(key).is_some(),
        "{tool}: structuredContent lacks `{key}`: {structured}"
    );
}

/// Collapse the line breaks schemars keeps from doc comments.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn list_tools(client: &Client) -> Vec<Tool> {
    client.list_all_tools().await.expect("tools/list")
}

// ------------------------------------------------------------------ stdio

/// rmcp's `#[tool]` derives `outputSchema` only from a return type named
/// `Json`, which is why Rover's wrapper (`rover::mcp::response::Json`) carries
/// that name. If the macro ever stops matching it, schemas vanish without a
/// compile error. Every listed tool — not just the six known ones — must
/// advertise one.
#[tokio::test]
async fn every_tool_still_advertises_an_output_schema() {
    let fx = fixture().await;
    let tools = list_tools(&fx.client).await;

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in ALL_TOOLS {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    for tool in &tools {
        let schema = tool
            .output_schema
            .as_ref()
            .unwrap_or_else(|| panic!("{} has no outputSchema", tool.name));
        assert_eq!(schema.get("type"), Some(&json!("object")), "{}", tool.name);
    }

    fx.client.cancel().await.ok();
}

#[tokio::test]
async fn compatibility_mode_is_described_in_every_input_schema() {
    let fx = fixture().await;
    let tools = list_tools(&fx.client).await;
    // The hint in `content` must say what the description says, in the same
    // words, and name the argument the description is attached to.
    let hint = normalize(STRUCTURED_CONTENT_HINT);
    for phrase in [
        "`structuredContent`",
        r#"`"compatibility_mode": "on"`"#,
        "on every later call to any Rover tool.",
    ] {
        assert!(hint.contains(phrase), "{phrase}: {hint}");
    }
    let search_note = "Each call is a billable search request";
    let batch_note = "Each call queues a new batch task";

    for name in ALL_TOOLS {
        let tool = tools.iter().find(|t| t.name.as_ref() == name).unwrap();
        let schema = Value::Object((*tool.input_schema).clone());
        let property = &schema["properties"]["compatibility_mode"];
        let description = normalize(property["description"].as_str().unwrap_or_default());

        assert!(!description.is_empty(), "{name}: no description: {schema}");
        assert_eq!(property["default"], json!("off"), "{name}: {property}");
        for phrase in [
            r#"Set to `"on"` if your MCP client cannot show you `structuredContent`."#,
            "set this on every later call to any Rover tool.",
        ] {
            assert!(description.contains(phrase), "{name}: {description}");
        }
        assert_eq!(
            description.contains(search_note),
            name == "search_tool",
            "{name}: {description}"
        );
        assert_eq!(
            description.contains(batch_note),
            name == "batch_fetch_tool",
            "{name}: {description}"
        );

        // Inlined, not a `$ref`: draft-07 ignores `$ref` siblings, and some
        // client converters drop them, which would lose `description` and
        // `default` for exactly the older clients they are for.
        assert!(property.get("$ref").is_none(), "{name}: {property}");

        // Only the strings "on" and "off" are advertised.
        let values: Vec<&Value> = property["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: no inline oneOf: {property}"))
            .iter()
            .map(|v| {
                assert_eq!(v["type"], json!("string"), "{name}: {property}");
                &v["const"]
            })
            .collect();
        assert_eq!(values, [&json!("on"), &json!("off")], "{name}");
    }

    fx.client.cancel().await.ok();
}

#[tokio::test]
async fn default_mode_sends_the_result_only_in_structured_content() {
    let fx = fixture().await;
    for tool in succeeding_tools() {
        // Omitted and explicitly "off" are the same thing.
        for mode in [None, Some(json!("off"))] {
            let res = call_ok(&fx.client, tool, with_mode(valid_args(&fx, tool), mode)).await;
            assert_eq!(texts(&res), [STRUCTURED_CONTENT_HINT], "{tool}");
            let structured = res
                .structured_content
                .as_ref()
                .unwrap_or_else(|| panic!("{tool}: no structuredContent"));
            assert_is_full_result(tool, structured);
        }
    }
    fx.client.cancel().await.ok();
}

#[tokio::test]
async fn compatibility_mode_on_also_sends_the_result_as_json_text() {
    let fx = fixture().await;
    for tool in succeeding_tools() {
        let args = with_mode(valid_args(&fx, tool), Some(json!("on")));
        let res = call_ok(&fx.client, tool, args).await;
        let structured = res
            .structured_content
            .as_ref()
            .unwrap_or_else(|| panic!("{tool}: no structuredContent in compatibility mode"));
        assert_is_full_result(tool, structured);

        let texts = texts(&res);
        assert_eq!(texts.len(), 1, "{tool}: {texts:?}");
        let parsed: Value = serde_json::from_str(&texts[0])
            .unwrap_or_else(|e| panic!("{tool}: content is not JSON ({e}): {}", texts[0]));
        assert_eq!(&parsed, structured, "{tool}");
    }
    fx.client.cancel().await.ok();
}

/// Mirrors the TypeScript SDK client, which throws when a tool that
/// advertises `outputSchema` returns a non-error result without
/// `structuredContent`. Driven from `tools/list`, in every mode.
#[tokio::test]
async fn structured_content_accompanies_every_output_schema() {
    let fx = fixture().await;
    let tools = list_tools(&fx.client).await;
    let mut checked = 0;
    for tool in tools.iter().filter(|t| t.output_schema.is_some()) {
        let name = tool.name.as_ref();
        for mode in [None, Some(json!("off")), Some(json!("on"))] {
            let Ok(res) = call(&fx.client, name, with_mode(valid_args(&fx, name), mode)).await
            else {
                continue; // a JSON-RPC error is not a result
            };
            if res.is_error == Some(true) {
                continue;
            }
            assert!(
                res.structured_content.is_some(),
                "Tool {name} has an output schema but did not return structured content"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, succeeding_tools().len() * 3, "some calls failed");
    fx.client.cancel().await.ok();
}

#[tokio::test]
async fn an_invalid_compatibility_mode_is_rejected() {
    let fx = fixture().await;
    for tool in ALL_TOOLS {
        for bad in [
            json!("yes"),
            json!(true),
            json!("ON"),
            json!(null),
            json!({ "on": null }),
            json!({ "off": null }),
        ] {
            let args = with_mode(valid_args(&fx, tool), Some(bad.clone()));
            let err = call(&fx.client, tool, args)
                .await
                .expect_err(&format!("{tool} accepted compatibility_mode {bad}"));
            assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{tool} {bad}: {err:?}");
        }
    }
    // Rejected before any work: nothing fetched, nothing searched, and — the
    // direct proof for `batch_fetch`, whose fetching is backgrounded — no task
    // queued.
    assert!(fx.origin.received_requests().await.unwrap().is_empty());
    assert!(fx.provider.received_requests().await.unwrap().is_empty());
    assert_eq!(task_count(&fx), 0, "an invalid batch_fetch queued a task");

    // Control: a valid call does show up in the same count, so the zero above
    // is not an artefact of reading the wrong database.
    call_ok(
        &fx.client,
        "batch_fetch_tool",
        valid_args(&fx, "batch_fetch_tool"),
    )
    .await;
    assert_eq!(task_count(&fx), 1);

    fx.client.cancel().await.ok();
}

/// Rows in the spawned server's `tasks` table.
fn task_count(fx: &Fixture) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(
        fx.data_dir.path().join("rover.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the server's database");
    conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
        .expect("count tasks")
}

/// Tool errors are JSON-RPC errors, not `isError` results, so the response
/// layout never applies to them.
#[tokio::test]
async fn tool_errors_are_unaffected_by_compatibility_mode() {
    let fx = fixture().await;
    for mode in [None, Some(json!("off")), Some(json!("on"))] {
        let err = call(
            &fx.client,
            "count_tokens_tool",
            with_mode(json!({}), mode.clone()),
        )
        .await
        .expect_err("count_tokens with neither text nor url must fail");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{mode:?}");
        let data = err.data.expect("structured error envelope");
        assert_eq!(data["code"], json!("invalid_args"), "{mode:?}: {data}");
    }
    fx.client.cancel().await.ok();
}

/// The end state #54 and #70 exist for: a default `fetch` carries the page
/// body exactly once in the whole response.
#[tokio::test]
async fn fetch_sends_the_page_body_once_in_the_whole_response() {
    let fx = fixture().await;
    let args = valid_args(&fx, "fetch_tool");

    let res = call_ok(&fx.client, "fetch_tool", args.clone()).await;
    assert_eq!(texts(&res), [STRUCTURED_CONTENT_HINT]);
    let document = res.structured_content.as_ref().unwrap()["content"]
        .as_str()
        .expect("structuredContent.content");
    assert_eq!(document.matches(SENTENCE).count(), 1, "{document}");
    let wire = serde_json::to_string(&res).unwrap();
    assert_eq!(wire.matches(SENTENCE).count(), 1, "{wire}");

    // Compatibility mode: once in each of the two fields, no more.
    let res = call_ok(&fx.client, "fetch_tool", with_mode(args, Some(json!("on")))).await;
    let document = res.structured_content.as_ref().unwrap()["content"]
        .as_str()
        .unwrap();
    assert_eq!(document.matches(SENTENCE).count(), 1, "{document}");
    assert_eq!(texts(&res)[0].matches(SENTENCE).count(), 1);
    let wire = serde_json::to_string(&res).unwrap();
    assert_eq!(wire.matches(SENTENCE).count(), 2, "{wire}");

    fx.client.cancel().await.ok();
}

// ------------------------------------------------------------------- HTTP

/// POST one `tools/call` to an in-process HTTP router and return the
/// JSON-RPC `result` exactly as it went over the wire.
async fn http_call(data_dir: &std::path::Path, args: Value) -> Value {
    let app = rover::mcp::http::router(common::http_state(data_dir, None).await);
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "count_tokens_tool", "arguments": args }
    });
    let res = app
        .oneshot(common::mcp_request(
            "POST",
            None,
            axum::body::Body::from(body.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let rpc: Value = serde_json::from_slice(&bytes).expect("JSON-RPC response");
    rpc.get("result")
        .unwrap_or_else(|| panic!("no result: {rpc}"))
        .clone()
}

#[tokio::test]
async fn http_transport_uses_the_same_layout() {
    let tmp = tempfile::tempdir().unwrap();

    let result = http_call(tmp.path(), json!({ "text": "hello world" })).await;
    assert_eq!(
        result["content"],
        json!([{ "type": "text", "text": STRUCTURED_CONTENT_HINT }]),
        "{result}"
    );
    assert!(result["structuredContent"]["tokens"].is_u64(), "{result}");

    let result = http_call(
        tmp.path(),
        json!({ "text": "hello world", "compatibility_mode": "on" }),
    )
    .await;
    let text = result["content"][0]["text"].as_str().expect("text block");
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed, result["structuredContent"], "{result}");
}

#[tokio::test]
async fn fetch_over_a_real_http_socket_sends_the_body_once() {
    use rmcp::ServiceExt as _;
    use rmcp::transport::StreamableHttpClientTransport;

    common::init_crypto();
    let origin = origin().await;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = "[robots]\nrespect = false\n\n[ssrf]\nlevel = \"loopback\"\n";
    let (mut child, base) = common::spawn_http_server(tmp.path(), cfg, None).await;
    let transport = StreamableHttpClientTransport::from_uri(format!("{base}/mcp"));
    let client = ().serve(transport).await.expect("http handshake");

    let url = format!("{}/p", origin.uri());
    let res = call_ok(&client, "fetch_tool", json!({ "url": url })).await;
    assert_eq!(texts(&res), [STRUCTURED_CONTENT_HINT]);
    let wire = serde_json::to_string(&res).unwrap();
    assert_eq!(wire.matches(SENTENCE).count(), 1, "{wire}");

    let res = call_ok(
        &client,
        "fetch_tool",
        json!({ "url": url, "compatibility_mode": "on" }),
    )
    .await;
    let parsed: Value = serde_json::from_str(&texts(&res)[0]).unwrap();
    assert_eq!(Some(&parsed), res.structured_content.as_ref());

    client.cancel().await.ok();
    child.kill().await.ok();
}
