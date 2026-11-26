//! Comprehensive `google-antigravity 0.1.24` wire conformance, backend config,
//! multimodal MIME, `OnInteraction`, `StopReason`, and turn-lifecycle parity tests.

use agy_bridge::{
    BuiltinTools, CapabilitiesConfig, LiteRTBackend, LiteRTConfig, LocalOpenAIConfig, StopReason,
    SystemInstructionSection, SystemInstructions, ThinkingLevel, VertexConfig,
    config::{AgentConfig, DEFAULT_MODEL, GeminiConfig},
    content::{self, ContentPrimitive, Document, mime},
    error::Error,
};

#[test]
fn sdk_0124_presets_and_validation_work() {
    assert_eq!(DEFAULT_MODEL, "gemini-4-hydrogen");
    assert_eq!(ThinkingLevel::ExtraHigh.as_str(), "xhigh");
    assert_eq!(
        BuiltinTools::deprecated(),
        &[
            BuiltinTools::ListDir,
            BuiltinTools::SearchDir,
            BuiltinTools::FindFile
        ]
    );
    assert!(!BuiltinTools::default_tools().contains(&BuiltinTools::AskQuestion));
    assert!(!BuiltinTools::default_tools().contains(&BuiltinTools::ListDir));
    assert_eq!(
        BuiltinTools::read_only(),
        &[
            BuiltinTools::ViewFile,
            BuiltinTools::ReadUrlContent,
            BuiltinTools::Schedule,
            BuiltinTools::Finish
        ]
    );

    assert_eq!(
        "STOP_REASON_QUOTA_EXHAUSTED"
            .parse::<StopReason>()
            .expect("prefixed"),
        StopReason::QuotaExhausted
    );
    assert_eq!(
        StopReason::from_proto_i32(1),
        StopReason::MaxModelCallsExceeded
    );

    let si = SystemInstructions::templated(
        Some("Custom persona"),
        [SystemInstructionSection {
            title: "rules".into(),
            content: "Stay concise.".into(),
        }],
    );
    assert!(matches!(si, SystemInstructions::Templated { .. }));

    let both_skills = AgentConfig::builder()
        .skills(["/tmp/skills"])
        .inline_skills([agy_bridge::InlineSkill::builder()
            .name("s1")
            .description("d")
            .content("c")
            .build()])
        .build();
    assert!(matches!(
        agy_bridge::agent::validate_create_config(&both_skills),
        Err(Error::InvalidConfig { .. })
    ));

    let rel_app_dir = AgentConfig::builder()
        .app_data_dir("relative/app/data")
        .build();
    assert!(matches!(
        agy_bridge::agent::validate_create_config(&rel_app_dir),
        Err(Error::InvalidConfig { .. })
    ));

    let bad_sub = AgentConfig::builder()
        .capabilities(
            CapabilitiesConfig::builder()
                .allowed_subagents(vec!["ghost_subagent".into()])
                .build(),
        )
        .build();
    assert!(matches!(
        agy_bridge::agent::validate_create_config(&bad_sub),
        Err(Error::InvalidConfig { .. })
    ));

    let oai_cfg = LocalOpenAIConfig::new("gemma-3-27b", "http://127.0.0.1:11434/v1");
    assert!(matches!(oai_cfg.validate(), Ok(())));
    let v_cfg = VertexConfig::builder()
        .project("proj-1")
        .location("us-central1")
        .build();
    assert!(matches!(
        v_cfg.validate(&std::collections::HashMap::new()),
        Ok(())
    ));
    let litert_cfg = LiteRTConfig::builder()
        .model_path("/tmp/model.litertlm")
        .backend(LiteRTBackend::Gpu)
        .build();
    litert_cfg.validate().expect("valid LiteRTConfig");
}

