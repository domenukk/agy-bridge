//! Integration tests for Native backend localharness process lifecycle,
//! multiplexing, and cleanup on agent shutdown/drop.

#![cfg(feature = "native")]

use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    time::Duration,
};

use agy_bridge::{AgyBridge, config::AgentConfig, proto};
use futures::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

struct MockHarness {
    _dir: tempfile::TempDir,
    bin_path: std::path::PathBuf,
    pid_file: std::path::PathBuf,
}

impl MockHarness {
    fn create(port: u16, prefix: &str, record_pid: bool) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("mock_harness_{prefix}_"))
            .tempdir()
            .expect("create mock tempdir");
        let bin_path = dir.path().join("mock_localharness");
        let payload_path = dir.path().join("payload.dat");
        let pid_file = dir.path().join("harness.pid");

        let output_config = proto::localharness::OutputConfig {
            port: i32::from(port),
            api_key: "mock-api-key".to_string(),
        };
        let mut out_bytes = Vec::new();
        output_config
            .encode(&mut out_bytes)
            .expect("encode output config");

        let mut frame = Vec::new();
        let len_u32 = u32::try_from(out_bytes.len()).expect("len fits u32");
        frame.extend_from_slice(&len_u32.to_le_bytes());
        frame.extend_from_slice(&out_bytes);

        fs::write(&payload_path, &frame).expect("write mock payload");

        let script_content = if record_pid {
            format!(
                "#!/bin/sh\necho $$ > '{}'\ncat '{}'\nexec sleep 30\n",
                pid_file.display(),
                payload_path.display()
            )
        } else {
            format!(
                "#!/bin/sh\ncat '{}'\nexec sleep 30\n",
                payload_path.display()
            )
        };

        fs::write(&bin_path, script_content).expect("write mock binary");
        fs::set_permissions(&bin_path, Permissions::from_mode(0o755)).expect("set executable");

        Self {
            _dir: dir,
            bin_path,
            pid_file,
        }
    }

    fn bin_path(&self) -> &std::path::Path {
        &self.bin_path
    }

    fn read_pid(&self) -> String {
        fs::read_to_string(&self.pid_file)
            .expect("read pid file")
            .trim()
            .to_string()
    }
}

