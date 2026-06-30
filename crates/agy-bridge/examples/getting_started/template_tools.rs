//! Demonstrates `prompt-templates` integration with `#[llm_tool]`.
//!
//! - `search_files` uses `prompt_file` to load its description from a `.tmpl.md` file.
//! - `search_files` uses `response_file` to render tool output through a template.

use agy_bridge::{AgyBridge, config::AgentConfig, prelude::*, tools::ToolRegistry};
use serde::Serialize;

/// A single search match returned by the tool.
#[derive(Serialize)]
struct Match {
    path: String,
}

/// The full result set returned by the tool.
#[derive(Serialize)]
struct SearchResult {
    matches: Vec<Match>,
    query: String,
}

#[llm_tool(
    prompt_file = "examples/getting_started/tools/search_files.tmpl.md",
    response_file = "examples/getting_started/tools/search_results.tmpl.md",
    params(pattern = "*", directory = "/project")
)]
fn search_files(
    /// Glob pattern to match against file names.
    pattern: &str,
    /// Root directory to search from.
    directory: &str,
) -> Result<SearchResult, String> {
    // Simulate finding some files.
    let fake_matches = vec![
        Match {
            path: format!("{directory}/README.md"),
        },
        Match {
            path: format!("{directory}/src/main.rs"),
        },
        Match {
            path: format!("{directory}/docs/{pattern}.md"),
        },
    ];
    Ok(SearchResult {
        matches: fake_matches,
        query: pattern.to_string(),
    })
}

#[tokio::main]
async fn main() -> Result<(), agy_bridge::error::Error> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    let mut registry = ToolRegistry::new();
    registry.register(SearchFiles);
    let config = AgentConfig::builder().build();
    let agent = bridge.agent(config).tools(registry).await?;

    let prompt = "Find all markdown files in /project";
    println!("  User: {prompt}");
    let response_text = agent.chat(prompt).await?.text().await?;
    println!("  Agent: {response_text}");

    agent.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use llm_tool::{RustTool, ToolContext};

    use super::*;

    #[test]
    fn test_search_files_description() {
        let desc = <SearchFiles as RustTool>::DESCRIPTION;
        assert!(desc.contains("Search for files matching a pattern in a directory."));
    }

    #[tokio::test]
    async fn test_search_files_execution() {
        let tool = SearchFiles;
        let ctx = ToolContext::new(None);
        let output = tool
            .call(
                SearchFilesParams {
                    pattern: "main.rs".to_string(),
                    directory: "/app".to_string(),
                },
                &ctx,
            )
            .await
            .unwrap();
        let content = output.content();
        assert!(content.contains("Results for \"main.rs\":"));
        assert!(content.contains("- /app/src/main.rs"));
    }

    #[tokio::test]
    async fn test_search_files_exact_output() {
        let tool = SearchFiles;
        let ctx = ToolContext::new(None);
        let output = tool
            .call(
                SearchFilesParams {
                    pattern: "test.rs".to_string(),
                    directory: "/dir".to_string(),
                },
                &ctx,
            )
            .await
            .unwrap();
        let content = output.content();
        // Verify the exact formatting produced by search_results.tmpl.md
        let expected = "\nResults for \"test.rs\":\n- /dir/README.md\n- /dir/src/main.rs\n- /dir/docs/test.rs.md\n";
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_search_files_stress_edge_cases() {
        let tool = SearchFiles;
        let ctx = ToolContext::new(None);

        // 1. Template injection resilience
        let injection_pattern = "{{ 7 * 7 }} {% for x in y %} <script>";
        let output = tool
            .call(
                SearchFilesParams {
                    pattern: injection_pattern.to_string(),
                    directory: "/sec".to_string(),
                },
                &ctx,
            )
            .await
            .unwrap();
        let content = output.content();
        assert!(
            content.contains(injection_pattern),
            "Should treat pattern as literal string without double evaluation"
        );

        // 2. Unicode and special characters
        let unicode_pattern = "🦀_test_\"quotes\"_\n_newline";
        let output = tool
            .call(
                SearchFilesParams {
                    pattern: unicode_pattern.to_string(),
                    directory: "/🦀".to_string(),
                },
                &ctx,
            )
            .await
            .unwrap();
        let content = output.content();
        assert!(content.contains("🦀_test_\"quotes\"_\n_newline"));
        assert!(content.contains("- /🦀/README.md"));

        // 3. Empty strings
        let output = tool
            .call(
                SearchFilesParams {
                    pattern: "".to_string(),
                    directory: "".to_string(),
                },
                &ctx,
            )
            .await
            .unwrap();
        let content = output.content();
        assert!(content.contains("Results for \"\":"));
        assert!(content.contains("- /README.md"));
    }
}
