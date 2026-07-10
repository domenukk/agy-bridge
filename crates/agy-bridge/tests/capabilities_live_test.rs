//! Built-in file tools and capabilities configuration tests.
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY="..." cargo test --test capabilities_live_test -- --nocapture
//! ```

mod common;

use common::{api_key, create_bridge, run_live_test, test_runtime};

// =============================================================================
// Test: Agent with built-in file tools
// =============================================================================

#[test]
fn live_agent_with_builtin_tools() {
    run_live_test("live_agent_with_builtin_tools", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();

            // Create a temp file for the agent to read under a non-hidden workspace prefix.
            let td = tempfile::Builder::new()
                .prefix("my-workspace")
                .tempdir()
                .expect("tempdir");
            let temp_dir = td.path().to_path_buf();
            std::fs::create_dir_all(&temp_dir).expect("create temp dir");
            let temp_path = temp_dir.join("secret.txt");
            std::fs::write(&temp_path, "The secret code is GAMMA-42.").expect("write temp file");

            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions(
                    "You are a file reader. Read files when asked and report their contents.",
                )
                .policies([agy_bridge::policies::PolicyRule::AllowAll])
                .workspaces(vec![temp_dir.clone()])
                .build();

            let agent = bridge.agent(config).await?;

            let prompt = format!(
                "Read the file at {} and tell me the secret code.",
                temp_path.display()
            );
            let text = agent.chat_text(&*prompt).await?;
            agent.shutdown().await?;

            eprintln!("Agent response: {text}");
            assert!(
                text.contains("GAMMA-42"),
                "Expected agent to read the file and return 'GAMMA-42', got: {text}"
            );
            Ok(())
        })
    });
}
