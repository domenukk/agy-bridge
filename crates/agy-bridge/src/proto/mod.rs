//! Native `serde` + binary-handshake protocol bindings for the Google Antigravity SDK local harness.

pub mod genai;
pub mod localharness;
pub mod serde_helpers;
pub mod skills;
pub mod wire;
pub mod wkt;

/// Protobuf well-known types (`google.protobuf`).
pub mod google {
    /// Protobuf well-known types (`google.protobuf`).
    pub mod protobuf {
        pub use crate::proto::wkt::*;
    }
}

/// Antigravity local harness protocol namespace (`antigravity.localharness`).
pub mod antigravity {
    /// Local harness protocol messages (`antigravity.localharness`).
    pub mod localharness {
        pub use crate::proto::localharness::*;
    }
}

pub use genai::skills as skill;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        genai::{
            AgentToolCallStep, CodeExecutionCallStep, DocumentContent, ExperimentalToolCallStep,
            FinishCallStep, InvokeSubagentCall, ManageTaskCallStep, Struct, ToolCallStep,
            VideoContent, ViewFileResultStep, agent_tool_call_step, document_content,
            manage_task_call_step, tool_call_step, video_content,
        },
        google::protobuf::{Duration, NullValue},
        localharness::{
            AgentBehavior, AutoPolicyConfig, ClientInfo, InputConfig, ModelConfig, ModelType,
            OutputConfig, OutputEvent, StopResult, SubagentsConfig, ThinkingEffort, model_config,
            stop_result,
        },
    };

    #[test]
    fn test_deserialize_init_response() {
        let json = r#"{"initializeConversationResponse":{"cascadeId":"eb92a4f4a84fac0795955a687f1d2eae"},"seqNum":1,"timestampMicros":1786720474186732}"#;
        let event: OutputEvent = serde_json::from_str(json).expect("deserialize OutputEvent");
        assert_eq!(event.seq_num, 1);
        assert_eq!(event.timestamp_micros, 1_786_720_474_186_732);
    }

    #[test]
    fn test_wire_round_trip_input_and_output_config_and_rejections() {
        let mut env = HashMap::new();
        env.insert("GEMINI_API_KEY".to_string(), "secret-key".to_string());
        let input = InputConfig {
            storage_directory: "/tmp/agy".to_string(),
            port: 9001,
            bind_address: "127.0.0.1".to_string(),
            client_info: Some(ClientInfo {
                language: "rust".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                language_version: "1.95".to_string(),
                os: "linux".to_string(),
                os_version: "6.12".to_string(),
            }),
            env,
        };
        let mut bytes = Vec::new();
        input.encode(&mut bytes).expect("encode InputConfig");
        let decoded = InputConfig::decode(&bytes).expect("decode InputConfig");
        assert_eq!(decoded, input);

        // Field number 0 (tag = 0x00) must fail wire decoding.
        assert!(InputConfig::decode(&[0x00, 0x01]).is_err());
        // Overlong varint ([0x80, 0x00] for port field #2 -> tag 0x10) must fail wire decoding.
        assert!(InputConfig::decode(&[0x10, 0x80, 0x00]).is_err());

        let output = OutputConfig {
            port: 43210,
            api_key: "harness-token-xyz".to_string(),
        };
        let mut out_bytes = Vec::new();
        output.encode(&mut out_bytes).expect("encode OutputConfig");
        let decoded_out = OutputConfig::decode(&out_bytes).expect("decode OutputConfig");
        assert_eq!(decoded_out, output);
    }

    #[test]
    fn test_protojson_duration_and_null_value_round_trip_and_validation() {
        let millis_duration = Duration {
            seconds: 12,
            nanos: 340_000_000,
        };
        let millis_json =
            serde_json::to_string(&millis_duration).expect("serialize 3-digit Duration");
        assert_eq!(millis_json, "\"12.340s\"");
        let parsed_millis: Duration =
            serde_json::from_str(&millis_json).expect("deserialize Duration");
        assert_eq!(parsed_millis, millis_duration);
        let parsed_unpadded: Duration =
            serde_json::from_str("\"12.34s\"").expect("deserialize unpadded Duration");
        assert_eq!(parsed_unpadded, millis_duration);

        let micros_duration = Duration {
            seconds: 3,
            nanos: 1_000,
        };
        assert_eq!(
            serde_json::to_string(&micros_duration).expect("serialize 6-digit Duration"),
            "\"3.000001s\""
        );

        let nanos_duration = Duration {
            seconds: 3,
            nanos: 1,
        };
        assert_eq!(
            serde_json::to_string(&nanos_duration).expect("serialize 9-digit Duration"),
            "\"3.000000001s\""
        );

        let neg = Duration {
            seconds: -1,
            nanos: -500_000_000,
        };
        let neg_s = serde_json::to_string(&neg).expect("serialize negative Duration");
        assert_eq!(neg_s, "\"-1.500s\"");
        let neg_parsed: Duration =
            serde_json::from_str(&neg_s).expect("deserialize negative Duration");
        assert_eq!(neg_parsed, neg);

        // Opposite signs and out-of-range nanos/seconds must fail both serialize and deserialize.
        serde_json::to_string(&Duration {
            seconds: 1,
            nanos: -500,
        })
        .expect_err("mixed-sign Duration must fail serialization");
        serde_json::from_str::<Duration>(r#"{"seconds":1,"nanos":-500}"#)
            .expect_err("mixed-sign Duration must fail deserialization");
        serde_json::from_str::<Duration>(r#"{"seconds":0,"nanos":1000000000}"#)
            .expect_err("out-of-range nanos must fail deserialization");
        serde_json::from_str::<Duration>("\"-s\"")
            .expect_err("digitless duration string must fail deserialization");

        let nv = NullValue::NullValue;
        let nv_json = serde_json::to_string(&nv).expect("serialize NullValue");
        assert_eq!(nv_json, "null");
        let nv_parsed: NullValue = serde_json::from_str("null").expect("deserialize NullValue");
        assert_eq!(nv_parsed, NullValue::NullValue);
    }

    #[test]
    fn test_protojson_0124_media_and_localharness_roundtrip() {
        let model = ModelConfig {
            name: "gemini-4-hydrogen".to_string(),
            types: vec![ModelType::Text as i32, ModelType::Image as i32],
            endpoint: Some(model_config::Endpoint::Vertex(
                super::localharness::VertexEndpoint {
                    project: "demo-project".to_string(),
                    location: "us-central1".to_string(),
                    ..super::localharness::VertexEndpoint::default()
                },
            )),
        };
        let json = serde_json::to_string(&model).expect("serialize ModelConfig");
        let round_trip: ModelConfig = serde_json::from_str(&json).expect("deserialize ModelConfig");
        assert_eq!(round_trip, model);

        let video = VideoContent {
            mime_type: video_content::MimeType::TypeMp4 as i32,
            mime_type_string: String::new(),
            resolution: 0,
            name: "clip.mp4".to_string(),
            data_or_uri: Some(video_content::DataOrUri::Data(vec![1, 2, 3, 4])),
            processing_oneof: Some(video_content::ProcessingOneof::ProcessingConfig(
                video_content::MediaProcessing {
                    r#type: Some(video_content::media_processing::Type::Static(
                        video_content::StaticMediaProcessing {
                            start_offset: Some(Duration {
                                seconds: 2,
                                nanos: 0,
                            }),
                            end_offset: Some(Duration {
                                seconds: 10,
                                nanos: 250_000_000,
                            }),
                            fps: 29.97,
                            content_layers: vec![
                                video_content::static_media_processing::ContentLayer::Frames as i32,
                            ],
                        },
                    )),
                },
            )),
        };
        let video_json = serde_json::to_string(&video).expect("serialize VideoContent");
        assert!(video_json.contains("\"FRAMES\""));
        let video_rt: VideoContent =
            serde_json::from_str(&video_json).expect("deserialize VideoContent");
        assert_eq!(video_rt, video);
        serde_json::from_str::<video_content::StaticMediaProcessing>(r#"{"fps":"NaN"}"#)
            .expect_err("NaN float must be rejected");
        serde_json::to_string(&video_content::StaticMediaProcessing {
            fps: f64::INFINITY,
            ..video_content::StaticMediaProcessing::default()
        })
        .expect_err("Infinity float serialization must be rejected");

        let doc = DocumentContent {
            mime_type: document_content::MimeType::Xlsx as i32,
            processing_oneof: Some(document_content::ProcessingOneof::ProcessingConfig(
                document_content::MediaProcessing {
                    r#type: Some(document_content::media_processing::Type::Static(
                        document_content::StaticMediaProcessing {
                            format_options: Some(
                                document_content::static_media_processing::FormatOptions::SpreadsheetOptions(
                                    document_content::SpreadsheetProcessingOptions {
                                        visible_cells: "A1:D10".to_string(),
                                        sheet_index: 0,
                                        content_layers: vec![
                                            document_content::spreadsheet_processing_options::ContentLayer::Formulas as i32,
                                        ],
                                    },
                                ),
                            ),
                        },
                    )),
                },
            )),
            ..DocumentContent::default()
        };
        let doc_json = serde_json::to_string(&doc).expect("serialize DocumentContent");
        assert!(doc_json.contains("\"TYPE_XLSX\""));
        assert_eq!(
            serde_json::from_str::<DocumentContent>(&doc_json)
                .expect("deserialize DocumentContent"),
            doc
        );
    }

    #[test]
    fn test_protojson_0124_localharness_enums_and_configs() {
        let sub_cfg = SubagentsConfig {
            enabled: true,
            disable_manage_subagents_tool: true,
            ..SubagentsConfig::default()
        };
        let sub_json = serde_json::to_string(&sub_cfg).expect("serialize SubagentsConfig");
        assert!(sub_json.contains("\"disableManageSubagentsTool\":true"));
        assert_eq!(
            ThinkingEffort::from_str_name("THINKING_EFFORT_MAX"),
            Some(ThinkingEffort::Max)
        );

        let stop_res = StopResult {
            decision: stop_result::Decision::Continue as i32,
            reason: "keep going".to_string(),
        };
        let stop_json = serde_json::to_string(&stop_res).expect("serialize StopResult");
        assert!(stop_json.contains("\"decision\":\"CONTINUE\""));

        let auto_cfg = AutoPolicyConfig {
            enabled: true,
            model: "gemini-4-hydrogen".to_string(),
        };
        assert_eq!(
            serde_json::from_str::<AutoPolicyConfig>(
                &serde_json::to_string(&auto_cfg).expect("serialize AutoPolicyConfig")
            )
            .expect("deserialize AutoPolicyConfig"),
            auto_cfg
        );
        assert_eq!(
            AgentBehavior::from_str_name("AGENT_BEHAVIOR_AUTONOMOUS"),
            Some(AgentBehavior::Autonomous)
        );
    }

    #[test]
    fn test_protojson_0124_steps_and_agent_tools_roundtrip() {
        let step_calls = [
            ToolCallStep {
                id: "c1".to_string(),
                r#type: Some(tool_call_step::Type::ManageTaskCall(ManageTaskCallStep {
                    action: manage_task_call_step::Action::Status as i32,
                    call_id: "task-1".to_string(),
                    input: String::new(),
                })),
                ..ToolCallStep::default()
            },
            ToolCallStep {
                id: "c2".to_string(),
                r#type: Some(tool_call_step::Type::FinishCall(FinishCallStep {
                    arguments: Struct::from_json_object(
                        serde_json::json!({"ok": true}).as_object().expect("obj"),
                    ),
                })),
                ..ToolCallStep::default()
            },
            ToolCallStep {
                id: "c3".to_string(),
                r#type: Some(tool_call_step::Type::AgentToolCall(AgentToolCallStep {
                    call: Some(agent_tool_call_step::Call::InvokeSubagent(
                        InvokeSubagentCall {
                            type_name: "research".to_string(),
                            role: "Helper".to_string(),
                            prompt: Vec::new(),
                        },
                    )),
                })),
                ..ToolCallStep::default()
            },
            ToolCallStep {
                id: "c4".to_string(),
                r#type: Some(tool_call_step::Type::ExperimentalToolCall(
                    ExperimentalToolCallStep {
                        name: "exp".to_string(),
                        internal_data: vec![0xAA, 0xBB],
                    },
                )),
                ..ToolCallStep::default()
            },
            ToolCallStep {
                id: "c5".to_string(),
                r#type: Some(tool_call_step::Type::CodeExecutionCall(
                    CodeExecutionCallStep {
                        arguments: None,
                        working_directory: "/tmp/ws".to_string(),
                    },
                )),
                ..ToolCallStep::default()
            },
        ];
        for tc in &step_calls {
            let s = serde_json::to_string(tc).expect("serialize ToolCallStep");
            let rt: ToolCallStep = serde_json::from_str(&s).expect("deserialize ToolCallStep");
            assert_eq!(&rt, tc);
        }

        let vf_res = ViewFileResultStep {
            error_message: String::new(),
            start_line: 1,
            end_line: 50,
            total_lines: 120,
        };
        assert_eq!(
            serde_json::from_str::<ViewFileResultStep>(
                &serde_json::to_string(&vf_res).expect("serialize ViewFileResultStep")
            )
            .expect("deserialize ViewFileResultStep"),
            vf_res
        );

        assert_eq!(
            AgentBehavior::from_str_name("AGENT_BEHAVIOR_AUTONOMOUS"),
            Some(AgentBehavior::Autonomous)
        );
    }
}
