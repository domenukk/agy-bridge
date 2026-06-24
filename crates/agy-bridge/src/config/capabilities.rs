//! Tool capability configuration.

use serde::{Deserialize, Serialize};

use super::DEFAULT_IMAGE_GENERATION_MODEL;

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinTools {
    /// List files and subdirectories.
    ListDir,
    /// Regex search within directory contents.
    SearchDir,
    /// Find files by name pattern.
    FindFile,
    /// Read file contents.
    ViewFile,
    /// Create a new file.
    CreateFile,
    /// Edit an existing file.
    EditFile,
    /// Execute a shell command.
    RunCommand,
    /// Ask the user a question.
    AskQuestion,
    /// Spawn a subagent.
    StartSubagent,
    /// Generate images from text prompts.
    GenerateImage,
    /// Signal task completion.
    Finish,
}

impl BuiltinTools {
    #[must_use]
    /// Returns tools that only read (no writes, no command execution).
    pub const fn read_only() -> &'static [Self] {
        &[
            Self::ListDir,
            Self::SearchDir,
            Self::FindFile,
            Self::ViewFile,
            Self::Finish,
        ]
    }

    /// Returns tools that cannot delete content (all except `RunCommand`).
    #[must_use]
    pub const fn nondestructive() -> &'static [Self] {
        &[
            Self::ListDir,
            Self::SearchDir,
            Self::FindFile,
            Self::ViewFile,
            Self::CreateFile,
            Self::EditFile,
            Self::AskQuestion,
            Self::StartSubagent,
            Self::GenerateImage,
            Self::Finish,
        ]
    }

    /// Returns all builtin tools.
    #[must_use]
    pub const fn all_tools() -> &'static [Self] {
        &[
            Self::ListDir,
            Self::SearchDir,
            Self::FindFile,
            Self::ViewFile,
            Self::CreateFile,
            Self::EditFile,
            Self::RunCommand,
            Self::AskQuestion,
            Self::StartSubagent,
            Self::GenerateImage,
            Self::Finish,
        ]
    }

    /// Returns tools that perform file read/write/create operations.
    ///
    /// These tools accept a file path argument and can be scoped to specific
    /// workspace directories via `policy::workspace_only()`.
    #[must_use]
    pub const fn file_tools() -> &'static [Self] {
        &[Self::ViewFile, Self::CreateFile, Self::EditFile]
    }

    /// Returns an empty tool list (no builtin tools).
    #[must_use]
    pub const fn none() -> &'static [Self] {
        &[]
    }

    #[must_use]
    /// Returns the Python SDK tool name string (e.g. `"list_directory"`).
    pub const fn as_sdk_name(&self) -> &'static str {
        match self {
            Self::ListDir => "list_directory",
            Self::SearchDir => "search_directory",
            Self::FindFile => "find_file",
            Self::ViewFile => "view_file",
            Self::CreateFile => "create_file",
            Self::EditFile => "edit_file",
            Self::RunCommand => "run_command",
            Self::AskQuestion => "ask_question",
            Self::StartSubagent => "start_subagent",
            Self::GenerateImage => "generate_image",
            Self::Finish => "finish",
        }
    }
}

impl std::fmt::Display for BuiltinTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_sdk_name())
    }
}

/// Agent capability toggles: tool allowlists, subagent support, and compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesConfig {
    /// Whether this agent can spawn subagents.
    #[serde(default = "super::default_true")]
    pub enable_subagents: bool,
    /// If set, only these built-in tools are available (allowlist).
    #[serde(default)]
    pub enabled_tools: Option<Vec<BuiltinTools>>,
    /// If set, these built-in tools are removed (denylist).
    #[serde(default)]
    pub disabled_tools: Option<Vec<BuiltinTools>>,
    /// Token threshold that triggers conversation compaction.
    pub compaction_threshold: Option<usize>,
    /// The model to use for image generation.
    ///
    /// This setting is a shorthand for `GeminiConfig.models.image_generation.name`.
    /// If both are specified, the value in [`GeminiConfig`](super::GeminiConfig) takes precedence and
    /// this field is ignored.
    #[serde(default = "super::default_image_model")]
    pub image_model: String,
    /// Optional JSON schema string for the finish tool's structured output.
    #[serde(default)]
    pub finish_tool_schema_json: Option<String>,
}

