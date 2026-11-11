//! Integration tests for pluggable external [`Runtime`](agy_bridge::agent::Runtime)
//! implementations using only public `agy_bridge` extension APIs.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, SystemTime},
};

use agy_bridge::{
    AgyBridge,
    agent::{AgentId, Runtime, validate_tool_config},
    config::AgentConfig,
    content::Content,
    error::Error,
    hooks::{
        HookResult, Hooks, OnSessionEndContext, OnSessionStartContext, OnToolErrorContext,
        PostToolCallContext, PostTurnContext, PreToolCallDecideContext, PreTurnContext,
        SessionContext,
    },
    policies::{AskUserHandler, PolicyRule},
    runtime::{
        UnconfiguredRuntime, agent_hooks, agent_policies, agent_policy_handler, agent_registry,
        agent_tool_state, check_tool_execution_allowed, clear_last_tool_error,
        get_agent_conversation_id, next_agent_id, record_last_tool_error, record_last_tool_output,
        remove_agent_bridge_state, set_agent_conversation_id, take_last_tool_error,
        take_last_tool_output, validate_create_config,
    },
    streaming::{
        self, ChatResponseHandle, ChatResponseWriter, ResponseEvent, StreamChunk, ToolCallEvent,
    },
    tools::{
        AvailableTool, EmptyParams, JsonSchema, RustTool, ToolContext, ToolError, ToolOutput,
        ToolRegistry, ToolSource,
    },
    types::{
        ConversationMessage, MessageRole, Step, StepSource, StepStatus, StepTarget, StepType,
        ToolResult, UsageMetadata,
    },
};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

const DEFAULT_CONVERSATION_PREFIX: &str = "custom-conv-";
const UPDATED_CONVERSATION_ID: &str = "custom-conv-reassigned";
const TOOL_COMPUTE: &str = "compute_sum";
const TOOL_FAILING: &str = "failing_probe";
const TOOL_CONFIRM: &str = "confirm_action";
const TOOL_DENIED: &str = "blocked_action";
const HANDLER_ID_CONFIRM: &str = "interactive_gate";
const META_KEY_SOURCE: &str = "source";
const META_VAL_CUSTOM: &str = "custom_tool";
const ERR_META_KEY_CODE: &str = "error_code";
const ERR_META_VAL_CODE: &str = "E_PROBE";
const STATE_KEY_CALLS: &str = "call_count";
const PROMPT_TOKENS: u64 = 42;
const CANDIDATE_TOKENS: u64 = 18;
const THOUGHT_TOKENS: u64 = 7;
const TOTAL_TOKENS: u64 = 67;

