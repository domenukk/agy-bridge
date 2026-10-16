use std::time::SystemTime;

use super::*;

#[test]
fn hook_result_allow() {
    let r = HookResult::allow();
    assert!(r.allow);
    assert!(r.message.is_empty());
}

#[test]
fn hook_result_deny() {
    let r = HookResult::deny("blocked by policy");
    assert!(!r.allow);
    assert_eq!(r.message, "blocked by policy");
}

#[test]
fn hook_result_allow_with_message() {
    let r = HookResult::allow_with_message("proceeding with caution");
    assert!(r.allow);
    assert_eq!(r.message, "proceeding with caution");
}

#[test]
fn hook_point_labels() {
    assert_eq!(HookPoint::PreTurn.label(), "pre_turn");
    assert_eq!(HookPoint::PostTurn.label(), "post_turn");
    assert_eq!(HookPoint::PreToolCallDecide.label(), "pre_tool_call_decide");
    assert_eq!(HookPoint::PostToolCall.label(), "post_tool_call");
    assert_eq!(HookPoint::OnCompaction.label(), "on_compaction");
    assert_eq!(HookPoint::OnSessionStart.label(), "on_session_start");
    assert_eq!(HookPoint::OnSessionEnd.label(), "on_session_end");
    assert_eq!(HookPoint::OnToolError.label(), "on_tool_error");
    assert_eq!(HookPoint::OnInteraction.label(), "on_interaction");
}

#[test]
fn hooks_fire_in_correct_order() {
    let mut set = HookSet::new();
    assert!(set.is_empty());

    set.push(HookEntry {
        name: "pre_turn_1".to_owned(),
        point: HookPoint::PreTurn,
        callback_id: "cb_pre1".to_owned(),
    })
    .unwrap();
    set.push(HookEntry {
        name: "pre_tool_decide".to_owned(),
        point: HookPoint::PreToolCallDecide,
        callback_id: "cb_decide".to_owned(),
    })
    .unwrap();
    set.push(HookEntry {
        name: "pre_turn_2".to_owned(),
        point: HookPoint::PreTurn,
        callback_id: "cb_pre2".to_owned(),
    })
    .unwrap();
    set.push(HookEntry {
        name: "post_turn_1".to_owned(),
        point: HookPoint::PostTurn,
        callback_id: "cb_post1".to_owned(),
    })
    .unwrap();
    set.push(HookEntry {
        name: "post_tool_1".to_owned(),
        point: HookPoint::PostToolCall,
        callback_id: "cb_posttool1".to_owned(),
    })
    .unwrap();

    assert_eq!(set.len(), 5);

    let pre_turn: Vec<&str> = set
        .at_point(HookPoint::PreTurn)
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(pre_turn, vec!["pre_turn_1", "pre_turn_2"]);

    let decide: Vec<&str> = set
        .at_point(HookPoint::PreToolCallDecide)
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(decide, vec!["pre_tool_decide"]);

    let post_turn: Vec<&str> = set
        .at_point(HookPoint::PostTurn)
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(post_turn, vec!["post_turn_1"]);

    let post_tool: Vec<&str> = set
        .at_point(HookPoint::PostToolCall)
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(post_tool, vec!["post_tool_1"]);
}

#[test]
fn hook_entry_serde_roundtrip() {
    let entry = HookEntry {
        name: "my_hook".to_owned(),
        point: HookPoint::PreToolCallDecide,
        callback_id: "cb_123".to_owned(),
    };
    let json = serde_json::to_string(&entry).expect("serialize");
    let parsed: HookEntry = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.name, entry.name);
    assert_eq!(parsed.point, entry.point);
    assert_eq!(parsed.callback_id, entry.callback_id);
}

#[test]
fn hook_result_serde_roundtrip() {
    let results = vec![
        HookResult::allow(),
        HookResult::deny("reason"),
        HookResult::allow_with_message("ok"),
    ];
    for result in &results {
        let json = serde_json::to_string(result).expect("serialize");
        let parsed: HookResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&parsed, result);
    }
}