impl CapabilitiesConfig {
    /// Create a capabilities config with only the specified tools enabled.
    ///
    /// Subagent support is enabled by default.
    #[must_use]
    pub fn with_tools(tools: Vec<BuiltinTools>) -> Self {
        Self {
            enabled_tools: Some(tools),
            ..Self::default()
        }
    }

    /// Create a capabilities config with all tools and subagent support.
    #[must_use]
    pub fn full() -> Self {
        Self::default()
    }

    /// Create a capabilities config for read-only agents with subagent support.
    #[must_use]
    pub fn read_only() -> Self {
        Self {
            enabled_tools: Some(BuiltinTools::read_only().to_vec()),
            ..Self::default()
        }
    }

    /// Create a capabilities config with no builtin tools — only custom tools.
    ///
    /// Subagent support is still enabled.
    #[must_use]
    pub fn custom_tools_only() -> Self {
        Self {
            enabled_tools: Some(vec![]),
            ..Self::default()
        }
    }

    /// # Errors
    ///
    /// Returns an error if `enabled_tools` and `disabled_tools` are both provided.
    pub const fn validate(&self) -> Result<(), &'static str> {
        if self.enabled_tools.is_some() && self.disabled_tools.is_some() {
            return Err("enabled_tools and disabled_tools are mutually exclusive");
        }
        Ok(())
    }
}

impl Default for CapabilitiesConfig {
    fn default() -> Self {
        Self {
            enable_subagents: true,
            enabled_tools: None,
            disabled_tools: None,
            compaction_threshold: None,
            image_model: DEFAULT_IMAGE_GENERATION_MODEL.to_owned(),
            finish_tool_schema_json: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use pyo3::types::PyAnyMethods;

    use super::*;

    #[test]
    fn test_builtin_tools() {
        let read_only = BuiltinTools::read_only();
        assert_eq!(read_only.len(), 5);
        assert!(read_only.contains(&BuiltinTools::ListDir));
        assert!(read_only.contains(&BuiltinTools::Finish));
        assert!(!read_only.contains(&BuiltinTools::CreateFile));

        let all = BuiltinTools::all_tools();
        assert_eq!(all.len(), 11);
        assert!(all.contains(&BuiltinTools::CreateFile));
        assert!(all.contains(&BuiltinTools::Finish));

        assert_eq!(BuiltinTools::ListDir.as_sdk_name(), "list_directory");
    }

    #[test]
    fn test_capabilities_validation() {
        let mut caps = CapabilitiesConfig {
            enable_subagents: true,
            enabled_tools: Some(vec![BuiltinTools::ListDir]),
            ..CapabilitiesConfig::default()
        };
        assert!(caps.validate().is_ok());

        caps.disabled_tools = Some(vec![BuiltinTools::SearchDir]);
        assert!(caps.validate().is_err());
    }

    #[test]

    fn builtin_tools_serde_roundtrip_all_variants() {
        let all = BuiltinTools::all_tools();
        for tool in all {
            let json = serde_json::to_string(tool).unwrap();
            let parsed: BuiltinTools = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, tool, "Failed roundtrip for {tool:?}");
        }
    }

    #[test]
    fn builtin_tools_python_str_covers_all_variants() {
        let expected = [
            (BuiltinTools::ListDir, "list_directory"),
            (BuiltinTools::SearchDir, "search_directory"),
            (BuiltinTools::FindFile, "find_file"),
            (BuiltinTools::ViewFile, "view_file"),
            (BuiltinTools::CreateFile, "create_file"),
            (BuiltinTools::EditFile, "edit_file"),
            (BuiltinTools::RunCommand, "run_command"),
            (BuiltinTools::AskQuestion, "ask_question"),
            (BuiltinTools::StartSubagent, "start_subagent"),
            (BuiltinTools::GenerateImage, "generate_image"),
            (BuiltinTools::Finish, "finish"),
        ];
        for (variant, py_str) in expected {
            assert_eq!(
                variant.as_sdk_name(),
                py_str,
                "Python str mismatch for {variant:?}"
            );
        }
    }

    #[test]
    fn builtin_tools_read_only_is_subset_of_all() {
        let all = BuiltinTools::all_tools();
        let read_only = BuiltinTools::read_only();
        for tool in read_only {
            assert!(
                all.contains(tool),
                "{tool:?} in read_only but not in all_tools"
            );
        }
    }

    #[test]
    fn builtin_tools_read_only_excludes_write_tools() {
        let read_only = BuiltinTools::read_only();
        assert!(!read_only.contains(&BuiltinTools::CreateFile));
        assert!(!read_only.contains(&BuiltinTools::EditFile));
        assert!(!read_only.contains(&BuiltinTools::RunCommand));
        assert!(!read_only.contains(&BuiltinTools::StartSubagent));
        assert!(!read_only.contains(&BuiltinTools::GenerateImage));
        assert!(!read_only.contains(&BuiltinTools::AskQuestion));
    }

    #[test]
    fn capabilities_config_both_none_is_valid() {
        let caps = CapabilitiesConfig::default();
        assert!(caps.validate().is_ok());
    }

    #[test]
    fn capabilities_config_only_disabled_is_valid() {
        let caps = CapabilitiesConfig {
            disabled_tools: Some(vec![BuiltinTools::RunCommand]),
            compaction_threshold: Some(2000),
            ..CapabilitiesConfig::default()
        };
        assert!(caps.validate().is_ok());
    }

    #[test]
    fn capabilities_config_serde_roundtrip() {
        let caps = CapabilitiesConfig {
            enable_subagents: true,
            enabled_tools: Some(vec![BuiltinTools::ViewFile, BuiltinTools::ListDir]),
            compaction_threshold: Some(8000),
            ..CapabilitiesConfig::default()
        };
        let json = serde_json::to_string(&caps).unwrap();
        let parsed: CapabilitiesConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.enable_subagents);
        assert_eq!(parsed.enabled_tools.as_ref().unwrap().len(), 2);
        assert_eq!(parsed.compaction_threshold, Some(8000));
    }