// ── Custom Tools ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ComputeSumParams {
    a: i64,
    b: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ComputeSumPayload {
    sum: i64,
    call_number: u64,
    conversation_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ComputeMetadata {
    source: &'static str,
}

#[derive(Debug, Serialize)]
struct ProbeErrorMetadata {
    error_code: &'static str,
}

struct ComputeSumTool;

impl RustTool for ComputeSumTool {
    type Params = ComputeSumParams;
    const NAME: &'static str = TOOL_COMPUTE;
    const DESCRIPTION: &'static str = "Computes a sum and records per-agent state.";

    async fn call(&self, params: Self::Params, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let Some(prev) = ctx
            .get_state(STATE_KEY_CALLS, serde_json::json!(0))
            .as_u64()
        else {
            return Err(ToolError::new("state counter was not u64"));
        };
        let next = prev + 1;
        ctx.set_state(STATE_KEY_CALLS, serde_json::json!(next))?;

        let payload = ComputeSumPayload {
            sum: params.a + params.b,
            call_number: next,
            conversation_id: ctx.conversation_id().map(str::to_owned),
        };
        let encoded = serde_json::to_string(&payload)
            .map_err(|e| ToolError::new(format!("serialize payload failed: {e}")))?;
        ToolOutput::new(encoded).with_metadata(&ComputeMetadata {
            source: META_VAL_CUSTOM,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ProbeParams {
    target: String,
}

struct FailingProbeTool;

impl RustTool for FailingProbeTool {
    type Params = ProbeParams;
    const NAME: &'static str = TOOL_FAILING;
    const DESCRIPTION: &'static str = "Always fails with structured error metadata.";

    async fn call(
        &self,
        params: Self::Params,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let err = ToolError::new(format!("probe failed for {}", params.target)).with_metadata(
            &ProbeErrorMetadata {
                error_code: ERR_META_VAL_CODE,
            },
        )?;
        Err(err)
    }
}

struct ConfirmActionTool;

impl RustTool for ConfirmActionTool {
    type Params = EmptyParams;
    const NAME: &'static str = TOOL_CONFIRM;
    const DESCRIPTION: &'static str = "Requires interactive confirmation.";

    async fn call(
        &self,
        _params: Self::Params,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new("confirmed-ok"))
    }
}

struct BlockedActionTool;

impl RustTool for BlockedActionTool {
    type Params = EmptyParams;
    const NAME: &'static str = TOOL_DENIED;
    const DESCRIPTION: &'static str = "Denied by policy.";

    async fn call(
        &self,
        _params: Self::Params,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new("should-never-run"))
    }
}

// ── Interactive Confirmation Handler ────────────────────────────────────────

struct ToggleAskUserHandler {
    allow: Arc<AtomicBool>,
    observed_tools: Arc<Mutex<Vec<String>>>,
}

impl AskUserHandler for ToggleAskUserHandler {
    fn confirm(&self, tool_name: &str, _tool_args: &serde_json::Value) -> bool {
        self.observed_tools
            .lock()
            .expect("observed_tools lock")
            .push(tool_name.to_owned());
        self.allow.load(Ordering::SeqCst)
    }
}

// ── Scripted In-Memory CustomRuntime ────────────────────────────────────────

#[derive(Clone)]
enum ScriptedTurn {
    StreamedReply {
        thought: String,
        text_chunks: Vec<String>,
        structured: Option<serde_json::Value>,
    },
    ToolInvoke {
        call_id: String,
        tool_name: String,
        args: serde_json::Value,
        follow_up: String,
    },
}

struct CustomSession {
    history: Mutex<Vec<ConversationMessage>>,
    turns: AtomicU32,
}

struct CustomRuntime {
    tag: &'static str,
    script: Mutex<VecDeque<ScriptedTurn>>,
    sessions: Mutex<HashMap<AgentId, Arc<CustomSession>>>,
    try_shutdown_count: AtomicU32,
}

impl CustomRuntime {
    fn new(tag: &'static str, turns: impl IntoIterator<Item = ScriptedTurn>) -> Self {
        Self {
            tag,
            script: Mutex::new(turns.into_iter().collect()),
            sessions: Mutex::new(HashMap::new()),
            try_shutdown_count: AtomicU32::new(0),
        }
    }

    const fn tag(&self) -> &'static str {
        self.tag
    }

    fn next_scripted_turn(&self) -> ScriptedTurn {
        self.script
            .lock()
            .expect("script mutex")
            .pop_front()
            .unwrap_or_else(|| ScriptedTurn::StreamedReply {
                thought: String::new(),
                text_chunks: vec!["default-reply".to_owned()],
                structured: None,
            })
    }

    fn lookup_session(&self, agent_id: AgentId) -> Result<Arc<CustomSession>, Error> {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .get(&agent_id)
            .cloned()
            .ok_or(Error::AgentNotStarted)
    }
}

async fn execute_tool_in_custom_runtime(
    agent_id: AgentId,
    call_id: &str,
    tool_name: &str,
    raw_args: serde_json::Value,
    writer: &ChatResponseWriter,
) -> String {
    writer
        .emit_tool_call(ToolCallEvent {
            name: tool_name.to_owned(),
            args: raw_args.clone(),
            id: Some(call_id.to_owned()),
            canonical_path: None,
        })
        .await;

    let args_json = serde_json::to_string(&raw_args).expect("serialize tool args");
    match check_tool_execution_allowed(agent_id, tool_name, &args_json) {
        Ok(true) => {}
        Ok(false) => {
            let msg = format!("policy denied tool '{tool_name}'");
            writer
                .emit_event(ResponseEvent::ToolResult(ToolResult {
                    name: tool_name.to_owned(),
                    id: Some(call_id.to_owned()),
                    result: serde_json::Value::Null,
                    error: Some(msg.clone()),
                }))
                .await;
            return msg;
        }
        Err(err) => return err.to_string(),
    }

    let hooks = agent_hooks(agent_id);
    let mut effective_args = raw_args;
    if let Some(ref hr) = hooks {
        let pre_ctx = PreToolCallDecideContext::new(tool_name, effective_args.clone());
        let decision = hr.run_pre_tool_call_decide(&pre_ctx);
        if !decision.allow {
            return format!("hook denied: {}", decision.message);
        }
        effective_args = hr.run_transform_tool_input(&pre_ctx);
    }

    dispatch_registered_tool(agent_id, call_id, tool_name, effective_args, hooks, writer).await
}

async fn dispatch_registered_tool(
    agent_id: AgentId,
    call_id: &str,
    tool_name: &str,
    effective_args: serde_json::Value,
    hooks: Option<Arc<Hooks>>,
    writer: &ChatResponseWriter,
) -> String {
    let registry = agent_registry(agent_id).expect("tool registry present");
    let state = agent_tool_state(agent_id).expect("tool shared state present");
    let mut ctx = ToolContext::new().with_shared_state(state);
    if let Some(conv_id) = get_agent_conversation_id(agent_id) {
        ctx = ctx.with_conversation_id(conv_id);
    }

    match registry
        .dispatch(tool_name, effective_args.clone(), &ctx)
        .await
    {
        Ok(output) => {
            clear_last_tool_error(agent_id);
            let meta_val = serde_json::to_value(output.metadata()).expect("serialize metadata");
            record_last_tool_output(agent_id, effective_args, meta_val);
            let captured = take_last_tool_output(agent_id).expect("captured tool output");
            let content = output.content().to_owned();
            if let Some(ref hr) = hooks {
                let post_ctx = PostToolCallContext::new(
                    tool_name,
                    captured.tool_args,
                    content.clone(),
                    captured.metadata,
                );
                hr.run_post_tool_call(&post_ctx);
            }
            writer
                .emit_event(ResponseEvent::ToolResult(ToolResult {
                    name: tool_name.to_owned(),
                    id: Some(call_id.to_owned()),
                    result: serde_json::json!({ "output": content }),
                    error: None,
                }))
                .await;
            content
        }
        Err(tool_err) => {
            record_last_tool_error(agent_id, effective_args, &tool_err);
            let captured = take_last_tool_error(agent_id).expect("captured tool error");
            let err_meta = captured
                .metadata
                .map_or(serde_json::Value::Null, serde_json::Value::Object);
            let rendered = if let Some(ref hr) = hooks {
                let err_ctx = OnToolErrorContext::new(
                    tool_name,
                    captured.tool_args,
                    tool_err.to_string(),
                    err_meta,
                );
                hr.run_on_tool_error(&err_ctx)
                    .unwrap_or_else(|| tool_err.to_string())
            } else {
                tool_err.to_string()
            };
            writer
                .emit_event(ResponseEvent::ToolResult(ToolResult {
                    name: tool_name.to_owned(),
                    id: Some(call_id.to_owned()),
                    result: serde_json::Value::Null,
                    error: Some(rendered.clone()),
                }))
                .await;
            rendered
        }
    }
}

fn sample_usage() -> UsageMetadata {
    UsageMetadata {
        prompt_token_count: Some(PROMPT_TOKENS),
        cached_content_token_count: None,
        candidates_token_count: Some(CANDIDATE_TOKENS),
        thoughts_token_count: Some(THOUGHT_TOKENS),
        total_token_count: Some(TOTAL_TOKENS),
        ..UsageMetadata::default()
    }
}

fn build_step(step_index: u32, text: &str, thought: &str) -> Step {
    let mut step = Step::default();
    step.id = format!("step-{step_index}");
    step.step_index = step_index;
    step.step_type = StepType::TextResponse;
    step.source = StepSource::Model;
    step.target = StepTarget::User;
    step.status = StepStatus::Done;
    text.clone_into(&mut step.content);
    text.clone_into(&mut step.content_delta);
    thought.clone_into(&mut step.thinking);
    thought.clone_into(&mut step.thinking_delta);
    step.is_complete_response = Some(true);
    step.usage_metadata = Some(sample_usage());
    step
}

impl Runtime for CustomRuntime {
    async fn create_agent(
        &self,
        agent_id: u64,
        config: AgentConfig,
    ) -> Result<(AgentId, Vec<AvailableTool>), Error> {
        validate_create_config(&config)?;
        let registry = agent_registry(agent_id);
        validate_tool_config(&config, registry.as_deref())?;

        assert!(
            agent_policies(agent_id).is_some(),
            "agent_policies must be populated before Runtime::create_agent"
        );

        let initial_conv_id = if let Some(existing) = get_agent_conversation_id(agent_id) {
            existing
        } else {
            let generated = format!("{DEFAULT_CONVERSATION_PREFIX}{agent_id}");
            set_agent_conversation_id(agent_id, generated.clone())?;
            generated
        };

        if let Some(hooks) = agent_hooks(agent_id) {
            let session = SessionContext::new(initial_conv_id, agent_id, SystemTime::now());
            hooks.run_on_session_start(&OnSessionStartContext::new(session));
        }

        let available_tools = config
            .tools
            .iter()
            .map(|def| AvailableTool {
                name: def.name.clone(),
                description: def.description.clone(),
                parameter_schema: def.parameter_schema.clone(),
                source: ToolSource::Custom,
            })
            .collect();

        self.sessions.lock().expect("sessions mutex").insert(
            agent_id,
            Arc::new(CustomSession {
                history: Mutex::new(Vec::new()),
                turns: AtomicU32::new(0),
            }),
        );

        Ok((agent_id, available_tools))
    }

    async fn chat(
        &self,
        agent_id: AgentId,
        content: &Content,
    ) -> Result<ChatResponseHandle, Error> {
        let session = self.lookup_session(agent_id)?;
        let prompt = content.as_text().unwrap_or("").to_owned();
        let turn_num = session.turns.fetch_add(1, Ordering::SeqCst) + 1;

        if let Some(hooks) = agent_hooks(agent_id) {
            let decision = hooks.run_pre_turn(&PreTurnContext::new(prompt.clone(), turn_num));
            if !decision.allow {
                return Err(Error::BackendError {
                    message: decision.message,
                });
            }
        }

        session
            .history
            .lock()
            .expect("history mutex")
            .push(ConversationMessage {
                role: MessageRole::User,
                content: prompt,
            });

        let scripted = self.next_scripted_turn();
        let (writer, handle) = streaming::channel();
        let session_clone = Arc::clone(&session);

        tokio::spawn(async move {
            let final_text = match scripted {
                ScriptedTurn::StreamedReply {
                    thought,
                    text_chunks,
                    structured,
                } => {
                    if !thought.is_empty() {
                        writer.emit_thought(&thought).await;
                    }
                    let mut combined = String::new();
                    for chunk in text_chunks {
                        combined.push_str(&chunk);
                        writer.emit_text(chunk).await;
                    }
                    writer
                        .emit_step(build_step(turn_num, &combined, &thought))
                        .await;
                    writer.set_usage(sample_usage());
                    if let Some(val) = structured {
                        writer.set_structured_output(val);
                    }
                    combined
                }
                ScriptedTurn::ToolInvoke {
                    call_id,
                    tool_name,
                    args,
                    follow_up,
                } => {
                    let outcome = execute_tool_in_custom_runtime(
                        agent_id, &call_id, &tool_name, args, &writer,
                    )
                    .await;
                    let reply = format!("{follow_up}: {outcome}");
                    writer.emit_text(&reply).await;
                    writer.emit_step(build_step(turn_num, &reply, "")).await;
                    writer.set_usage(sample_usage());
                    reply
                }
            };

            if let Some(hooks) = agent_hooks(agent_id) {
                hooks.run_post_turn(&PostTurnContext::new(final_text.as_str(), turn_num));
            }

            session_clone
                .history
                .lock()
                .expect("history mutex")
                .push(ConversationMessage {
                    role: MessageRole::Model,
                    content: final_text,
                });
        });

        Ok(handle)
    }

    async fn shutdown_agent(&self, agent_id: AgentId) -> Result<(), Error> {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .remove(&agent_id);
        if let Some(hooks) = agent_hooks(agent_id) {
            let conv_id =
                get_agent_conversation_id(agent_id).unwrap_or_else(|| format!("agent-{agent_id}"));
            let session = SessionContext::new(conv_id, agent_id, SystemTime::UNIX_EPOCH);
            hooks.run_on_session_end(&OnSessionEndContext::new(session));
        }
        Ok(())
    }

    fn try_shutdown_agent(&self, agent_id: AgentId) {
        self.try_shutdown_count.fetch_add(1, Ordering::SeqCst);
        self.sessions
            .lock()
            .expect("sessions mutex")
            .remove(&agent_id);
        if let Some(hooks) = agent_hooks(agent_id) {
            let conv_id =
                get_agent_conversation_id(agent_id).unwrap_or_else(|| format!("agent-{agent_id}"));
            let session = SessionContext::new(conv_id, agent_id, SystemTime::UNIX_EPOCH);
            hooks.run_on_session_end(&OnSessionEndContext::new(session));
        }
        remove_agent_bridge_state(agent_id);
    }

    async fn cancel(&self, agent_id: AgentId) -> Result<(), Error> {
        self.lookup_session(agent_id).map(|_s| ())
    }

    async fn wait_for_idle(&self, agent_id: AgentId) -> Result<(), Error> {
        self.lookup_session(agent_id).map(|_s| ())
    }

    async fn send(&self, agent_id: AgentId, _content: &Content) -> Result<(), Error> {
        self.lookup_session(agent_id).map(|_s| ())
    }

    async fn signal_idle(&self, agent_id: AgentId) -> Result<(), Error> {
        self.lookup_session(agent_id).map(|_s| ())
    }

    async fn wait_for_wakeup(&self, agent_id: AgentId, _timeout: Duration) -> Result<bool, Error> {
        self.lookup_session(agent_id).map(|_s| false)
    }

    async fn history(&self, agent_id: AgentId) -> Result<Vec<ConversationMessage>, Error> {
        let session = self.lookup_session(agent_id)?;
        Ok(session.history.lock().expect("history mutex").clone())
    }

    async fn turn_count(&self, agent_id: AgentId) -> Result<u32, Error> {
        let session = self.lookup_session(agent_id)?;
        Ok(session.turns.load(Ordering::SeqCst))
    }

    async fn total_usage(&self, agent_id: AgentId) -> Result<UsageMetadata, Error> {
        self.lookup_session(agent_id).map(|_s| sample_usage())
    }

    async fn last_turn_usage(&self, agent_id: AgentId) -> Result<UsageMetadata, Error> {
        self.lookup_session(agent_id).map(|_s| sample_usage())
    }

    async fn clear_history(&self, agent_id: AgentId) -> Result<(), Error> {
        let session = self.lookup_session(agent_id)?;
        session.history.lock().expect("history mutex").clear();
        session.turns.store(0, Ordering::SeqCst);
        Ok(())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

struct RecordedHookEvents {
    events: Arc<Mutex<Vec<String>>>,
}

impl RecordedHookEvents {
    fn new() -> (Self, Hooks) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let e_start = Arc::clone(&events);
        let e_pre_turn = Arc::clone(&events);
        let e_post_turn = Arc::clone(&events);
        let e_pre_tool = Arc::clone(&events);
        let e_post_tool = Arc::clone(&events);
        let e_tool_err = Arc::clone(&events);
        let e_end = Arc::clone(&events);

        let hooks = Hooks::new()
            .with_session_start("session-start", move |ctx| {
                e_start
                    .lock()
                    .expect("events")
                    .push(format!("start:{}", ctx.session.session_id));
            })
            .with_pre_turn("pre-turn", move |ctx| {
                e_pre_turn
                    .lock()
                    .expect("events")
                    .push(format!("pre_turn:{}:{}", ctx.turn_number, ctx.prompt));
                if ctx.prompt == "block-this-turn" {
                    HookResult::deny("turn blocked by pre_turn")
                } else {
                    HookResult::allow()
                }
            })
            .with_post_turn("post-turn", move |ctx| {
                e_post_turn
                    .lock()
                    .expect("events")
                    .push(format!("post_turn:{}", ctx.turn_number));
            })
            .with_pre_tool_call_decide("pre-tool", move |ctx| {
                e_pre_tool
                    .lock()
                    .expect("events")
                    .push(format!("pre_tool:{}", ctx.tool_name));
                HookResult::allow()
            })
            .with_transform_tool_input("add-bonus", |ctx| {
                if ctx.tool_name == TOOL_COMPUTE {
                    let a = ctx.tool_args.get("a").and_then(serde_json::Value::as_i64)?;
                    let b = ctx.tool_args.get("b").and_then(serde_json::Value::as_i64)?;
                    Some(serde_json::json!({ "a": a + 100, "b": b }))
                } else {
                    None
                }
            })
            .with_post_tool_call("post-tool", move |ctx| {
                let src = ctx
                    .metadata
                    .get(META_KEY_SOURCE)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("missing");
                e_post_tool
                    .lock()
                    .expect("events")
                    .push(format!("post_tool:{}:{src}", ctx.tool_name));
            })
            .with_tool_error("on-error", move |ctx| {
                let code = ctx
                    .metadata
                    .get(ERR_META_KEY_CODE)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("missing");
                e_tool_err
                    .lock()
                    .expect("events")
                    .push(format!("tool_error:{}:{code}", ctx.tool_name));
                Some(format!("transformed[{code}]: {}", ctx.error))
            })
            .with_session_end("session-end", move |ctx| {
                e_end
                    .lock()
                    .expect("events")
                    .push(format!("end:{}", ctx.session.session_id));
            });

        (Self { events }, hooks)
    }

    fn snapshot(&self) -> Vec<String> {
        self.events.lock().expect("events").clone()
    }
}

fn full_lifecycle_script() -> [ScriptedTurn; 6] {
    [
        ScriptedTurn::ToolInvoke {
            call_id: "c1".to_owned(),
            tool_name: TOOL_COMPUTE.to_owned(),
            args: serde_json::json!({ "a": 3, "b": 4 }),
            follow_up: "sum1".to_owned(),
        },
        ScriptedTurn::ToolInvoke {
            call_id: "c2".to_owned(),
            tool_name: TOOL_COMPUTE.to_owned(),
            args: serde_json::json!({ "a": 5, "b": 6 }),
            follow_up: "sum2".to_owned(),
        },
        ScriptedTurn::ToolInvoke {
            call_id: "c3".to_owned(),
            tool_name: TOOL_FAILING.to_owned(),
            args: serde_json::json!({ "target": "db" }),
            follow_up: "err".to_owned(),
        },
        ScriptedTurn::ToolInvoke {
            call_id: "c4".to_owned(),
            tool_name: TOOL_CONFIRM.to_owned(),
            args: serde_json::json!({}),
            follow_up: "confirm_allow".to_owned(),
        },
        ScriptedTurn::ToolInvoke {
            call_id: "c5".to_owned(),
            tool_name: TOOL_CONFIRM.to_owned(),
            args: serde_json::json!({}),
            follow_up: "confirm_deny".to_owned(),
        },
        ScriptedTurn::ToolInvoke {
            call_id: "c6".to_owned(),
            tool_name: TOOL_DENIED.to_owned(),
            args: serde_json::json!({}),
            follow_up: "policy_deny".to_owned(),
        },
    ]
}

#[tokio::test]
async fn test_custom_runtime_full_lifecycle_hooks_tools_policies_and_state() {
    let runtime = CustomRuntime::new("in-memory-v1", full_lifecycle_script());
    let bridge = AgyBridge::with_runtime(runtime);
    assert_eq!(bridge.runtime().tag(), "in-memory-v1");

    let registry = ToolRegistry::new()
        .with_tool(ComputeSumTool)
        .with_tool(FailingProbeTool)
        .with_tool(ConfirmActionTool)
        .with_tool(BlockedActionTool);
    let (recorder, hooks) = RecordedHookEvents::new();

    let allow_confirm = Arc::new(AtomicBool::new(true));
    let observed_confirms = Arc::new(Mutex::new(Vec::new()));
    let ask_handler = ToggleAskUserHandler {
        allow: Arc::clone(&allow_confirm),
        observed_tools: Arc::clone(&observed_confirms),
    };

    let config = AgentConfig::builder()
        .policies([
            PolicyRule::allow(TOOL_COMPUTE),
            PolicyRule::allow(TOOL_FAILING),
            PolicyRule::AskUser {
                tool: TOOL_CONFIRM.to_owned(),
                handler_id: HANDLER_ID_CONFIRM.to_owned(),
            },
            PolicyRule::deny(TOOL_DENIED),
            PolicyRule::DenyAll,
        ])
        .build();

    let agent = bridge
        .agent(config)
        .tools(registry)
        .hooks(hooks)
        .policy_handler(ask_handler)
        .await
        .expect("agent creation succeeds");

    assert_eq!(agent.runtime().tag(), "in-memory-v1");
    assert!(agent_policy_handler(agent.id()).is_some());

    let expected_conv = format!("{DEFAULT_CONVERSATION_PREFIX}{}", agent.id());
    assert_eq!(
        agent.conversation_id().as_deref(),
        Some(expected_conv.as_str())
    );

    let r1 = agent.chat_text("first").await.expect("turn 1");
    assert!(r1.contains("\"sum\":107"), "transform + sum expected: {r1}");
    assert!(
        r1.contains("\"call_number\":1"),
        "first call expected: {r1}"
    );

    set_agent_conversation_id(agent.id(), UPDATED_CONVERSATION_ID.to_owned())
        .expect("reassign conversation id");
    assert_eq!(
        agent.conversation_id().as_deref(),
        Some(UPDATED_CONVERSATION_ID)
    );

    let r2 = agent.chat_text("second").await.expect("turn 2");
    assert!(r2.contains("\"sum\":111"), "second sum expected: {r2}");
    assert!(
        r2.contains("\"call_number\":2"),
        "persisted state expected: {r2}"
    );
    assert!(
        r2.contains(UPDATED_CONVERSATION_ID),
        "updated conversation id must reach ToolContext: {r2}"
    );

    let r3 = agent.chat_text("third").await.expect("turn 3");
    assert!(
        r3.contains("transformed[E_PROBE]: probe failed for db"),
        "on_tool_error metadata + transform expected: {r3}"
    );

    let r4 = agent.chat_text("fourth").await.expect("turn 4");
    assert_eq!(r4, "confirm_allow: confirmed-ok");

    allow_confirm.store(false, Ordering::SeqCst);
    let r5 = agent.chat_text("fifth").await.expect("turn 5");
    assert!(r5.contains("policy denied tool 'confirm_action'"), "{r5}");

    let r6 = agent.chat_text("sixth").await.expect("turn 6");
    assert!(r6.contains("policy denied tool 'blocked_action'"), "{r6}");

    let denied_turn = agent
        .chat_text("block-this-turn")
        .await
        .expect_err("pre_turn must deny");
    assert!(denied_turn.to_string().contains("turn blocked by pre_turn"));

    assert_eq!(
        observed_confirms.lock().expect("observed").as_slice(),
        &[TOOL_CONFIRM, TOOL_CONFIRM]
    );

    agent.shutdown().await.expect("shutdown succeeds");

    let hook_log = recorder.snapshot();
    assert!(hook_log.contains(&format!("start:{expected_conv}")));
    assert!(hook_log.contains(&format!("post_tool:{TOOL_COMPUTE}:{META_VAL_CUSTOM}")));
    assert!(hook_log.contains(&format!("tool_error:{TOOL_FAILING}:{ERR_META_VAL_CODE}")));
    assert!(hook_log.contains(&format!("end:{UPDATED_CONVERSATION_ID}")));
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct StructuredAnswer {
    verdict: String,
    score: u32,
}

#[tokio::test]
async fn test_custom_runtime_streaming_fan_out_usage_and_structured_output() {
    let structured_json = serde_json::json!({
        "verdict": "approved",
        "score": 99
    });
    let runtime = CustomRuntime::new(
        "streaming-v1",
        [ScriptedTurn::StreamedReply {
            thought: "analyzing invariants".to_owned(),
            text_chunks: vec![
                "Hello ".to_owned(),
                "custom ".to_owned(),
                "world!".to_owned(),
            ],
            structured: Some(structured_json.clone()),
        }],
    );
    let bridge = AgyBridge::with_runtime(runtime);
    let agent = bridge
        .default_agent()
        .await
        .expect("default agent on custom runtime");

    let mut response = agent.chat("stream everything").await.expect("chat starts");
    let mut text_rx = response.take_text_stream().expect("text stream");
    let mut thought_rx = response.take_thought_stream().expect("thought stream");
    let mut chunk_stream = response.receive_chunks().expect("chunk stream");
    let mut step_stream = response.receive_steps().expect("step stream");
    let mut event_rx = response.take_event_stream().expect("event stream");

    let mut texts = Vec::new();
    while let Some(tok) = text_rx.recv().await {
        texts.push(tok);
    }
    assert_eq!(texts.concat(), "Hello custom world!");

    let mut thoughts = Vec::new();
    while let Some(th) = thought_rx.recv().await {
        thoughts.push(th);
    }
    assert_eq!(thoughts, vec!["analyzing invariants"]);

    let mut chunks = Vec::new();
    while let Some(ch) = chunk_stream.next().await {
        chunks.push(ch);
    }
    assert_eq!(chunks.len(), 4);
    assert!(matches!(&chunks[0], StreamChunk::Thought(t) if t == "analyzing invariants"));
    assert!(matches!(&chunks[1], StreamChunk::Text(t) if t == "Hello "));

    let mut steps = Vec::new();
    while let Some(st) = step_stream.next().await {
        steps.push(st);
    }
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].content, "Hello custom world!");

    let mut events = Vec::new();
    while let Some(ev) = event_rx.recv().await {
        events.push(ev);
    }
    assert_eq!(events.len(), 4);
    assert!(matches!(&events[0], ResponseEvent::ThoughtChunk(t) if t == "analyzing invariants"));

    let usage = agent.get_last_usage().expect("usage recorded");
    assert_eq!(usage.prompt_token_count, Some(PROMPT_TOKENS));
    assert_eq!(usage.total_token_count, Some(TOTAL_TOKENS));

    let parsed: StructuredAnswer = agent
        .get_last_structured_output_as::<StructuredAnswer>()
        .expect("structured output present")
        .expect("deserializes cleanly");
    assert_eq!(
        parsed,
        StructuredAnswer {
            verdict: "approved".to_owned(),
            score: 99,
        }
    );

    agent.shutdown().await.expect("shutdown");

    // Verify ChatResponseWriter::is_abandoned() reflects handle receiver drop.
    let (writer, handle) = streaming::channel();
    assert!(!writer.is_abandoned());
    drop(handle);
    assert!(writer.is_abandoned());
}

#[tokio::test]
async fn test_drop_without_shutdown_fires_try_shutdown_and_removes_bridge_state() {
    let runtime = Arc::new(CustomRuntime::new("drop-test", []));
    let bridge = AgyBridge::new(Arc::clone(&runtime));
    let session_ended = Arc::new(AtomicBool::new(false));
    let ended_clone = Arc::clone(&session_ended);

    let hooks = Hooks::new().with_session_end("drop-end", move |_ctx| {
        ended_clone.store(true, Ordering::SeqCst);
    });
    let registry = ToolRegistry::new().with_tool(ConfirmActionTool);

    let agent_id = {
        let agent = bridge
            .default_agent()
            .tools(registry)
            .hooks(hooks)
            .await
            .expect("agent created");
        let id = agent.id();
        assert!(agent_registry(id).is_some());
        assert!(agent_hooks(id).is_some());
        assert!(agent_policies(id).is_some());
        assert!(agent_tool_state(id).is_some());
        assert!(get_agent_conversation_id(id).is_some());
        id
        // `agent` dropped here without `.shutdown().await`
    };

    assert_eq!(runtime.try_shutdown_count.load(Ordering::SeqCst), 1);
    assert!(session_ended.load(Ordering::SeqCst));
    assert!(agent_registry(agent_id).is_none());
    assert!(agent_hooks(agent_id).is_none());
    assert!(agent_policies(agent_id).is_none());
    assert!(agent_policy_handler(agent_id).is_none());
    assert!(agent_tool_state(agent_id).is_none());
    assert!(get_agent_conversation_id(agent_id).is_none());
}

#[tokio::test]
async fn test_unconfigured_runtime_returns_runtime_not_started() {
    let rt = UnconfiguredRuntime;
    assert!(!rt.is_alive());

    let builder = AgyBridge::builder().channel_capacity(32);
    assert_eq!(builder.config().channel_capacity, 32);

    let id = next_agent_id();
    let content = Content::from("ping");

    assert!(matches!(
        rt.create_agent(id, AgentConfig::default()).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.chat(id, &content).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.shutdown_agent(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(rt.cancel(id).await, Err(Error::RuntimeNotStarted)));
    assert!(matches!(
        rt.wait_for_idle(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.send(id, &content).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.signal_idle(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.wait_for_wakeup(id, Duration::from_millis(1)).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.history(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.turn_count(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.total_usage(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.last_turn_usage(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.clear_history(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.last_response(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.compaction_indices(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(rt.delete(id).await, Err(Error::RuntimeNotStarted)));
    assert!(matches!(
        rt.disconnect(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.is_idle(id).await,
        Err(Error::RuntimeNotStarted)
    ));
    assert!(matches!(
        rt.sandbox_status(id).await,
        Err(Error::RuntimeNotStarted)
    ));

    let bridge = AgyBridge::with_runtime(UnconfiguredRuntime);
    assert!(matches!(
        bridge.active_agent_count().await,
        Err(Error::RuntimeNotStarted)
    ));
    let err = match bridge.default_agent().await {
        Ok(_agent) => panic!("UnconfiguredRuntime must reject agent creation"),
        Err(e) => e,
    };
    assert!(matches!(err, Error::RuntimeNotStarted), "got: {err:?}");
    assert!(!err.is_retryable());
    assert!(!err.is_quota_error());
}
