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
        genai::{VideoContent, video_content},
        google::protobuf::{Duration, NullValue},
        localharness::{
            AgentBehavior, ClientInfo, InputConfig, ModelConfig, ModelType, OutputConfig,
            OutputEvent, model_config,
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
    fn test_wire_round_trip_input_and_output_config() {
        let mut env = HashMap::new();
        env.insert("GEMINI_API_KEY".to_string(), "secret-key".to_string());
        let input = InputConfig {
            storage_directory: "/tmp/agy".to_string(),
            port: 9001,
            bind_address: "127.0.0.1".to_string(),
            client_info: Some(ClientInfo {
                language: "rust".to_string(),
                version: "0.16.6".to_string(),
                language_version: "1.95".to_string(),
                os: "linux".to_string(),
                os_version: "6.12".to_string(),
            }),
            env,
            use_interactions_api: true,
        };
        let mut bytes = Vec::new();
        input.encode(&mut bytes).expect("encode InputConfig");
        let decoded = InputConfig::decode(&bytes).expect("decode InputConfig");
        assert_eq!(decoded, input);

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
    fn test_protojson_duration_and_null_value_round_trip() {
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

        let nv = NullValue::NullValue;
        let nv_json = serde_json::to_string(&nv).expect("serialize NullValue");
        assert_eq!(nv_json, "null");
        let nv_parsed: NullValue = serde_json::from_str("null").expect("deserialize NullValue");
        assert_eq!(nv_parsed, NullValue::NullValue);
    }

    #[test]
    fn test_protojson_enums_oneofs_and_base64() {
        let model = ModelConfig {
            name: "gemini-3-flash".to_string(),
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
        assert!(json.contains("\"MODEL_TYPE_TEXT\""));
        assert!(json.contains("\"MODEL_TYPE_IMAGE\""));
        assert!(json.contains("\"vertexEndpoint\""));
        let round_trip: ModelConfig = serde_json::from_str(&json).expect("deserialize ModelConfig");
        assert_eq!(round_trip, model);

        let from_snake_and_int: ModelConfig = serde_json::from_str(
            r#"{"name":"m","types":[1,"MODEL_TYPE_IMAGE"],"vertex_endpoint":{"project":"p"}}"#,
        )
        .expect("deserialize snake_case and integer enum tags");
        assert_eq!(
            from_snake_and_int.types,
            vec![ModelType::Text as i32, ModelType::Image as i32]
        );

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
                        },
                    )),
                },
            )),
        };
        let video_json = serde_json::to_string(&video).expect("serialize VideoContent");
        assert!(video_json.contains("\"TYPE_MP4\""));
        assert!(video_json.contains("\"AQIDBA==\""));
        let video_rt: VideoContent =
            serde_json::from_str(&video_json).expect("deserialize VideoContent");
        assert_eq!(video_rt, video);

        assert_eq!(
            AgentBehavior::from_str_name("AGENT_BEHAVIOR_AUTONOMOUS"),
            Some(AgentBehavior::Autonomous)
        );
    }
}
