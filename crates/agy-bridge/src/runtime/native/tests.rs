use super::{events::to_usage_metadata, *};
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

#[test]
fn test_build_harness_config_options_subagents_and_workspace_policy() {
    use crate::{
        config::{
            AgentBehavior, BuiltinTools, CompactionConfig, GeminiConfig, GenerationConfig,
            ModelConfig, ModelEntry, SubagentCapabilities, SubagentConfig, ThinkingLevel,
        },
        policies::{PolicyRule, PolicySet},
        types::ServiceTier,
    };

    let default_entry = ModelEntry {
        name: "gemini-3.5-pro".to_string(),
        api_key: Some("per-model-key".to_string()),
        generation: GenerationConfig {
            thinking_level: Some(ThinkingLevel::High),
            service_tier: Some(ServiceTier::Priority),
            max_output_tokens: None,
        },
    };
    let gemini = GeminiConfig {
        api_key: Some("fallback-key".to_string()),
        base_url: Some("http://127.0.0.1:8675".to_string()),
        models: ModelConfig {
            default: default_entry,
            image_generation: ModelEntry::default(),
        },
    };
    let subagent = SubagentConfig::builder()
        .name("helper")
        .description("Helper subagent")
        .capabilities(
            SubagentCapabilities::builder()
                .agent_behavior(AgentBehavior::Minimal)
                .enabled_tools(vec![BuiltinTools::ViewFile])
                .build(),
        )
        .build();
    let cfg = AgentConfig::builder()
        .model("gemini-3.5-pro")
        .gemini(gemini)
        .compaction_config(CompactionConfig::one_million())
        .subagents(vec![subagent])
        .build();

    let policies = PolicySet::validated_from(vec![PolicyRule::WorkspaceOnly(vec![
        std::path::PathBuf::from("/tmp"),
    ])])
    .unwrap();
    let harness_cfg = config::build_harness_config(&cfg, None, None, &policies);

    assert_eq!(harness_cfg.compaction_threshold, 950_000);
    let cc = harness_cfg.compaction_config.expect("compaction_config");
    assert_eq!(cc.token_threshold, 950_000);
    assert_eq!(cc.max_context_tokens, 1_000_000);

    let text_model = &harness_cfg.models[0];
    let Some(proto::localharness::model_config::Endpoint::GeminiApiEndpoint(ref ep)) =
        text_model.endpoint
    else {
        panic!("Expected GeminiApiEndpoint");
    };
    assert_eq!(ep.api_key, "per-model-key");
    let opts = ep.options.as_ref().expect("GeminiModelOptions");
    assert_eq!(opts.thinking_level, "high");
    assert_eq!(opts.service_tier, "priority");

    let custom_sub = &harness_cfg.custom_subagents[0];
    let sub_tools = custom_sub
        .harness_side_tools
        .as_ref()
        .expect("subagent harness_side_tools");
    assert!(sub_tools.view_file.as_ref().unwrap().enabled);
    assert!(!sub_tools.run_command.as_ref().unwrap().enabled);

    let pol_cfg = harness_cfg.policy_config.expect("policy_config");
    assert_eq!(
        pol_cfg.workspace_containment,
        proto::localharness::policy_config::WorkspaceContainment::Enabled as i32
    );
    let root_tools = harness_cfg
        .harness_side_tools
        .expect("root harness_side_tools");
    assert!(
        root_tools
            .permissions
            .expect("permissions")
            .enforce_workspace_validation
    );
}

#[test]
fn test_remove_persisted_conversation_files() {
    let dir = std::env::temp_dir().join(format!("agy_del_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_file = dir.join("conv-123.db");
    let traj_dir = dir.join("traj-conv-123");
    std::fs::write(&db_file, b"sqlite").unwrap();
    std::fs::create_dir_all(&traj_dir).unwrap();
    std::fs::write(traj_dir.join("step.json"), b"{}").unwrap();

    ws::remove_persisted_conversation_files(&dir, "conv-123");
    assert!(!db_file.exists());
    assert!(!traj_dir.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
