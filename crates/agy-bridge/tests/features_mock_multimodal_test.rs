//! Mock-server integration test: multimodal (image) input transport.
//!
//! Construction and serde of media types are unit-tested in `content::media`.
//! This suite closes the remaining gap for **images**: verifying that image
//! bytes attached to a chat turn are actually transmitted to the backend,
//! exercising the full Rust → Python → localharness → HTTP pipeline.
//! **No API key required.**
//!
//! ## Why only images are covered *here* (in the mock suite)
//!
//! The Gemini backend uses two transports depending on media type:
//! - **Images** are sent *inline* as base64 inside the `generateContent`
//!   request body, so the `MockGeminiServer` receives (and can assert on) them.
//! - **Video and documents** are uploaded via the resumable *Files API*,
//!   performed by the compiled `localharness` binary, which does **not** route
//!   the upload through the injected `base_url`. The bytes therefore never reach
//!   the mock (an audio/video/document turn produces zero requests to it), so
//!   the upload path can only be verified *live*. Those live tests exist in
//!   `conversation_live_test.rs`: `live_multimodal_video_mp4` and
//!   `live_multimodal_document_pdf` (both confirmed passing end-to-end).
//! - **Audio** currently does **not** work through the harness at all: the
//!   `localharness` "translate input" step rejects every audio MIME the Python
//!   SDK advertises (`audio/wav`, `audio/mp3`, `audio/aac`, `audio/ogg`,
//!   `audio/flac`, ...) with `unsupported MIME type`, and then hangs instead of
//!   returning an error. This was reproduced in pure Python (no Rust, no mock),
//!   so it is a harness/SDK-contract bug, not an agy-bridge defect. No audio
//!   test can pass until the harness is fixed.
//!
//! So this mock suite covers the one mock-observable transport (inline images);
//! the upload transports are covered by the live suite.
//!
//! Run with:
//! ```sh
//! cargo test --test features_mock_multimodal_test -- --nocapture
//! ```

use agy_bridge::content::{Content, ContentPrimitive, Image};
use agy_bridge_test_support::*;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

/// An image is inlined as base64 data in the `generateContent` request body,
/// alongside the text prompt.
#[test]
fn image_input_encoded_as_base64_inline() {
    let rt = multi_thread_rt();
    rt.block_on(async {
        let server = MockGeminiServer::start(vec![MockResponse::Text("I see it.".into())]).await;

        let agent = BRIDGE
            .agent(agent_config(&server.base_url(), "vision"))
            .await
            .expect("agent");

        // Distinctive bytes so we can locate their base64 in the request body.
        let image_bytes = b"AGY_IMAGE_MARKER_PIXELS_0123456789".to_vec();
        let expected_b64 = BASE64.encode(&image_bytes);

        let content = Content::from(vec![
            ContentPrimitive::Text {
                text: "describe this image".into(),
            },
            ContentPrimitive::Image(Image::png(image_bytes)),
        ]);

        let text = agent.chat_text(content).await.expect("chat");
        assert_eq!(text.trim(), "I see it.");

        let posts = server.recorded_posts().await;
        assert!(
            posts.iter().any(|p| p.body.contains(&expected_b64)),
            "request body should carry the base64-encoded image bytes"
        );
        assert!(
            posts.iter().any(|p| p.body.contains("describe this image")),
            "request body should also carry the text prompt"
        );

        agent.shutdown().await.expect("shutdown");
    });
}