#[test]
fn hook_set_serde_roundtrip() {
    let mut set = HookSet::new();
    set.push(HookEntry {
        name: "gate".to_owned(),
        point: HookPoint::PreTurn,
        callback_id: "cb_1".to_owned(),
    })
    .unwrap();
    set.push(HookEntry {
        name: "logger".to_owned(),
        point: HookPoint::PostToolCall,
        callback_id: "cb_2".to_owned(),
    })
    .unwrap();
    let json = serde_json::to_string(&set).expect("serialize");
    let parsed: HookSet = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.len(), 2);
    let names: Vec<&str> = parsed.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["gate", "logger"]);
}

#[test]
fn hook_set_from_conversions() {
    let mut set = HookSet::new();
    set.push(HookEntry {
        name: "gate".to_owned(),
        point: HookPoint::PreTurn,
        callback_id: "cb_1".to_owned(),
    })
    .unwrap();

    let vec_from_owned: Vec<HookEntry> = Vec::from(set.clone());
    assert_eq!(vec_from_owned.len(), 1);
    assert_eq!(vec_from_owned[0].name, "gate");

    let vec_from_ref: Vec<HookEntry> = Vec::from(&set);
    assert_eq!(vec_from_ref.len(), 1);
    assert_eq!(vec_from_ref[0].name, "gate");

    let entry = HookEntry {
        name: "gate".to_owned(),
        point: HookPoint::PreTurn,
        callback_id: "cb_1".to_owned(),
    };
    let set_from_arr = HookSet::from([entry.clone()]);
    assert_eq!(set_from_arr.len(), 1);

    let set_from_vec = HookSet::from(vec![entry]);
    assert_eq!(set_from_vec.len(), 1);
}

#[test]
fn empty_hook_set_iteration_at_each_point() {
    let set = HookSet::new();
    for point in [
        HookPoint::PreTurn,
        HookPoint::PostTurn,
        HookPoint::PreToolCallDecide,
        HookPoint::PostToolCall,
        HookPoint::OnCompaction,
        HookPoint::OnSessionStart,
        HookPoint::OnSessionEnd,
        HookPoint::OnToolError,
        HookPoint::OnInteraction,
    ] {
        assert_eq!(
            set.at_point(point).count(),
            0,
            "Empty HookSet should have 0 hooks at {point:?}"
        );
    }
}

#[test]
fn hook_point_serde_roundtrip() {
    let points = [
        HookPoint::PreTurn,
        HookPoint::PostTurn,
        HookPoint::PreToolCallDecide,
        HookPoint::PostToolCall,
        HookPoint::OnCompaction,
        HookPoint::OnSessionStart,
        HookPoint::OnSessionEnd,
        HookPoint::OnToolError,
        HookPoint::OnInteraction,
    ];
    for point in points {
        let json = serde_json::to_string(&point).expect("serialize");
        let parsed: HookPoint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, point);
    }
}

#[test]
fn hook_set_default_is_empty() {
    let set = HookSet::default();
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
}

#[test]
fn hook_set_multiple_hooks_at_same_point() {
    let mut set = HookSet::new();
    for i in 0..5 {
        set.push(HookEntry {
            name: format!("hook_{i}"),
            point: HookPoint::PreToolCallDecide,
            callback_id: format!("cb_{i}"),
        })
        .unwrap();
    }
    assert_eq!(set.len(), 5);
    assert_eq!(set.at_point(HookPoint::PreToolCallDecide).count(), 5);
    assert_eq!(set.at_point(HookPoint::PreTurn).count(), 0);
}

#[test]
fn hook_result_deny_with_string_owned() {
    let reason = String::from("policy violation detected");
    let r = HookResult::deny(reason.clone());
    assert!(!r.allow);
    assert_eq!(r.message, reason);
}

#[test]
fn hook_entry_with_new_hook_points() {
    let new_points = [
        (HookPoint::OnCompaction, "compaction_hook"),
        (HookPoint::OnSessionStart, "session_start_hook"),
        (HookPoint::OnSessionEnd, "session_end_hook"),
        (HookPoint::OnToolError, "tool_error_hook"),
        (HookPoint::OnInteraction, "interaction_hook"),
    ];
    let mut set = HookSet::new();
    for (point, name) in &new_points {
        set.push(HookEntry {
            name: (*name).to_owned(),
            point: *point,
            callback_id: format!("cb_{name}"),
        })
        .unwrap();
    }
    assert_eq!(set.len(), 5);
    for (point, name) in &new_points {
        let hooks: Vec<&str> = set.at_point(*point).map(|e| e.name.as_str()).collect();
        assert_eq!(hooks, vec![*name], "expected hook at {point:?}");
    }
}

