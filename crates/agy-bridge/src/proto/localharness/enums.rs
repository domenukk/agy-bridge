//! Top-level protocol enums (`antigravity.localharness`).

use crate::proto::serde_helpers::define_proto_enum;

define_proto_enum! {
    pub enum LifecycleHook {
        #[default]
        Unspecified = 0 => "LIFECYCLE_HOOK_UNSPECIFIED",
        OnSessionStart = 1 => "LIFECYCLE_HOOK_ON_SESSION_START",
        OnSessionEnd = 2 => "LIFECYCLE_HOOK_ON_SESSION_END",
        PreTurn = 3 => "LIFECYCLE_HOOK_PRE_TURN",
        PostTurn = 4 => "LIFECYCLE_HOOK_POST_TURN",
        PreTool = 5 => "LIFECYCLE_HOOK_PRE_TOOL",
        PostTool = 6 => "LIFECYCLE_HOOK_POST_TOOL",
        OnToolError = 7 => "LIFECYCLE_HOOK_ON_TOOL_ERROR",
        OnCompaction = 8 => "LIFECYCLE_HOOK_ON_COMPACTION",
        Stop = 9 => "LIFECYCLE_HOOK_STOP",
    }
}

define_proto_enum! {
    pub enum AgentBehavior {
        #[default]
        Unspecified = 0 => "AGENT_BEHAVIOR_UNSPECIFIED",
        Autonomous = 1 => "AGENT_BEHAVIOR_AUTONOMOUS",
        Interactive = 2 => "AGENT_BEHAVIOR_INTERACTIVE",
        Minimal = 3 => "AGENT_BEHAVIOR_MINIMAL",
    }
}

define_proto_enum! {
    pub enum ModelType {
        #[default]
        Unspecified = 0 => "MODEL_TYPE_UNSPECIFIED",
        Text = 1 => "MODEL_TYPE_TEXT",
        Image = 2 => "MODEL_TYPE_IMAGE",
    }
}

define_proto_enum! {
    pub enum PolicyDecision {
        #[default]
        Unspecified = 0 => "POLICY_DECISION_UNSPECIFIED",
        Allow = 1 => "POLICY_DECISION_ALLOW",
        Deny = 2 => "POLICY_DECISION_DENY",
        AskUser = 3 => "POLICY_DECISION_ASK_USER",
    }
}

define_proto_enum! {
    pub enum PolicyEvaluationOutcome {
        #[default]
        Unspecified = 0 => "POLICY_EVALUATION_OUTCOME_UNSPECIFIED",
        Allow = 1 => "POLICY_EVALUATION_OUTCOME_ALLOW",
        Deny = 2 => "POLICY_EVALUATION_OUTCOME_DENY",
        NoMatch = 3 => "POLICY_EVALUATION_OUTCOME_NO_MATCH",
    }
}

define_proto_enum! {
    pub enum ToolExecutionMode {
        #[default]
        Unspecified = 0 => "TOOL_EXECUTION_MODE_UNSPECIFIED",
        Sync = 1 => "TOOL_EXECUTION_MODE_SYNC",
        Async = 2 => "TOOL_EXECUTION_MODE_ASYNC",
    }
}

define_proto_enum! {
    pub enum Modality {
        #[default]
        Unspecified = 0 => "MODALITY_UNSPECIFIED",
        Text = 1 => "TEXT",
        Image = 2 => "IMAGE",
        Video = 3 => "VIDEO",
        Audio = 4 => "AUDIO",
        Document = 5 => "DOCUMENT",
    }
}