async fn handle_mock_agent_session(
    stream: tokio::net::TcpStream,
    cascade_id: String,
    response_text: String,
) {
    let mut ws = accept_async(stream).await.expect("ws handshake");

    let init_msg = ws.next().await.expect("first message").expect("valid msg");
    let init_text = init_msg.to_text().expect("text msg");
    let _init_event: proto::localharness::InitializeConversationEvent =
        serde_json::from_str(init_text).expect("parse init event");

    let init_resp = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::InitializeConversationResponse(
                proto::localharness::InitializeConversationResponse {
                    cascade_id,
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&init_resp).unwrap().into(),
    ))
    .await
    .expect("send init resp");

    let user_msg = ws.next().await.expect("user msg").expect("valid msg");
    let user_text = user_msg.to_text().expect("text msg");
    let _input_event: proto::localharness::InputEvent =
        serde_json::from_str(user_text).expect("parse input event");

    let step_event = proto::localharness::OutputEvent {
        event: Some(proto::localharness::output_event::Event::StepUpdate(
            proto::localharness::StepUpdate {
                text_delta: response_text.clone(),
                text: response_text,
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&step_event).unwrap().into(),
    ))
    .await
    .expect("send step update");

    let state_event = proto::localharness::OutputEvent {
        event: Some(
            proto::localharness::output_event::Event::TrajectoryStateUpdate(
                proto::localharness::TrajectoryStateUpdate {
                    state: 3,
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };
    ws.send(Message::Text(
        serde_json::to_string(&state_event).unwrap().into(),
    ))
    .await
    .expect("send state update");
}

#[tokio::test]
async fn test_native_backend_multiplexing_multiple_agents_on_single_harness() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();

    let server_task = tokio::spawn(async move {
        // Accept connection 1 for agent 1
        let (stream1, _) = listener.accept().await.expect("accept connection 1");
        let h1 = tokio::spawn(handle_mock_agent_session(
            stream1,
            "cascade-multi-1".to_string(),
            "Answer from agent 1".to_string(),
        ));

        // Accept connection 2 for agent 2 ON THE SAME PORT / HARNESS
        let (stream2, _) = listener.accept().await.expect("accept connection 2");
        let h2 = tokio::spawn(handle_mock_agent_session(
            stream2,
            "cascade-multi-2".to_string(),
            "Answer from agent 2".to_string(),
        ));

        tokio::try_join!(h1, h2).expect("mock agent tasks panicked");
    });

    let mock = MockHarness::create(port, "multiplex", false);

    let bridge = AgyBridge::native_builder()
        .harness_path(mock.bin_path())
        .build_native()
        .expect("build bridge");

    // Spawn agent 1
    let agent1 = bridge
        .agent(AgentConfig::default())
        .await
        .expect("create agent 1");
    assert_eq!(
        agent1.conversation_id(),
        Some("cascade-multi-1".to_string())
    );

    // Spawn agent 2 on the SAME bridge (reuses the same harness process)
    let agent2 = bridge
        .agent(AgentConfig::default())
        .await
        .expect("create agent 2");
    assert_eq!(
        agent2.conversation_id(),
        Some("cascade-multi-2".to_string())
    );

    assert_eq!(bridge.active_agent_count().await.expect("count"), 2);
    // Verify both agents are multiplexed onto a single localharness process
    assert_eq!(bridge.runtime().active_harness_count().await, 1);

    // Both agents chat concurrently
    let (reply1, reply2) = tokio::join!(
        agent1.chat_text("Hello from 1"),
        agent2.chat_text("Hello from 2"),
    );
    assert_eq!(reply1.expect("reply 1"), "Answer from agent 1");
    assert_eq!(reply2.expect("reply 2"), "Answer from agent 2");

    // Shut down agent 1; agent 2 remains active, harness stays alive
    agent1.shutdown().await.expect("shutdown agent 1");
    assert_eq!(bridge.active_agent_count().await.expect("count"), 1);
    assert_eq!(bridge.runtime().active_harness_count().await, 1);

    // Shut down agent 2
    agent2.shutdown().await.expect("shutdown agent 2");
    assert_eq!(bridge.active_agent_count().await.expect("count"), 0);
    assert_eq!(
        bridge.runtime().active_harness_count().await,
        0,
        "shared harness should be terminated as soon as last active agent shuts down"
    );

    server_task.await.expect("server task completed");
    bridge.runtime().shutdown().await.expect("runtime shutdown");
    assert_eq!(bridge.runtime().active_harness_count().await, 0);
}

#[tokio::test]
async fn test_native_harness_killed_when_last_agent_shuts_down() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut ws = accept_async(stream).await.expect("ws handshake");

        let init_msg = ws.next().await.expect("first message").expect("valid msg");
        let init_text = init_msg.to_text().expect("text msg");
        let _init_event: proto::localharness::InitializeConversationEvent =
            serde_json::from_str(init_text).expect("parse init event");

        let init_resp = proto::localharness::OutputEvent {
            event: Some(
                proto::localharness::output_event::Event::InitializeConversationResponse(
                    proto::localharness::InitializeConversationResponse {
                        cascade_id: "cascade-kill-test".to_string(),
                        ..Default::default()
                    },
                ),
            ),
            ..Default::default()
        };
        ws.send(Message::Text(
            serde_json::to_string(&init_resp).unwrap().into(),
        ))
        .await
        .expect("send init resp");
    });

    let mock = MockHarness::create(port, "kill_last_agent", true);

    let save_dir = tempfile::tempdir().expect("tempdir");
    let bridge = AgyBridge::native_builder()
        .harness_path(mock.bin_path())
        .build_native()
        .expect("build bridge");

    let agent = bridge
        .agent(AgentConfig {
            save_dir: Some(save_dir.path().to_path_buf()),
            ..AgentConfig::default()
        })
        .await
        .expect("create agent");

    assert_eq!(bridge.runtime().active_harness_count().await, 1);

    let pid_str = mock.read_pid();

    // Verify child process is alive before shutdown
    let alive_before = std::process::Command::new("kill")
        .args(["-0", &pid_str])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run kill -0")
        .success();
    assert!(
        alive_before,
        "harness child process should be alive before shutdown"
    );

    agent.shutdown().await.expect("shutdown agent");

    assert_eq!(
        bridge.runtime().active_harness_count().await,
        0,
        "harness count must drop to 0 immediately when last agent shuts down"
    );

    // Verify child process has exited and been reaped
    let alive_after = std::process::Command::new("kill")
        .args(["-0", &pid_str])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run kill -0")
        .success();
    assert!(
        !alive_after,
        "harness child process PID {pid_str} should have exited and been reaped"
    );

    server_task.await.expect("server task completed");
}

