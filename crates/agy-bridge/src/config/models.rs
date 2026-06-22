//! Model configuration types.

use serde::{Deserialize, Serialize};

use super::{DEFAULT_IMAGE_GENERATION_MODEL, DEFAULT_MODEL};

/// Controls the depth of extended thinking for models that support it.
///
/// Higher levels allow the model more internal reasoning steps at the cost
/// of increased latency and token usage.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ThinkingLevel {
    /// Least reasoning depth; fastest and cheapest.
    Minimal,
    /// Below-average reasoning depth.
    Low,
    /// Balanced reasoning depth (the default).
    #[default]
    Medium,
    /// Maximum reasoning depth; highest latency and token usage.
    High,
}

impl ThinkingLevel {
    /// Returns the lowercase string representation used in serialization.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl std::fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Generation parameters for a model, mirroring the SDK's `GenerationConfig`.
///
/// Currently only `thinking_level` is forwarded to the Gemini backend via
/// the Antigravity SDK. Additional generation parameters (temperature,
/// `top_p`, etc.) will be added when the SDK exposes them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationConfig {
    /// Thinking level for models that support extended thinking.
    /// When `None`, the model's default level is used.
    #[serde(default)]
    pub thinking_level: Option<ThinkingLevel>,
}

/// A single model slot with its name, optional API key, and generation config.
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Model identifier (e.g. `"gemini-3.5-flash"`).
    pub name: String,
    /// Per-model API key override.
    pub api_key: Option<String>,
    /// Generation parameters for this model.
    #[serde(default)]
    pub generation: GenerationConfig,
}

impl Default for ModelEntry {
    fn default() -> Self {
        default_model_entry()
    }
}

impl std::fmt::Debug for ModelEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelEntry")
            .field("name", &self.name)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("generation", &self.generation)
            .finish()
    }
}

/// Model selection for each capability, mirroring the SDK's `ModelConfig`.
///
/// Each slot holds a full [`ModelEntry`] (with optional per-model API key
/// and generation config). Bare model name strings are accepted via
/// `#[serde(deserialize_with)]` coercion on the Python side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// The primary reasoning model.
    #[serde(default = "default_model_entry")]
    pub default: ModelEntry,
    /// The model used for image generation.
    #[serde(default = "default_image_model_entry")]
    pub image_generation: ModelEntry,
}

pub(crate) fn default_model_entry() -> ModelEntry {
    ModelEntry {
        name: DEFAULT_MODEL.to_owned(),
        api_key: None,
        generation: GenerationConfig::default(),
    }
}

pub(crate) fn default_image_model_entry() -> ModelEntry {
    ModelEntry {
        name: DEFAULT_IMAGE_GENERATION_MODEL.to_owned(),
        api_key: None,
        generation: GenerationConfig::default(),
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            default: default_model_entry(),
            image_generation: default_image_model_entry(),
        }
    }
}

/// Configuration for the Gemini model backend.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct GeminiConfig {
    /// Shared API key for all models. Falls back to `$GEMINI_API_KEY` env var.
    /// Individual `ModelEntry` instances can override this.
    pub api_key: Option<String>,
    /// Base URL for the Gemini API endpoint.
    /// When set, overrides the default Gemini API endpoint (e.g., for a local
    /// proxy, staging environment, or alternative API-compatible gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Per-modality model selection and configuration.
    #[serde(default)]
    pub models: ModelConfig,
}

impl std::fmt::Debug for GeminiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("base_url", &self.base_url)
            .field("models", &self.models)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_level_serde() {
        let level = ThinkingLevel::Minimal;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"minimal\"");
        let parsed: ThinkingLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ThinkingLevel::Minimal);

        let level = ThinkingLevel::High;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"high\"");

        assert_eq!(ThinkingLevel::Medium.as_str(), "medium");
    }

    #[test]
    fn model_entry_serde_roundtrip() {
        let entry = ModelEntry {
            name: "gemini-3.5-flash".to_string(),
            api_key: Some("mock_test_api_key_123".to_string()),
            generation: GenerationConfig {
                thinking_level: Some(ThinkingLevel::High),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ModelEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "gemini-3.5-flash");
        assert_eq!(parsed.api_key.as_deref(), Some("mock_test_api_key_123"));
        assert_eq!(parsed.generation.thinking_level, Some(ThinkingLevel::High));
    }

    #[test]
    fn model_entry_minimal_serde() {
        let json = r#"{"name":"flash"}"#;
        let parsed: ModelEntry = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.name, "flash");
        assert!(parsed.api_key.is_none());
        assert!(parsed.generation.thinking_level.is_none());
    }

    #[test]
    fn model_config_serde_roundtrip() {
        let config = ModelConfig {
            default: ModelEntry {
                name: "gemini-3.5-flash".to_string(),
                api_key: None,
                generation: GenerationConfig::default(),
            },
            image_generation: ModelEntry {
                name: "imagen-3".to_string(),
                api_key: None,
                generation: GenerationConfig::default(),
            },
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ModelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.default.name, "gemini-3.5-flash");
        assert_eq!(parsed.image_generation.name, "imagen-3");
    }

    #[test]
    fn model_config_defaults() {
        let config = ModelConfig::default();
        assert_eq!(config.default.name, DEFAULT_MODEL);
        assert_eq!(config.image_generation.name, DEFAULT_IMAGE_GENERATION_MODEL);
    }

    #[test]
    fn gemini_config_serde_roundtrip() {
        let config = GeminiConfig {
            api_key: Some("global-key".to_string()),
            base_url: None,
            models: ModelConfig {
                default: ModelEntry {
                    name: "gemini-3.5-flash".to_string(),
                    api_key: None,
                    generation: GenerationConfig::default(),
                },
                image_generation: default_image_model_entry(),
            },
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: GeminiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.api_key.as_deref(), Some("global-key"));
        assert!(parsed.base_url.is_none());
        assert_eq!(parsed.models.default.name, "gemini-3.5-flash");
    }

    #[test]
    fn gemini_config_default() {
        let config = GeminiConfig::default();
        assert!(config.api_key.is_none());
        assert_eq!(config.models.default.name, DEFAULT_MODEL);
        assert_eq!(
            config.models.image_generation.name,
            DEFAULT_IMAGE_GENERATION_MODEL
        );
    }

    #[test]
    fn thinking_level_all_variants_python_str() {
        assert_eq!(ThinkingLevel::Minimal.as_str(), "minimal");
        assert_eq!(ThinkingLevel::Low.as_str(), "low");
        assert_eq!(ThinkingLevel::Medium.as_str(), "medium");
        assert_eq!(ThinkingLevel::High.as_str(), "high");
    }

    #[test]
    fn thinking_level_all_variants_serde() {
        for (variant, expected) in [
            (ThinkingLevel::Minimal, "\"minimal\""),
            (ThinkingLevel::Low, "\"low\""),
            (ThinkingLevel::Medium, "\"medium\""),
            (ThinkingLevel::High, "\"high\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
            let parsed: ThinkingLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }
}
