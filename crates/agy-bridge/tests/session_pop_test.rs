//! Guard tests for conversation history semantics via the mock test proxy.
//!
//! # Why there is no `remove_last_turn`
//!
//! An earlier build exposed `Agent::remove_last_turn` for "safety recovery"
//! (pop a refusal, retry clean). An end-to-end test through the real `PyO3`
//! runtime + Antigravity SDK against a [`MockGeminiServer`] proved it was a
//! **silent no-op**, and the root cause makes it *unimplementable* from the
//! bridge:
//!
//! 1. The SDK conversation keeps its transcript in `Conversation._steps`
//!    (with turn boundaries in `_turn_start_indices`) — there is **no**
//!    `_history` attribute, so the old implementation's
//!    `hasattr("_history")` check always failed and no-op'd.
//! 2. More fundamentally, `LocalConnection.send()` transmits only the *new*
//!    `user_input` to the localharness subprocess. The **harness** owns the
//!    authoritative conversation `contents` and calls Gemini; its input
//!    protocol (`InputEvent`) has only `user_input`, `complex_user_input`,
//!    and `tool_confirmation` — **no rewind/remove/truncate**. Manipulating
//!    any Python-side state therefore cannot change what the model sees.
//!
//! The Python SDK itself has no turn-removal capability (only
//! `clear_history()`, which likewise clears the Python-side transcript, not
//! the harness context). Rather than ship a misleading API, the bridge
//! intentionally does **not** expose one.
//!
//! These tests lock in the observable behavior that made removal impossible:
//! conversation history is **cumulative** on the wire — every turn resends
//! all prior turns. If someone reintroduces a turn-removal API, they must
//! make it actually change the wire payload, and these tests document the
//! bar it has to clear.
//!
//! Run with:
//! ```sh
//! cargo test --test session_pop_test -- --nocapture
//! ```

use agy_bridge_test_support::*;

/// Locate the recorded POST that carried a given user-message marker in its
/// body. Searches newest-first so the most recent matching turn wins.
async fn request_carrying(server: &MockGeminiServer, marker: &str) -> String {
    let posts = server.recorded_posts().await;
    posts
        .iter()
        .rev()
        .find(|p| p.body.contains(marker))
        .unwrap_or_else(|| {
            panic!(
                "no recorded request carried marker {marker:?}; recorded {} post(s)",
                posts.len()
            )
        })
        .body
        .clone()
}

/// A brand-new conversation's first request carries only that first turn —
/// establishing the baseline before cumulative growth is asserted below.
#[test]
fn first_turn_carries_only_itself() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server =
            MockGeminiServer::start(vec![MockResponse::Text("ONE_MODEL_REPLY".into())]).await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "history-baseline"))
            .await
            .expect("agent");

        let reply = agent.chat_text("ONE_USER_MSG").await.expect("turn 1");
        assert_eq!(reply.trim(), "ONE_MODEL_REPLY", "turn 1 reply");

        let wire = request_carrying(&server, "ONE_USER_MSG").await;
        assert!(
            wire.contains("ONE_USER_MSG"),
            "turn-1 request must carry its own user msg. Body: {wire}"
        );

        agent.shutdown().await.expect("shutdown");
    });
}

/// Conversation history is **cumulative and harness-owned**: each turn's
/// request resends the full transcript of all prior turns. This is the exact
/// property that makes a bridge-side "remove last turn" impossible — the
/// authoritative context lives in the localharness, not in any Rust/Python
/// state the bridge can edit.
///
/// Uses unique markers (`ONE_*`, `TWO_*`, `THREE_*`) so substring checks
/// against the JSON request body are unambiguous.
#[test]
fn history_accumulates_cumulatively_on_the_wire() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![
            MockResponse::Text("ONE_MODEL_REPLY".into()),
            MockResponse::Text("TWO_MODEL_REPLY".into()),
            MockResponse::Text("THREE_MODEL_REPLY".into()),
        ])
        .await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "history-cumulative"))
            .await
            .expect("agent");

        // ── Turn 1 ──
        let t1 = agent.chat_text("ONE_USER_MSG").await.expect("turn 1");
        assert_eq!(t1.trim(), "ONE_MODEL_REPLY", "turn 1 reply");

        // ── Turn 2: its request must already carry turn 1 in history ──
        let t2 = agent.chat_text("TWO_USER_MSG").await.expect("turn 2");
        assert_eq!(t2.trim(), "TWO_MODEL_REPLY", "turn 2 reply");

        let turn2_wire = request_carrying(&server, "TWO_USER_MSG").await;
        assert!(
            turn2_wire.contains("ONE_USER_MSG"),
            "turn-2 request must resend turn-1 user msg. Body: {turn2_wire}"
        );
        assert!(
            turn2_wire.contains("ONE_MODEL_REPLY"),
            "turn-2 request must resend turn-1 model reply. Body: {turn2_wire}"
        );

        // ── Turn 3: its request must carry the full transcript of 1 + 2 ──
        let t3 = agent.chat_text("THREE_USER_MSG").await.expect("turn 3");
        assert_eq!(t3.trim(), "THREE_MODEL_REPLY", "turn 3 reply");

        let wire = request_carrying(&server, "THREE_USER_MSG").await;
        for marker in [
            "ONE_USER_MSG",
            "ONE_MODEL_REPLY",
            "TWO_USER_MSG",
            "TWO_MODEL_REPLY",
        ] {
            assert!(
                wire.contains(marker),
                "turn-3 request must resend the full cumulative history \
                 (missing {marker:?}). Body: {wire}"
            );
        }

        agent.shutdown().await.expect("shutdown");
    });
}