    #[test]
    fn builtin_tools_snake_case_serde() {
        // Verify that serde serializes with snake_case as specified by the attribute.
        let tool = BuiltinTools::StartSubagent;
        let json = serde_json::to_string(&tool).unwrap();
        assert_eq!(json, "\"start_subagent\"");

        let tool = BuiltinTools::GenerateImage;
        let json = serde_json::to_string(&tool).unwrap();
        assert_eq!(json, "\"generate_image\"");
    }

    #[test]
    fn capabilities_config_empty_enabled_list_vs_none() {
        // An explicitly empty enabled_tools list means "no tools enabled"
        // whereas None means "use default set".
        let caps_empty = CapabilitiesConfig {
            enabled_tools: Some(vec![]),
            ..CapabilitiesConfig::default()
        };
        assert!(caps_empty.validate().is_ok());
        assert!(caps_empty.enabled_tools.as_ref().unwrap().is_empty());

        let caps_none = CapabilitiesConfig::default();
        assert!(caps_none.enabled_tools.is_none());
    }

    #[test]
    fn capabilities_default_enables_subagents() {
        // Matches the Python SDK default: enable_subagents=True
        let caps = CapabilitiesConfig::default();
        assert!(
            caps.enable_subagents,
            "enable_subagents should default to true, matching the SDK"
        );
    }

    #[test]
    fn capabilities_serde_missing_enable_subagents_defaults_true() {
        // When enable_subagents is absent from JSON, it should default to true.
        let json = r#"{"enabled_tools": ["view_file"]}"#;
        let caps: CapabilitiesConfig = serde_json::from_str(json).unwrap();
        assert!(
            caps.enable_subagents,
            "Missing enable_subagents in JSON should deserialize to true"
        );
    }

    #[test]
    fn capabilities_serde_explicit_false_is_respected() {
        let json = r#"{"enable_subagents": false}"#;
        let caps: CapabilitiesConfig = serde_json::from_str(json).unwrap();
        assert!(!caps.enable_subagents, "Explicit false should be preserved");
    }

