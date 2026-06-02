//! Demonstrates custom error handling for agent operations.
//!
//! Shows how to match on `Error` variants to handle different
//! failure modes: invalid configuration, runtime errors, and
//! unexpected successes.

use agy_bridge::{
    AgyBridge,
    config::{AgentConfig, CapabilitiesConfig},
    error::Error,
};

#[tokio::main]
async fn main() {
    agy_bridge::load_dotenv();
    // ── 1. Handle bridge initialisation errors ──────────────────────────
    let bridge = match AgyBridge::builder().build() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to start bridge: {e}");
            return;
        }
    };

    // ── 2. Trigger a config validation error (mutually exclusive fields) ─
    let bad_caps = CapabilitiesConfig {
        enabled_tools: Some(vec![]),
        disabled_tools: Some(vec![]),
        ..CapabilitiesConfig::default()
    };
    let bad_config = AgentConfig::builder().capabilities(bad_caps).build();

    match bridge.agent(bad_config).await {
        Err(Error::InvalidConfig { message }) => {
            println!("Caught InvalidConfig (expected): {message}");
        }
        Err(other) => {
            println!("Unexpected error variant: {other}");
        }
        Ok(_) => {
            println!("Unexpectedly succeeded — this should not happen");
        }
    }

    // ── 3. Trigger a runtime error with a non-existent model ────────────
    let bad_model = AgentConfig::builder()
        .model("this-model-does-not-exist-999")
        .build();

    match bridge.agent(bad_model).await {
        Err(e) => {
            println!("\nCaught runtime error (expected): {e}");
        }
        Ok(agent) => {
            println!("Unexpectedly created agent with bad model");
            if let Err(e) = agent.shutdown().await {
                eprintln!("Failed to shut down agent: {e}");
            }
        }
    }
}