#[test]
fn hook_entry_serde_roundtrip_new_points() {
    let new_points = [
        HookPoint::OnCompaction,
        HookPoint::OnSessionStart,
        HookPoint::OnSessionEnd,
        HookPoint::OnToolError,
        HookPoint::OnInteraction,
    ];
    for point in new_points {
        let entry = HookEntry {
            name: format!("test_{}", point.label()),
            point,
            callback_id: format!("cb_{}", point.label()),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let parsed: HookEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, entry.name);
        assert_eq!(parsed.point, entry.point);
        assert_eq!(parsed.callback_id, entry.callback_id);
    }
}

// ── SessionContext tests ────────────────────────────────────────────

#[test]
fn session_context_clone() {
    let ctx = SessionContext {
        session_id: "sess-1".into(),
        agent_id: 42,
        started_at: SystemTime::now(),
    };
    let cloned = ctx;
    assert_eq!(cloned.session_id, "sess-1");
    assert_eq!(cloned.agent_id, 42);
}

#[test]
fn session_context_debug_format() {
    let ctx = SessionContext {
        session_id: "sess-debug".into(),
        agent_id: 1,
        started_at: SystemTime::now(),
    };
    let dbg = format!("{ctx:?}");
    assert!(dbg.contains("sess-debug"));
    assert!(dbg.contains("agent_id: 1"));
}

#[test]
fn session_context_serde_roundtrip_preserves_started_at() {
    let original = SessionContext {
        session_id: "sess-rt".into(),
        agent_id: 99,
        started_at: SystemTime::now(),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let parsed: SessionContext = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.session_id, original.session_id);
    assert_eq!(parsed.agent_id, original.agent_id);
    // SystemTime roundtrips through serde; Instant did not.
    assert_eq!(parsed.started_at, original.started_at);
}

// ── HookEntry::new validated constructor tests ──────────────────────

#[test]
fn hook_entry_new_valid() {
    let entry = HookEntry::new("safety_gate", HookPoint::PreToolCallDecide, "cb_safety")
        .expect("valid entry");
    assert_eq!(entry.name, "safety_gate");
    assert_eq!(entry.point, HookPoint::PreToolCallDecide);
    assert_eq!(entry.callback_id, "cb_safety");
}

#[test]
fn hook_entry_new_rejects_empty_name() {
    let result = HookEntry::new("", HookPoint::PreTurn, "cb_1");
    assert!(result.is_err(), "should reject empty name");
}

#[test]
fn hook_entry_new_rejects_whitespace_name() {
    let result = HookEntry::new("   ", HookPoint::PreTurn, "cb_1");
    assert!(result.is_err(), "should reject whitespace-only name");
}

#[test]
fn hook_entry_new_rejects_empty_callback_id() {
    let result = HookEntry::new("my_hook", HookPoint::PreTurn, "");
    assert!(result.is_err(), "should reject empty callback_id");
}

#[test]
fn hook_entry_new_rejects_whitespace_callback_id() {
    let result = HookEntry::new("my_hook", HookPoint::PostTurn, "  ");
    assert!(result.is_err(), "should reject whitespace-only callback_id");
}

#[test]
fn pre_tool_call_decide_context_serde_aliases() {
    let json_std = r#"{"tool_name":"my_tool","tool_args":{"foo":"bar"}}"#;
    let parsed_std: PreToolCallDecideContext = serde_json::from_str(json_std).unwrap();
    assert_eq!(parsed_std.tool_name, "my_tool");
    assert_eq!(parsed_std.tool_args["foo"], "bar");

    let json_alias = r#"{"name":"my_tool","args":{"foo":"bar"}}"#;
    let parsed_alias: PreToolCallDecideContext = serde_json::from_str(json_alias).unwrap();
    assert_eq!(parsed_alias.tool_name, "my_tool");
    assert_eq!(parsed_alias.tool_args["foo"], "bar");
}

#[test]
fn pre_tool_call_decide_context_serde_default() {
    let json_no_args = r#"{"name":"my_tool"}"#;
    let parsed_no_args: PreToolCallDecideContext = serde_json::from_str(json_no_args).unwrap();
    assert_eq!(parsed_no_args.tool_name, "my_tool");
    assert_eq!(parsed_no_args.tool_args, serde_json::Value::Null);
}

