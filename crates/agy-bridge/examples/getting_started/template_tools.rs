//! Demonstrates `md-tmpl` integration with `#[llm_tool]`.
//!
//! - `prompt_file` loads the tool **description** (shown to the LLM) from a
//!   `.tmpl.md` template. Compile-time `params(...)` are baked into the
//!   description at zero runtime cost.
//! - `response_file` renders the tool's **return value** through a template
//!   before the LLM sees it, keeping output formatting out of Rust code.

use agy_bridge::{AgyBridge, config::AgentConfig, prelude::*, tools::ToolRegistry};
use serde::Serialize;

/// A single file match returned by the search tool.
#[derive(Serialize)]
struct FileMatch {
    path: String,
}

/// The full result set returned by the search tool.
#[derive(Serialize)]
struct SearchResult {
    matches: Vec<FileMatch>,
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
    // In a real tool this would walk the filesystem; here we return
    // hard-coded paths so the example is self-contained.
    let results = vec![
        FileMatch {
            path: format!("{directory}/README.md"),
        },
        FileMatch {
            path: format!("{directory}/src/main.rs"),
        },
        FileMatch {
            path: format!("{directory}/docs/{pattern}.md"),
        },
    ];
    Ok(SearchResult {
        matches: results,
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
    fn description_comes_from_template() {
        let desc = <SearchFiles as RustTool>::DESCRIPTION;
        assert!(
            desc.contains("Search for files matching a pattern in a directory."),
            "Expected template body in DESCRIPTION, got: {desc}"
        );
    }

    #[tokio::test]
    async fn basic_execution() {
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
    async fn exact_output_format() {
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
        let expected = "\nResults for \"test.rs\":\n- /dir/README.md\n- /dir/src/main.rs\n- /dir/docs/test.rs.md\n";
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn template_injection_is_literal() {
        let tool = SearchFiles;
        let ctx = ToolContext::new(None);

        // Values containing template syntax must be treated as literal strings,
        // never re-evaluated by the template engine.
        let injection = "{{ 7 * 7 }} {% for x in y %} <script>";
        let output = tool
            .call(
                SearchFilesParams {
                    pattern: injection.to_string(),
                    directory: "/sec".to_string(),
                },
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            output.content().contains(injection),
            "Template syntax in user data must pass through unchanged"
        );
    }

    #[tokio::test]
    async fn unicode_and_special_characters() {
        let tool = SearchFiles;
        let ctx = ToolContext::new(None);

        let pattern = "🦀_test_\"quotes\"_\n_newline";
        let output = tool
            .call(
                SearchFilesParams {
                    pattern: pattern.to_string(),
                    directory: "/🦀".to_string(),
                },
                &ctx,
            )
            .await
            .unwrap();
        let content = output.content();
        assert!(content.contains("🦀_test_\"quotes\"_\n_newline"));
        assert!(content.contains("- /🦀/README.md"));
    }

    #[tokio::test]
    async fn empty_inputs() {
        let tool = SearchFiles;
        let ctx = ToolContext::new(None);

        let output = tool
            .call(
                SearchFilesParams {
                    pattern: String::new(),
                    directory: String::new(),
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