#[test]
fn sdk_0124_office_mimes_and_base_url_precedence_work() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (filename, expected_mime) in [
        ("deck.pptx", mime::APPLICATION_PPTX),
        ("legacy.ppt", mime::APPLICATION_PPT),
        ("slides.odp", mime::APPLICATION_ODP),
        ("spec.docx", mime::APPLICATION_DOCX),
        ("memo.doc", mime::APPLICATION_DOC),
        ("tpl.dotx", mime::APPLICATION_DOTX),
        ("notes.odt", mime::APPLICATION_ODT),
        ("sheet.xlsx", mime::APPLICATION_XLSX),
        ("old.xls", mime::APPLICATION_XLS),
        ("calc.ods", mime::APPLICATION_ODS),
    ] {
        let path = dir.path().join(filename);
        std::fs::write(&path, b"office-bytes").expect("write office file");
        let doc = Document::from_file(&path).expect("Document::from_file");
        assert_eq!(doc.mime_type, expected_mime);
        let prim = content::from_file(&path).expect("content::from_file");
        assert!(matches!(prim, ContentPrimitive::Document(_)));
    }
    for audio_mime in [
        mime::AUDIO_MP3,
        mime::AUDIO_MP4,
        mime::AUDIO_WEBM,
        mime::AUDIO_L16,
        mime::AUDIO_X_WAV,
    ] {
        let prim = content::from_bytes(vec![1, 2, 3, 4], audio_mime).expect("audio from_bytes");
        assert!(matches!(prim, ContentPrimitive::Audio(_)));
    }

    let mut env_cfg = AgentConfig::builder().build();
    env_cfg.env.insert(
        "GOOGLE_GEMINI_BASE_URL".into(),
        "http://127.0.0.1:29001".into(),
    );
    assert_eq!(
        env_cfg.effective_base_url().as_deref(),
        Some("http://127.0.0.1:29001")
    );
    env_cfg.gemini = Some(GeminiConfig {
        base_url: Some("http://127.0.0.1:29002".into()),
        ..Default::default()
    });
    assert_eq!(
        env_cfg.effective_base_url().as_deref(),
        Some("http://127.0.0.1:29002")
    );
}

/// Canonical `google.protobuf.json_format.MessageToJson(...)` payloads emitted
/// by `google-antigravity==0.1.24` (`localharness_pb2`, `content_pb2`,
/// `steps_pb2`, and `agent_tools_pb2`).
///
/// Cross-checked against the live installed protobuf definitions in
/// `crates/agy-bridge/tests/python/test_v0168_sdk0124_conformance.py` (and in
/// `protojson_0124_live_pb2_roundtrip` when both `native` and `python` are
/// enabled) so the pure-Rust `native` test suite runs cleanly on any OS with
/// zero Python / `.venv` dependency.
#[cfg(feature = "native")]
const SDK_0124_TSU_JSON: &str = r#"{"trajectoryId":"root-1","state":"STATE_FULLY_IDLE","error":"Rate limited","stopReason":"STOP_REASON_QUOTA_EXHAUSTED","errorCode":"RESOURCE_EXHAUSTED"}"#;
#[cfg(feature = "native")]
const SDK_0124_STOP_ARGS_JSON: &str = r#"{"responseText":"done","trajectoryId":"root-1","continuationCount":2,"stopReason":"STOP_REASON_MAX_MODEL_CALLS_EXCEEDED"}"#;
#[cfg(feature = "native")]
const SDK_0124_SKILL_LOOKUP_JSON: &str = r#"{"operation":"OPERATION_LOOKUP_SKILLS","requestedSkillNames":["sec"],"resolvedSkillNames":["sec"]}"#;
#[cfg(feature = "native")]
const SDK_0124_DOC_CONTENT_JSON: &str = r#"{"legacy_mime_type_enum":"TYPE_XLSX","processingConfig":{"static":{"spreadsheetOptions":{"visibleCells":"A1:B5","sheetIndex":1}}}}"#;
#[cfg(feature = "native")]
const SDK_0124_VIEW_FILE_JSON: &str = r#"{"startLine":10,"endLine":20,"totalLines":100}"#;
#[cfg(feature = "native")]
const SDK_0124_MANAGE_TASK_JSON: &str = r#"{"action":"ACTION_STATUS","callId":"task-9"}"#;
#[cfg(feature = "native")]
const SDK_0124_AGENT_TOOL_CALL_JSON: &str =
    r#"{"invokeSubagent":{"typeName":"research","role":"Auditor"}}"#;
#[cfg(feature = "native")]
const SDK_0124_FINISH_CALL_JSON: &str = r#"{"arguments":{"fields":[{"name":"ok","value":{"numberValue":1.0}},{"name":"ratio","value":{"numberValue":1.5}}]}}"#;
#[cfg(feature = "native")]
const SDK_0124_VERTEX_ENDPOINT_JSON: &str = r#"{"baseUrl":"https://us-central1-aiplatform.googleapis.com","httpHeaders":{"X-Vertex":"1"},"project":"proj-1","location":"us-central1"}"#;