#[test]
fn post_tool_call_context_serde_aliases_and_default() {
    let json_std = r#"{"tool_name":"my_tool","tool_args":{"foo":"bar"},"result":"success"}"#;
    let parsed_std: PostToolCallContext = serde_json::from_str(json_std).unwrap();
    assert_eq!(parsed_std.tool_name, "my_tool");
    assert_eq!(parsed_std.tool_args["foo"], "bar");
    assert_eq!(parsed_std.result, "success");

    let json_alias = r#"{"name":"my_tool","args":{"foo":"bar"},"result":"success"}"#;
    let parsed_alias: PostToolCallContext = serde_json::from_str(json_alias).unwrap();
    assert_eq!(parsed_alias.tool_name, "my_tool");
    assert_eq!(parsed_alias.tool_args["foo"], "bar");
    assert_eq!(parsed_alias.result, "success");

    let json_no_args = r#"{"name":"my_tool","result":"success"}"#;
    let parsed_no_args: PostToolCallContext = serde_json::from_str(json_no_args).unwrap();
    assert_eq!(parsed_no_args.tool_name, "my_tool");
    assert_eq!(parsed_no_args.tool_args, serde_json::Value::Null);
    assert_eq!(parsed_no_args.result, "success");
}

#[test]
fn on_tool_error_context_serde_aliases_and_default() {
    let json_std = r#"{"tool_name":"my_tool","tool_args":{"foo":"bar"},"error":"failed"}"#;
    let parsed_std: OnToolErrorContext = serde_json::from_str(json_std).unwrap();
    assert_eq!(parsed_std.tool_name, "my_tool");
    assert_eq!(parsed_std.tool_args["foo"], "bar");
    assert_eq!(parsed_std.error, "failed");

    let json_alias = r#"{"name":"my_tool","args":{"foo":"bar"},"error":"failed"}"#;
    let parsed_alias: OnToolErrorContext = serde_json::from_str(json_alias).unwrap();
    assert_eq!(parsed_alias.tool_name, "my_tool");
    assert_eq!(parsed_alias.tool_args["foo"], "bar");
    assert_eq!(parsed_alias.error, "failed");

    let json_no_args = r#"{"name":"my_tool","error":"failed"}"#;
    let parsed_no_args: OnToolErrorContext = serde_json::from_str(json_no_args).unwrap();
    assert_eq!(parsed_no_args.tool_name, "my_tool");
    assert_eq!(parsed_no_args.tool_args, serde_json::Value::Null);
    assert_eq!(parsed_no_args.error, "failed");

    let json_no_name = r#"{"error":"failed"}"#;
    let parsed_no_name: Result<OnToolErrorContext, _> = serde_json::from_str(json_no_name);
    assert!(parsed_no_name.is_err());
}

#[test]
fn on_tool_error_context_metadata_defaults_to_null() {
    let json = r#"{"tool_name":"my_tool","error":"failed"}"#;
    let parsed: OnToolErrorContext = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.metadata, serde_json::Value::Null);
    assert!(!parsed.is_not_found());
}

#[test]
fn on_tool_error_context_metadata_deserialized() {
    let json = r#"{"tool_name":"my_tool","error":"failed","metadata":{"status_code":503}}"#;
    let parsed: OnToolErrorContext = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.metadata["status_code"], 503);
}

#[test]
fn on_tool_error_context_is_not_found_detects_registry_miss() {
    // Metadata mirrors what `ToolError::not_found` attaches.
    let error = llm_tool::ToolError::not_found(llm_tool::RegistryItem::Tool, "add_nummbers");
    let ctx = OnToolErrorContext {
        tool_name: "add_nummbers".into(),
        tool_args: serde_json::Value::Null,
        error: error.to_string(),
        metadata: serde_json::to_value(error.metadata()).unwrap(),
    };
    assert!(ctx.is_not_found());
}

#[test]
fn on_tool_error_context_is_not_found_false_for_generic_error() {
    let ctx = OnToolErrorContext {
        tool_name: "t".into(),
        tool_args: serde_json::Value::Null,
        error: "handler blew up".into(),
        metadata: serde_json::json!({"some": "value"}),
    };
    assert!(!ctx.is_not_found());
}