#[tokio::test]
async fn test_native_shared_harness_survives_until_last_agent_shuts_down() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();

    let server_task = tokio::spawn(async move {
        let (stream1, _) = listener.accept().await.expect("accept connection 1");
        let h1 = tokio::spawn(handle_mock_agent_session(
            stream1,
            "shared-cascade-1".to_string(),
            "Reply 1".to_string(),
        ));

        let (stream2, _) = listener.accept().await.expect("accept connection 2");
        let h2 = tokio::spawn(handle_mock_agent_session(
            stream2,
            "shared-cascade-2".to_string(),
            "Reply 2".to_string(),
        ));

        tokio::try_join!(h1, h2).expect("mock agent tasks");
    });

    let mock = MockHarness::create(port, "shared_survive", false);
    let shared_dir = tempfile::tempdir().expect("shared tempdir");

    let bridge = AgyBridge::native_builder()
        .harness_path(mock.bin_path())
        .build_native()
        .expect("build bridge");

    let config = AgentConfig {
        save_dir: Some(shared_dir.path().to_path_buf()),
        ..AgentConfig::default()
    };

    let agent1 = bridge.agent(config.clone()).await.expect("create agent 1");
    let agent2 = bridge.agent(config).await.expect("create agent 2");

    assert_eq!(bridge.runtime().active_harness_count().await, 1);

    let (r1, r2) = tokio::join!(agent1.chat_text("hi 1"), agent2.chat_text("hi 2"));
    assert_eq!(r1.expect("reply 1"), "Reply 1");
    assert_eq!(r2.expect("reply 2"), "Reply 2");

    // Shut down agent 1; harness must remain active for agent 2
    agent1.shutdown().await.expect("shutdown agent 1");
    assert_eq!(
        bridge.runtime().active_harness_count().await,
        1,
        "shared harness should stay alive while agent 2 is still active"
    );

    // Shut down agent 2; now harness must be terminated
    agent2.shutdown().await.expect("shutdown agent 2");
    assert_eq!(
        bridge.runtime().active_harness_count().await,
        0,
        "shared harness should be terminated once last agent (agent 2) shuts down"
    );

    server_task.await.expect("server task completed");
}

#[tokio::test]
async fn test_native_harness_killed_when_agent_handle_dropped() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept connection");
        let mut ws = accept_async(stream).await.expect("ws handshake");

        let init_msg = ws.next().await.expect("first message").expect("valid msg");
        let init_text = init_msg.to_text().expect("text msg");
        let _init_event: proto::localharness::InitializeConversationEvent =
            serde_json::from_str(init_text).expect("parse init event");

        let init_resp = proto::localharness::OutputEvent {
            event: Some(
                proto::localharness::output_event::Event::InitializeConversationResponse(
                    proto::localharness::InitializeConversationResponse {
                        cascade_id: "cascade-drop-test".to_string(),
                        ..Default::default()
                    },
                ),
            ),
            ..Default::default()
        };
        ws.send(Message::Text(
            serde_json::to_string(&init_resp).unwrap().into(),
        ))
        .await
        .expect("send init resp");
    });

    let mock = MockHarness::create(port, "drop_handle", true);

    let save_dir = tempfile::tempdir().expect("tempdir");
    let bridge = AgyBridge::native_builder()
        .harness_path(mock.bin_path())
        .build_native()
        .expect("build bridge");

    {
        let _agent = bridge
            .agent(AgentConfig {
                save_dir: Some(save_dir.path().to_path_buf()),
                ..AgentConfig::default()
            })
            .await
            .expect("create agent");

        assert_eq!(bridge.runtime().active_harness_count().await, 1);
        // Drop `_agent` without calling `.shutdown().await`
    }

    let pid_str = mock.read_pid();

    // Poll briefly for async reaper task spawned during Drop to complete
    let mut exited = false;
    for _ in 0..20 {
        if bridge.runtime().active_harness_count().await == 0 {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid_str])
                .stderr(std::process::Stdio::null())
                .status()
                .expect("run kill -0")
                .success();
            if !alive {
                exited = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert_eq!(
        bridge.runtime().active_harness_count().await,
        0,
        "harness count must be 0 after AgentHandle is dropped"
    );
    assert!(
        exited,
        "harness child process PID {pid_str} should have exited after AgentHandle drop"
    );

    server_task.await.expect("server task completed");
}
