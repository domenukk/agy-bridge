use super::*;
use crate::content::{Audio, Content, Document, Image, Video};

#[tokio::test]
async fn test_native_runtime_creation_and_empty_counts() {
    let runtime = NativeRuntime::default();
    assert_eq!(runtime.active_agent_count().await.unwrap(), 0);
    assert!(runtime.shutdown_agent(999).await.is_ok());
}

#[test]
fn test_to_usage_metadata() {
    let proto_usage = proto::localharness::UsageMetadata {
        prompt_token_count: 100,
        candidates_token_count: 50,
        total_token_count: 150,
        cached_content_token_count: 20,
        thoughts_token_count: 10,
        ..Default::default()
    };
    let usage = to_usage_metadata(&proto_usage);
    assert_eq!(usage.prompt_token_count, Some(100));
    assert_eq!(usage.candidates_token_count, Some(50));
    assert_eq!(usage.total_token_count, Some(150));
    assert_eq!(usage.cached_content_token_count, Some(20));
    assert_eq!(usage.thoughts_token_count, Some(10));
}

#[test]
fn test_to_proto_user_input_multimodal() {
    use crate::content::ContentPrimitive;
    let content = Content::Multi {
        parts: vec![
            ContentPrimitive::Text {
                text: "Explain this content".to_string(),
            },
            ContentPrimitive::Image(Image::png(vec![1, 2, 3])),
            ContentPrimitive::Audio(Audio::mp3(vec![4, 5, 6])),
            ContentPrimitive::Document(Document::pdf(vec![7, 8, 9])),
            ContentPrimitive::Video(Video::mp4(vec![10, 11, 12])),
        ],
    };

    let proto_input = to_proto_user_input(&content);
    assert_eq!(proto_input.parts.len(), 5);
}
