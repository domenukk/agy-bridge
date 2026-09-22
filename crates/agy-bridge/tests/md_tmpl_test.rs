//! Integration tests for `md-tmpl` support with `#[llm_tool]` and `agy-bridge`.
#![cfg(feature = "md-tmpl")]

use agy_bridge::{
    prelude::*,
    tools::{RustTool, ToolContext, ToolRegistry},
};
use agy_bridge_test_support::*;
use serde::Serialize;

#[derive(Serialize)]
struct FileMatch {
    path: String,
}

#[derive(Serialize)]
struct SearchResult {
    matches: Vec<FileMatch>,
    query: String,
}

#[llm_tool(
    description_file = "examples/getting_started/tools/search_files.tmpl.md",
    response_file = "examples/getting_started/tools/search_results.tmpl.md",
    params(pattern = "*", directory = "/project")
)]
fn search_files_test(
    /// Glob pattern to match against file names.
    pattern: &str,
    /// Root directory to search from.
    directory: &str,
) -> Result<SearchResult, String> {
    Ok(SearchResult {
        matches: vec![
            FileMatch {
                path: format!("{directory}/README.md"),
            },
            FileMatch {
                path: format!("{directory}/src/main.rs"),
            },
            FileMatch {
                path: format!("{directory}/docs/{pattern}.md"),
            },
        ],
        query: pattern.to_string(),
    })
}

#[test]
fn test_tool_template_description() {
    let desc = <SearchFilesTest as RustTool>::DESCRIPTION;
    assert!(
        desc.contains("Search for files matching a pattern in a directory."),
        "Expected template body in DESCRIPTION, got: {desc}"
    );
}

#[tokio::test]
async fn test_tool_template_rendering_execution() {
    let tool = SearchFilesTest;
    let ctx = ToolContext::new();
    let output = tool
        .call(
            SearchFilesTestParams {
                pattern: "test.rs".to_string(),
                directory: "/workspace".to_string(),
            },
            &ctx,
        )
        .await
        .expect("tool call succeeds");

    let content = output.content();
    assert!(content.contains("Results for \"test.rs\":"));
    assert!(content.contains("- /workspace/README.md"));
    assert!(content.contains("- /workspace/src/main.rs"));
    assert!(content.contains("- /workspace/docs/test.rs.md"));
}

#[test]
fn test_template_tool_in_mock_server_agent() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::FunctionCall {
                name: "search_files_test".into(),
                args: serde_json::json!({
                    "pattern": "lib.rs",
                    "directory": "/src"
                }),
            },
            MockResponse::Text("Found files successfully.".into()),
        ])
        .await;

        let mut registry = ToolRegistry::new();
        registry.register(SearchFilesTest);

        let bridge = shared_bridge();
        let agent = bridge
            .agent(agent_config(&server.base_url(), "searcher"))
            .tools(registry)
            .await
            .expect("agent created");

        let response = agent.chat_text("find files").await.expect("chat completes");
        assert!(
            response.contains("Found files successfully."),
            "Expected response to contain 'Found files successfully.', got: {response}"
        );

        let posts = server.recorded_posts().await;
        assert_eq!(posts.len(), 2, "Expected 2 POST requests (call + response)");

        let second_body = &posts[1].body;
        assert!(
            second_body.contains("Results for") && second_body.contains("lib.rs"),
            "Expected rendered template header in function response, got: {second_body}"
        );
        assert!(
            second_body.contains("/src/README.md")
                && second_body.contains("/src/src/main.rs")
                && second_body.contains("/src/docs/lib.rs.md"),
            "Expected rendered template matches in function response, got: {second_body}"
        );

        agent.shutdown().await.expect("shutdown succeeds");
    });
}
