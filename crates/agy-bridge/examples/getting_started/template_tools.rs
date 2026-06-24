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
    response_file = "examples/getting_started/tools/search_results.tmpl.md"
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
