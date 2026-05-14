mod common;

use rmcp::model::CallToolRequestParams;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE_HTML: &str = include_str!("fixtures/m4/article-jsonld-og-twitter.html");

#[tokio::test]
async fn lists_get_metadata_tool() {
    let tmp = tempfile::tempdir().unwrap();
    common::seed_default_tokenizer(tmp.path());
    let client = common::spawn_client(tmp.path()).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"get_metadata_tool"),
        "missing get_metadata_tool: {names:?}"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn get_metadata_returns_expected_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(FIXTURE_HTML),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    common::seed_default_tokenizer(tmp.path());
    let client = common::spawn_client(tmp.path()).await;

    let args = json!({"url": server.uri()});
    let mut params = CallToolRequestParams::new("get_metadata_tool".to_string());
    if let Some(obj) = args.as_object().cloned() {
        params = params.with_arguments(obj);
    }
    let res = client.call_tool(params).await.unwrap();
    let blob = serde_json::to_string(&res).unwrap();
    assert!(
        blob.contains("\"title\""),
        "expected title field in response: {blob}"
    );
    assert!(
        blob.contains("\"schema_types\""),
        "expected schema_types: {blob}"
    );
    assert!(
        blob.contains("\"extraction_quality\""),
        "expected extraction_quality: {blob}"
    );

    client.cancel().await.unwrap();
    drop(server);
}