#[cfg(feature = "native")]
#[test]
fn protojson_0124_cross_language_pb2_wire_conformance() {
    use agy_bridge::proto;

    let tsu: proto::localharness::TrajectoryStateUpdate =
        serde_json::from_str(SDK_0124_TSU_JSON).expect("parse tsu");
    assert_eq!(tsu.error_code, "RESOURCE_EXHAUSTED");
    assert_eq!(
        StopReason::from_proto_i32(tsu.stop_reason),
        StopReason::QuotaExhausted
    );

    let sa: proto::localharness::StopArgs =
        serde_json::from_str(SDK_0124_STOP_ARGS_JSON).expect("parse StopArgs");
    assert_eq!(
        StopReason::from_proto_i32(sa.stop_reason),
        StopReason::MaxModelCallsExceeded
    );
    let sa_int: proto::localharness::StopArgs =
        serde_json::from_str(r#"{"stopReason": 1}"#).expect("parse int StopArgs");
    assert_eq!(
        StopReason::from_proto_i32(sa_int.stop_reason),
        StopReason::MaxModelCallsExceeded
    );

    let sl: proto::localharness::ActionSkillLookup =
        serde_json::from_str(SDK_0124_SKILL_LOOKUP_JSON).expect("parse ActionSkillLookup");
    assert_eq!(sl.requested_skill_names, vec!["sec"]);

    let _doc: proto::genai::DocumentContent =
        serde_json::from_str(SDK_0124_DOC_CONTENT_JSON).expect("parse DocumentContent");
    let vf: proto::genai::ViewFileResultStep =
        serde_json::from_str(SDK_0124_VIEW_FILE_JSON).expect("parse ViewFileResultStep");
    assert_eq!((vf.start_line, vf.end_line, vf.total_lines), (10, 20, 100));
    let _mt: proto::genai::ManageTaskCallStep =
        serde_json::from_str(SDK_0124_MANAGE_TASK_JSON).expect("parse ManageTaskCallStep");
    let _atc: proto::genai::AgentToolCallStep =
        serde_json::from_str(SDK_0124_AGENT_TOOL_CALL_JSON).expect("parse AgentToolCallStep");
    let fc: proto::genai::FinishCallStep =
        serde_json::from_str(SDK_0124_FINISH_CALL_JSON).expect("parse FinishCallStep");
    assert_eq!(
        fc.arguments
            .as_ref()
            .map(proto::genai::Struct::to_json_value),
        Some(serde_json::json!({"ok": 1, "ratio": 1.5}))
    );
    let ve: proto::localharness::VertexEndpoint =
        serde_json::from_str(SDK_0124_VERTEX_ENDPOINT_JSON).expect("parse VertexEndpoint");
    assert_eq!(
        (ve.project.as_str(), ve.location.as_str()),
        ("proj-1", "us-central1")
    );
    assert_eq!(
        ve.http_headers.get("X-Vertex").map(String::as_str),
        Some("1")
    );

    // Re-serialize back to ProtoJSON and confirm round-trip preserves fields and camelCase keys.
    let trajectory_round: proto::localharness::TrajectoryStateUpdate =
        serde_json::from_str(&serde_json::to_string(&tsu).expect("serialize tsu"))
            .expect("round-trip tsu");
    assert_eq!(trajectory_round.error_code, "RESOURCE_EXHAUSTED");
    assert_eq!(trajectory_round.stop_reason, tsu.stop_reason);
    let stop_args_round: proto::localharness::StopArgs =
        serde_json::from_str(&serde_json::to_string(&sa).expect("serialize sa"))
            .expect("round-trip sa");
    assert_eq!(stop_args_round.stop_reason, sa.stop_reason);
    assert_eq!(stop_args_round.continuation_count, 2);
    let skill_lookup_round: proto::localharness::ActionSkillLookup =
        serde_json::from_str(&serde_json::to_string(&sl).expect("serialize sl"))
            .expect("round-trip sl");
    assert_eq!(skill_lookup_round.requested_skill_names, vec!["sec"]);
}

#[cfg(all(feature = "native", feature = "python"))]
fn locate_python_venv_site_packages(py: pyo3::Python<'_>) -> Option<std::path::PathBuf> {
    use pyo3::types::PyAnyMethods;

    let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let venv = manifest_path.ancestors().find_map(|dir| {
        let candidate = dir.join(".venv");
        candidate.is_dir().then_some(candidate)
    })?;
    let sys = py.import("sys").expect("import sys");
    let vi = sys.getattr("version_info").expect("version_info");
    let major: u32 = vi.getattr("major").expect("major").extract().expect("u32");
    let minor: u32 = vi.getattr("minor").expect("minor").extract().expect("u32");
    let unix_sp = venv
        .join("lib")
        .join(format!("python{major}.{minor}"))
        .join("site-packages");
    if unix_sp.is_dir() {
        return Some(unix_sp);
    }
    let win_sp = venv.join("Lib").join("site-packages");
    win_sp.is_dir().then_some(win_sp)
}

#[cfg(all(feature = "native", feature = "python"))]
#[test]
fn protojson_0124_live_pb2_roundtrip() {
    use agy_bridge::proto;
    use pyo3::types::PyAnyMethods;

    let tsu: proto::localharness::TrajectoryStateUpdate =
        serde_json::from_str(SDK_0124_TSU_JSON).expect("parse tsu");
    let sa: proto::localharness::StopArgs =
        serde_json::from_str(SDK_0124_STOP_ARGS_JSON).expect("parse StopArgs");
    let sl: proto::localharness::ActionSkillLookup =
        serde_json::from_str(SDK_0124_SKILL_LOOKUP_JSON).expect("parse ActionSkillLookup");
    let trajectory_json = serde_json::to_string(&tsu).expect("serialize tsu");
    let stop_args_json = serde_json::to_string(&sa).expect("serialize sa");
    let skill_lookup_json = serde_json::to_string(&sl).expect("serialize sl");

    pyo3::Python::attach(|py| {
        if let Some(sp) = locate_python_venv_site_packages(py) {
            let site = py.import("site").expect("import site");
            site.call_method1("addsitedir", (sp.to_string_lossy().as_ref(),))
                .expect("addsitedir");
        }
        let json_format = py
            .import("google.protobuf.json_format")
            .expect("import google.protobuf.json_format");
        let pb2 = py
            .import("google.antigravity.proto.localharness_pb2")
            .expect("import localharness_pb2");
        let trajectory_msg = pb2
            .getattr("TrajectoryStateUpdate")
            .expect("TrajectoryStateUpdate")
            .call0()
            .expect("construct TrajectoryStateUpdate");
        json_format
            .call_method1("Parse", (trajectory_json.as_str(), trajectory_msg))
            .expect("Parse TrajectoryStateUpdate");
        let stop_args_msg = pb2
            .getattr("StopArgs")
            .expect("StopArgs")
            .call0()
            .expect("construct StopArgs");
        json_format
            .call_method1("Parse", (stop_args_json.as_str(), stop_args_msg))
            .expect("Parse StopArgs");
        let skill_lookup_msg = pb2
            .getattr("ActionSkillLookup")
            .expect("ActionSkillLookup")
            .call0()
            .expect("construct ActionSkillLookup");
        json_format
            .call_method1("Parse", (skill_lookup_json.as_str(), skill_lookup_msg))
            .expect("Parse ActionSkillLookup");
    });
}

#[cfg(feature = "native")]
mod native_parity_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use agy_bridge::{
        SubagentCapabilities, SubagentConfig,
        content::Image,
        hooks::{Hooks, QuestionHookResult, QuestionResponse},
        policies::PolicyRule,
        proto,
    };
    use futures::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    use super::*;

    type ServerWs = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    fn write_mock_harness(port: u16) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("mock_dir");
        #[cfg(windows)]
        let bin_name = "mock_localharness.exe";
        #[cfg(not(windows))]
        let bin_name = "mock_localharness";
        let bin_path = dir.path().join(bin_name);
        let mut out_bytes = Vec::new();
        proto::localharness::OutputConfig {
            port: i32::from(port),
            api_key: "mock-key".into(),
        }
        .encode(&mut out_bytes)
        .expect("encode");
        let mut frame = u32::try_from(out_bytes.len())
            .expect("len")
            .to_le_bytes()
            .to_vec();
        frame.extend_from_slice(&out_bytes);
        std::fs::write(dir.path().join("payload.dat"), &frame).expect("write payload");
        std::fs::copy(env!("AGY_BRIDGE_MOCK_HARNESS_BIN"), &bin_path).expect("copy bin");
        (dir, bin_path)
    }

    async fn send_ev(ws: &mut ServerWs, event: proto::localharness::output_event::Event) {
        let out = proto::localharness::OutputEvent {
            event: Some(event),
            ..Default::default()
        };
        ws.send(Message::Text(
            serde_json::to_string(&out).expect("json").into(),
        ))
        .await
        .expect("send ws");
    }

    async fn verify_init_and_send_resume(ws: &mut ServerWs) {
        let init_msg = ws.next().await.expect("init").expect("ok");
        let init_ev: proto::localharness::InitializeConversationEvent =
            serde_json::from_str(init_msg.to_text().expect("txt")).expect("parse init");
        let hc = init_ev.config.expect("hc");
        assert!(
            hc.harness_side_tools
                .as_ref()
                .expect("hst")
                .subagents
                .as_ref()
                .expect("sub")
                .disable_manage_subagents_tool
        );
        assert!(
            !hc.custom_subagents[0]
                .harness_side_tools
                .as_ref()
                .expect("sub hst")
                .subagents
                .as_ref()
                .expect("sub sub")
                .enabled
        );
        assert!(
            hc.custom_subagents[1]
                .harness_side_tools
                .as_ref()
                .expect("sub2 hst")
                .subagents
                .as_ref()
                .expect("sub2 sub")
                .disable_manage_subagents_tool
        );

        send_ev(
            ws,
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id: "root-t".into(),
                    history: vec![proto::localharness::StepUpdate {
                        trajectory_id: "root-t".into(),
                        step_index: 0,
                        state: 2,
                        finish: Some(proto::localharness::ActionFinish {
                            output_string: r#"{"restored":true}"#.into(),
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ),
        )
        .await;
    }

    async fn run_turn1_on_mock_ws(ws: &mut ServerWs) {
        let u1_raw = ws.next().await.expect("u1").expect("ok");
        let u1: proto::localharness::InputEvent =
            serde_json::from_str(u1_raw.to_text().expect("txt")).expect("parse u1");
        if let Some(proto::localharness::input_event::Event::UserInput(ui)) = u1.event {
            assert_eq!(
                ui.parts[0].part,
                Some(proto::localharness::user_input::part::Part::Text(
                    "hello clean".into()
                ))
            );
            if let Some(proto::localharness::user_input::part::Part::Media(m)) = &ui.parts[1].part {
                assert_eq!(m.description, "chart-desc");
            } else {
                panic!("expected media part");
            }
        }

        for ev in [
            proto::localharness::StepUpdate {
                trajectory_id: "sub-t".into(),
                parent_trajectory_id: "root-t".into(),
                step_index: 9,
                state: 2,
                compaction: Some(proto::localharness::ActionCompaction {}),
                finish: Some(proto::localharness::ActionFinish {
                    output_string: r#"{"sub_leak":true}"#.into(),
                }),
                ..Default::default()
            },
            proto::localharness::StepUpdate {
                trajectory_id: "root-t".into(),
                step_index: 3,
                state: 1,
                compaction: Some(proto::localharness::ActionCompaction {}),
                ..Default::default()
            },
            proto::localharness::StepUpdate {
                trajectory_id: "root-t".into(),
                step_index: 3,
                state: 2,
                compaction: Some(proto::localharness::ActionCompaction {}),
                ..Default::default()
            },
            proto::localharness::StepUpdate {
                trajectory_id: "root-t".into(),
                step_index: 4,
                state: 3,
                questions_request: Some(proto::localharness::UserQuestionsRequest {
                    questions: vec![proto::localharness::UserQuestion {
                        question_type: Some(
                            proto::localharness::user_question::QuestionType::MultipleChoice(
                                proto::localharness::MultipleChoice {
                                    question: "Proceed?".into(),
                                    choices: vec!["No".into(), "Yes".into()],
                                    is_multi_select: false,
                                },
                            ),
                        ),
                    }],
                }),
                ..Default::default()
            },
        ] {
            send_ev(
                ws,
                proto::localharness::output_event::Event::StepUpdate(Box::new(ev)),
            )
            .await;
        }

        verify_question_response_and_finish_turn1(ws).await;
    }

    async fn verify_question_response_and_finish_turn1(ws: &mut ServerWs) {
        let q_msg = ws.next().await.expect("q_resp").expect("ok");
        let q_in: proto::localharness::InputEvent =
            serde_json::from_str(q_msg.to_text().expect("txt")).expect("parse q_in");
        match q_in.event {
            Some(proto::localharness::input_event::Event::QuestionResponse(qr)) => {
                assert_eq!(qr.step_index, 4);
                match qr.result {
                    Some(proto::localharness::user_questions_response::Result::Response(r)) => {
                        assert_eq!(
                            r.answers[0].answer,
                            Some(
                                proto::localharness::user_question_answer::Answer::MultipleChoiceAnswer(
                                    proto::localharness::MultipleChoiceAnswer {
                                        selected_choice_indices: vec![1],
                                        freeform_response: String::new(),
                                    }
                                )
                            )
                        );
                    }
                    other => panic!("expected QuestionsResponse, got {other:?}"),
                }
            }
            other => panic!("expected QuestionResponse, got {other:?}"),
        }

        send_ev(
            ws,
            proto::localharness::output_event::Event::StepUpdate(Box::new(
                proto::localharness::StepUpdate {
                    trajectory_id: "root-t".into(),
                    step_index: 5,
                    state: 2,
                    source: 3,
                    target: 1,
                    text_delta: "Turn1 ok".into(),
                    text: "Turn1 ok".into(),
                    finish: Some(proto::localharness::ActionFinish {
                        output_string: r#"{"turn":1}"#.into(),
                    }),
                    ..Default::default()
                },
            )),
        )
        .await;
        send_ev(
            ws,
            proto::localharness::output_event::Event::TrajectoryStateUpdate(
                proto::localharness::TrajectoryStateUpdate {
                    trajectory_id: "root-t".into(),
                    state: 2,
                    stop_reason: 1,
                    ..Default::default()
                },
            ),
        )
        .await;
    }

    async fn run_multiplexed_lifecycle_mock(listener: TcpListener) {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut ws = accept_async(stream).await.expect("ws");
        verify_init_and_send_resume(&mut ws).await;
        run_turn1_on_mock_ws(&mut ws).await;

        let _u2 = ws.next().await.expect("u2").expect("ok");
        send_ev(
            &mut ws,
            proto::localharness::output_event::Event::StepUpdate(Box::new(
                proto::localharness::StepUpdate {
                    trajectory_id: "root-t".into(),
                    step_index: 6,
                    state: 2,
                    finish: Some(proto::localharness::ActionFinish {
                        output_string: r#"{"poisoned":true}"#.into(),
                    }),
                    ..Default::default()
                },
            )),
        )
        .await;
        ws.send(Message::Text(
            r#"{"trajectoryStateUpdate":{"trajectoryId":"root-t","state":"STATE_FULLY_IDLE","error":"quota hit","errorCode":"RESOURCE_EXHAUSTED","stopReason":"STOP_REASON_QUOTA_EXHAUSTED"}}"#.into(),
        ))
        .await
        .expect("send errored state");
    }

    #[tokio::test]
    async fn native_litert_returns_unsupported_feature_error() {
        let bridge_lrt = agy_bridge::AgyBridge::native_builder()
            .build_native()
            .expect("build");
        let lrt_err = match bridge_lrt
            .default_agent()
            .litert(
                LiteRTConfig::builder()
                    .model_path("/tmp/model.litertlm")
                    .backend(LiteRTBackend::Gpu)
                    .build(),
            )
            .await
        {
            Ok(_unexpected) => panic!("LiteRT must fail with UnsupportedFeature on NativeRuntime"),
            Err(e) => e,
        };
        assert!(matches!(lrt_err, Error::UnsupportedFeature { .. }));
    }

    #[tokio::test]
    async fn native_multiplexed_lifecycle_stop_reason_and_interaction() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(run_multiplexed_lifecycle_mock(listener));
        let (_dir, bin_path) = write_mock_harness(port);

        let bridge = agy_bridge::AgyBridge::native_builder()
            .harness_path(&bin_path)
            .inter_agent_delay(std::time::Duration::ZERO)
            .build_native()
            .expect("build");

        let start_fires = Arc::new(AtomicUsize::new(0));
        let end_fires = Arc::new(AtomicUsize::new(0));
        let sf = Arc::clone(&start_fires);
        let ef = Arc::clone(&end_fires);
        let mut hooks = Hooks::new()
            .with_interaction("answer_q", |ctx| {
                assert_eq!(ctx.questions.len(), 1);
                assert_eq!(ctx.questions[0].options[1].id, "2");
                QuestionHookResult::answered([QuestionResponse::selected(["2"])])
            })
            .with_pre_turn("allow", |_ctx| agy_bridge::HookResult::allow());
        hooks.on_session_start("ss", move |_ctx| {
            sf.fetch_add(1, Ordering::SeqCst);
        });
        hooks.on_session_end("se", move |_ctx| {
            ef.fetch_add(1, Ordering::SeqCst);
        });

        let cfg = AgentConfig::builder()
            .subagents([
                SubagentConfig::builder()
                    .name("reader")
                    .description("Default read-only subagent")
                    .build(),
                SubagentConfig::builder()
                    .name("manager")
                    .description("Managed subagent")
                    .capabilities(
                        SubagentCapabilities::builder()
                            .enabled_tools(vec![BuiltinTools::ViewFile])
                            .disable_manage_subagents_tool(true)
                            .build(),
                    )
                    .build(),
            ])
            .build();

        let agent = bridge
            .agent(cfg)
            .disable_manage_subagents_tool(true)
            .hooks(hooks)
            .policies([PolicyRule::AllowAll])
            .await
            .expect("create agent");

        assert_eq!(start_fires.load(Ordering::SeqCst), 1);
        assert_eq!(
            agent.get_last_structured_output(),
            Some(serde_json::json!({"restored": true}))
        );

        let parts = vec![
            ContentPrimitive::Text {
                text: "hello\0\x07 clean".into(),
            },
            ContentPrimitive::Image(
                Image::png(vec![0x89, 0x50, 0x4E, 0x47]).with_description("chart-desc"),
            ),
        ];
        let res1 = agent
            .chat(parts)
            .await
            .expect("chat 1")
            .text()
            .await
            .expect("text 1");
        assert_eq!(res1.text(), "Turn1 ok");
        assert_eq!(res1.stop_reason(), StopReason::MaxModelCallsExceeded);
        assert_eq!(agent.last_stop_reason(), StopReason::MaxModelCallsExceeded);
        assert_eq!(
            res1.structured_output(),
            Some(&serde_json::json!({"turn": 1}))
        );
        assert_eq!(agent.compaction_indices().await.expect("comp"), vec![3]);

        let h2 = agent.chat("turn 2").await.expect("chat 2");
        let err2 = h2.text().await.expect_err("turn 2 must fail");
        assert!(err2.to_string().contains("quota hit"));
        assert_eq!(agent.get_last_structured_output(), None);
        assert_eq!(agent.last_stop_reason(), StopReason::QuotaExhausted);

        agent.clear_history().await.expect("clear_history");
        assert_eq!(agent.last_stop_reason(), StopReason::Unspecified);
        assert_eq!(agent.get_last_structured_output(), None);

        agent.shutdown().await.expect("shutdown");
        assert_eq!(end_fires.load(Ordering::SeqCst), 1);
        server.await.expect("server");
    }
}

#[cfg(any(feature = "native", feature = "python"))]
#[agy_bridge::tools::llm_tool(description = "Returns clean content plus host-only secret metadata")]
async fn metadata_probe_tool(
    /// Input tag.
    tag: String,
) -> Result<agy_bridge::tools::ToolOutput, agy_bridge::tools::ToolError> {
    agy_bridge::tools::ToolOutput::new(format!("public:{tag}"))
        .with_metadata(&serde_json::json!({"secret_host_meta": "DO_NOT_LEAK_991"}))
}

#[cfg(any(feature = "native", feature = "python"))]
async fn verify_live_0124_default_model_xhigh_and_metadata_isolation<
    R: agy_bridge::agent::Runtime,
>(
    bridge: &agy_bridge::AgyBridge<R>,
) {
    use std::sync::{Arc, Mutex};

    use agy_bridge::{
        config::{GenerationConfig, ModelConfig, ModelEntry},
        hooks::{Hooks, PostToolCallContext},
        policies::PolicyRule,
        tools::ToolRegistry,
    };
    use agy_bridge_test_support::{MockGeminiServer, MockResponse};

    let server = MockGeminiServer::start(vec![
        MockResponse::FunctionCall {
            name: "metadata_probe_tool".into(),
            args: serde_json::json!({
                "tag": "ok",
                "toolSummary": "Probe meta",
                "toolAction": "Probing meta"
            }),
        },
        MockResponse::Text("Verified 0.1.24.".into()),
    ])
    .await;

    let captured_meta: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let meta_clone = Arc::clone(&captured_meta);
    let hooks =
        Hooks::new().with_post_tool_call("capture_meta", move |ctx: &PostToolCallContext| {
            meta_clone.lock().expect("lock").push(ctx.metadata.clone());
        });

    let mut reg = ToolRegistry::new();
    reg.register(MetadataProbeTool);
    let save_dir = tempfile::tempdir().expect("save_dir");

    // Configure via config.env["GOOGLE_GEMINI_BASE_URL"] AND ThinkingLevel::ExtraHigh ("xhigh")
    // while keeping DEFAULT_MODEL ("gemini-4-hydrogen")!
    let mut cfg = AgentConfig::builder()
        .save_dir(save_dir.path())
        .api_key("test-key")
        .gemini(GeminiConfig {
            api_key: Some("test-key".into()),
            base_url: None,
            http_headers: [("X-Suite".to_string(), "v0168".to_string())]
                .into_iter()
                .collect(),
            models: ModelConfig {
                default: ModelEntry {
                    name: DEFAULT_MODEL.to_string(),
                    api_key: None,
                    generation: GenerationConfig {
                        thinking_level: Some(ThinkingLevel::ExtraHigh),
                        service_tier: None,
                        max_output_tokens: None,
                    },
                },
                image_generation: ModelEntry::default(),
            },
        })
        .capabilities(CapabilitiesConfig::custom_tools_only())
        .policies([PolicyRule::AllowAll])
        .build();
    cfg.env
        .insert("GOOGLE_GEMINI_BASE_URL".into(), server.base_url());

    let agent = bridge
        .agent(cfg)
        .tools(reg)
        .hooks(hooks)
        .await
        .expect("create agent");

    let reply = agent.chat_text("test 0.1.24").await.expect("chat");
    assert_eq!(reply, "Verified 0.1.24.");

    let posts = server.recorded_posts().await;
    assert!(posts.len() >= 2);
    let second_body = &posts[1].body;
    assert!(
        second_body.contains("public:ok"),
        "Tool content must reach model: {second_body}"
    );
    assert!(
        !second_body.contains("DO_NOT_LEAK_991"),
        "Host-only ToolOutput::metadata() must NEVER leak to model: {second_body}"
    );
    let metas = captured_meta.lock().expect("lock").clone();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0]["secret_host_meta"], "DO_NOT_LEAK_991");

    agent.signal_idle().await.expect("signal_idle");
    agent.shutdown().await.expect("shutdown");
}