    #[test]
    fn capabilities_with_tools_enables_subagents() {
        let caps = CapabilitiesConfig::with_tools(vec![
            BuiltinTools::ViewFile,
            BuiltinTools::StartSubagent,
        ]);
        assert!(caps.enable_subagents);
        assert_eq!(caps.enabled_tools.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn capabilities_full_enables_subagents() {
        let caps = CapabilitiesConfig::full();
        assert!(caps.enable_subagents);
        assert!(caps.enabled_tools.is_none()); // None = SDK defaults (all tools)
    }

    #[test]
    fn capabilities_read_only_enables_subagents_but_no_start_subagent() {
        let caps = CapabilitiesConfig::read_only();
        assert!(caps.enable_subagents);
        let tools = caps.enabled_tools.as_ref().unwrap();
        // read_only tools should NOT include StartSubagent
        assert!(
            !tools.contains(&BuiltinTools::StartSubagent),
            "read_only should not include StartSubagent in enabled_tools"
        );
    }

    #[test]
    fn capabilities_custom_tools_only_enables_subagents() {
        let caps = CapabilitiesConfig::custom_tools_only();
        assert!(caps.enable_subagents);
        assert!(caps.enabled_tools.as_ref().unwrap().is_empty());
    }

    #[test]
    fn start_subagent_in_all_tools_and_nondestructive() {
        let all = BuiltinTools::all_tools();
        assert!(
            all.contains(&BuiltinTools::StartSubagent),
            "all_tools() must include StartSubagent"
        );
        let nondestructive = BuiltinTools::nondestructive();
        assert!(
            nondestructive.contains(&BuiltinTools::StartSubagent),
            "nondestructive() must include StartSubagent"
        );
        let read_only = BuiltinTools::read_only();
        assert!(
            !read_only.contains(&BuiltinTools::StartSubagent),
            "read_only() must NOT include StartSubagent"
        );
    }

    /// Verify our `BuiltinTools` enum exactly matches the Python SDK's tool names.
    #[test]
    fn builtin_tools_match_python_sdk() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::runtime::venv::configure_python_sys_path(py)
                .unwrap_or_else(|e| panic!("Failed to configure python sys.path: {e}"));
            let types_mod = py
                .import("google.antigravity.types")
                .expect("Failed to import google.antigravity.types");
            let bt = types_mod
                .getattr("BuiltinTools")
                .expect("Failed to get BuiltinTools");
            // BuiltinTools is a (str, Enum) subclass — use `list(BuiltinTools)`
            // to iterate members, then extract `.value` from each.
            let builtins = py.import("builtins").expect("Failed to import builtins");
            let members = builtins
                .getattr("list")
                .expect("Failed to get list")
                .call1((bt,))
                .expect("Failed to call list(BuiltinTools)");
            let py_tools: Vec<String> = members
                .try_iter()
                .expect("Failed to iter members")
                .map(|item| {
                    item.and_then(|v| v.getattr("value"))
                        .and_then(|v| v.extract::<String>())
                })
                .collect::<pyo3::PyResult<Vec<String>>>()
                .expect("Failed to extract tool values");

            let rust_tools: Vec<String> = BuiltinTools::all_tools()
                .iter()
                .map(|t| t.as_sdk_name().to_owned())
                .collect();

            assert_eq!(
                rust_tools.len(),
                py_tools.len(),
                "Tool count mismatch: Rust has {}, Python has {}.\nRust: {rust_tools:?}\nPython: {py_tools:?}",
                rust_tools.len(),
                py_tools.len(),
            );

            for py_name in &py_tools {
                assert!(
                    rust_tools.contains(py_name),
                    "Python SDK has tool '{py_name}' but Rust BuiltinTools does not"
                );
            }

            for rust_name in &rust_tools {
                assert!(
                    py_tools.contains(rust_name),
                    "Rust BuiltinTools has '{rust_name}' but Python SDK does not"
                );
            }
        });
    }

    /// Verify the `BuiltinTools` enum maps correctly to the validate function.
    #[test]
    fn capabilities_validate_rejects_both_enabled_and_disabled() {
        let caps = CapabilitiesConfig {
            enabled_tools: Some(vec![BuiltinTools::ViewFile]),
            disabled_tools: Some(vec![BuiltinTools::RunCommand]),
            ..CapabilitiesConfig::default()
        };
        assert!(caps.validate().is_err());
    }
}
