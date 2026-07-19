//! Long-running conversation + agent-liveness live tests.
//!
//! These tests defend two guarantees that matter for real, long-lived agent
//! sessions driven through agy-bridge:
//!
//! 1. **Long-running conversations work.** A single agent handles many
//!    sequential turns while retaining conversation state end-to-end. There is
//!    no per-turn wall-clock cap that would truncate a legitimately long turn
//!    (`ChatResponseHandle::text()` drains until the stream naturally closes).
//!
//! 2. **We never kill a running agent by accident.** Abandoning a response
//!    handle (a consumer that gives up mid-stream) and issuing concurrent
//!    read-only queries *while a turn is in flight* must not tear down the
//!    agent: the turn still completes and subsequent turns still work.
//!
//! Run with:
//! ```sh
//! GEMINI_API_KEY="..." cargo test --test long_running_live_test -- --nocapture
//! ```

mod common;

use common::{api_key, create_bridge, run_live_test, test_runtime};

/// A concise, memory-focused persona so turns stay small (cheap on TPM) while
/// still exercising cross-turn state retention.
fn memory_agent_config() -> agy_bridge::config::AgentConfig {
    agy_bridge::config::AgentConfig::builder()
        .system_instructions(
            "You are a helpful assistant with perfect memory. \
             Answer in one short sentence.",
        )
        .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
        .build()
}

// =============================================================================
// Test: a long, stateful, multi-turn conversation runs to completion.
// =============================================================================

#[test]
fn live_long_running_multiturn_conversation() {
    run_live_test("live_long_running_multiturn_conversation", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
            let agent = bridge.agent(memory_agent_config()).await?;

            // Turn 1: plant a fact the agent must recall at the very end.
            let planted = agent
                .chat("Remember this secret number: 42. Just acknowledge.")
                .await?
                .text()
                .await?;
            eprintln!("[turn 1] ack: {}", planted.text());

            // A run of ordinary turns to build a genuinely long conversation.
            // Each must return non-empty text — i.e. the turn ran to completion
            // and was not truncated or aborted.
            let prompts = [
                "What is 2 + 2?",
                "Name a primary color.",
                "What is the capital of Japan?",
                "Say the opposite of 'hot'.",
                "How many days are in a week?",
                "Name a fruit.",
                "What comes after Tuesday?",
                "Count to three.",
            ];
            for (i, prompt) in prompts.iter().enumerate() {
                let reply = agent.chat(*prompt).await?.text().await?;
                let text = reply.text();
                eprintln!("[turn {}] {prompt} -> {text}", i + 2);
                assert!(
                    !text.trim().is_empty(),
                    "turn {} ('{prompt}') returned empty text — the turn was \
                     truncated or the agent was killed mid-conversation",
                    i + 2
                );
            }

            // Turn count must reflect every completed turn (plant + prompts +
            // the upcoming recall turn is not counted yet).
            let expected_turns = u32::try_from(prompts.len() + 1).expect("turn count fits in u32");
            let turns = agent.turn_count().await?;
            assert_eq!(
                turns, expected_turns,
                "expected {expected_turns} completed turns in a long-running \
                 conversation, got {turns}"
            );

            // Final turn: the agent must still remember the fact from turn 1,
            // proving state persisted across the whole long conversation.
            let recall = agent
                .chat("What was the secret number I told you at the start?")
                .await?
                .text()
                .await?;
            let recall_text = recall.text();
            eprintln!("[recall] {recall_text}");
            assert!(
                recall_text.contains("42"),
                "agent lost long-conversation memory: expected it to recall \
                 '42', got: {recall_text}"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: abandoning a response handle does not kill the running agent.
// =============================================================================

#[test]
fn live_abandoned_response_does_not_kill_agent() {
    run_live_test("live_abandoned_response_does_not_kill_agent", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
            let agent = bridge.agent(memory_agent_config()).await?;

            // Turn 1: start a turn and then DROP the response handle without
            // draining it. This simulates a consumer that gives up mid-stream.
            // Dropping the response handle must NOT drop/shut down the agent.
            {
                let response = agent
                    .chat("Write a short sentence about the ocean.")
                    .await?;
                drop(response); // abandon it — no .text(), no draining.
            }

            // Turn 2: the agent must still be alive and fully functional.
            let reply = agent.chat("What is 10 minus 3?").await?.text().await?;
            let text = reply.text();
            eprintln!("[post-abandon] 10 - 3 -> {text}");
            assert!(
                !text.trim().is_empty(),
                "agent was killed by an abandoned response: turn 2 returned \
                 empty text"
            );
            let lower = text.to_lowercase();
            assert!(
                lower.contains('7') || lower.contains("seven"),
                "agent should still answer correctly after an abandoned \
                 response, got: {text}"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}

// =============================================================================
// Test: concurrent read-only queries during an in-flight turn don't abort it.
// =============================================================================

#[test]
fn live_concurrent_reads_do_not_abort_running_turn() {
    run_live_test("live_concurrent_reads_do_not_abort_running_turn", || {
        let _api_key = api_key();
        let rt = test_runtime();

        rt.block_on(async {
            let bridge = create_bridge();
            let config = agy_bridge::config::AgentConfig::builder()
                .system_instructions("You are a storyteller. Write a vivid six sentence story.")
                .capabilities(agy_bridge::config::CapabilitiesConfig::custom_tools_only())
                .build();
            let agent = bridge.agent(config).await?;

            // Kick off a deliberately longer turn.
            let response = agent
                .chat("Tell me a six sentence story about a lighthouse keeper.")
                .await?;

            // While the turn streams, hammer the runtime with concurrent
            // read-only queries. The command loop processes commands
            // concurrently, so these must neither block nor abort the turn.
            let drain = async { response.text().await };
            let poke = async {
                for _ in 0..6 {
                    if let Err(e) = agent.turn_count().await {
                        eprintln!("concurrent turn_count() during turn: {e}");
                    }
                    if let Err(e) = agent.history().await {
                        eprintln!("concurrent history() during turn: {e}");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
                }
            };

            let (drain_result, ()) = tokio::join!(drain, poke);
            let reply = drain_result?;
            let text = reply.text();
            eprintln!("[concurrent] story len = {} chars", text.len());
            assert!(
                !text.trim().is_empty(),
                "the in-flight turn produced no text — concurrent reads \
                 aborted the running agent"
            );

            // Agent remains usable for a follow-up turn.
            let followup = agent
                .chat("In one word, what was that about?")
                .await?
                .text()
                .await?;
            assert!(
                !followup.text().trim().is_empty(),
                "agent unusable after concurrent reads during a turn"
            );

            agent.shutdown().await?;
            Ok(())
        })
    });
}