#[cfg(feature = "native")]
#[tokio::test]
async fn native_live_0124_default_model_xhigh_and_local_openai_config() {
    let bridge = agy_bridge::AgyBridge::native_builder()
        .inter_agent_delay(std::time::Duration::ZERO)
        .build_native()
        .expect("build native bridge");
    verify_live_0124_default_model_xhigh_and_metadata_isolation(&bridge).await;

    let oai_cfg = LocalOpenAIConfig::new("gemma-3-27b", "http://127.0.0.1:11434/v1");
    oai_cfg.validate().expect("valid LocalOpenAIConfig");
    let v_cfg = VertexConfig::builder()
        .project("proj-1")
        .location("us-central1")
        .build();
    v_cfg
        .validate(&std::collections::HashMap::new())
        .expect("valid VertexConfig");
}

#[cfg(feature = "python")]
#[tokio::test]
async fn python_live_0124_default_model_xhigh_and_office_doc_fail_loud() {
    use agy_bridge::policies::PolicyRule;

    let bridge = agy_bridge::AgyBridge::python_builder()
        .inter_agent_delay(std::time::Duration::ZERO)
        .build_python()
        .expect("build python bridge");
    verify_live_0124_default_model_xhigh_and_metadata_isolation(&bridge).await;

    // Office Document (.pptx) on PythonRuntime fails loudly with Error::InvalidConfig
    // when google.antigravity.types.SUPPORTED_DOCUMENT_MIMES does not list PPTX yet.
    let save_dir = tempfile::tempdir().expect("save_dir");
    let agent = bridge
        .agent(
            AgentConfig::builder()
                .save_dir(save_dir.path())
                .api_key("test-key")
                .gemini(GeminiConfig {
                    api_key: Some("test-key".into()),
                    base_url: Some("http://127.0.0.1:9".into()),
                    ..Default::default()
                })
                .capabilities(CapabilitiesConfig::custom_tools_only())
                .policies([PolicyRule::AllowAll])
                .build(),
        )
        .await
        .expect("create python agent");
    let pptx_doc = Document::new(b"PK\x03\x04".to_vec(), mime::APPLICATION_PPTX);
    let err = match agent.chat(pptx_doc).await {
        Ok(_unexpected) => panic!("PPTX Document must fail with InvalidConfig on Python 0.1.24"),
        Err(e) => e,
    };
    assert!(
        matches!(err, Error::InvalidConfig { .. }),
        "Expected Error::InvalidConfig, got: {err:?}"
    );
    agent.shutdown().await.expect("shutdown");
}
