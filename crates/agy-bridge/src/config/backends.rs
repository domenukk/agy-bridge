//! Alternative model backend configurations (`0.1.24`+): Vertex AI, Local `OpenAI`, and `LiteRT`.

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use typed_builder::TypedBuilder;

use super::ModelConfig;

/// Configuration for the Google Cloud Vertex AI model backend (`VertexEndpoint`).
#[derive(Clone, Default, Serialize, Deserialize, TypedBuilder)]
#[builder(field_defaults(default))]
pub struct VertexConfig {
    /// Google Cloud project ID (standard regional auth mode).
    /// Falls back to `$GOOGLE_CLOUD_PROJECT` when `base_url` and `api_key` are unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    pub project: Option<String>,
    /// Google Cloud location / region (e.g. `"us-central1"`).
    /// Falls back to `$GOOGLE_CLOUD_LOCATION` when `base_url` and `api_key` are unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    pub location: Option<String>,
    /// Optional custom base URL for proxy or gateway routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    pub base_url: Option<String>,
    /// Custom HTTP headers sent with Vertex AI requests.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub http_headers: HashMap<String, String>,
    /// Vertex AI Express Mode API key (mutually exclusive with `project` / `location` unless `base_url` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    pub api_key: Option<String>,
    /// Per-modality model selection and generation options.
    #[serde(default)]
    pub models: ModelConfig,
}

impl std::fmt::Debug for VertexConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VertexConfig")
            .field("project", &self.project)
            .field("location", &self.location)
            .field("base_url", &self.base_url)
            .field("http_headers_len", &self.http_headers.len())
            .field("api_key", &self.api_key.as_ref().map(|_k| "[REDACTED]"))
            .field("models", &self.models)
            .finish()
    }
}

impl VertexConfig {
    /// Validate Vertex AI authentication mode (`(project && location)` XOR `api_key`, unless `base_url` is set),
    /// plus RFC-9110 `http_headers` and `base_url` hygiene.
    ///
    /// # Errors
    ///
    /// Returns an error message if headers/URLs are invalid, if both explicit `api_key`
    /// and `project`/`location` are set, or if neither authentication mode is configured.
    pub fn validate(&self, env: &HashMap<String, String>) -> Result<(), String> {
        self.validate_with_fallback(env, None)
    }

    /// Validate Vertex AI configuration with an optional top-level `AgentConfig::api_key` fallback.
    ///
    /// When `project` or `location` is explicitly configured and `self.api_key` is `None`,
    /// Standard regional mode takes precedence and `fallback_api_key` is not treated as an
    /// Express Mode conflict. When neither `project` nor `location` is set on `VertexConfig`,
    /// `fallback_api_key` satisfies Express Mode.
    ///
    /// # Errors
    ///
    /// Returns an error message if headers/URLs or authentication invariants are violated.
    pub fn validate_with_fallback(
        &self,
        env: &HashMap<String, String>,
        fallback_api_key: Option<&str>,
    ) -> Result<(), String> {
        if let Some(ref url) = self.base_url {
            super::validation::validate_http_url(url, "VertexConfig.base_url", true)?;
        }
        super::validation::validate_http_headers(&self.http_headers, "VertexConfig.http_headers")?;

        if self.base_url.as_deref().is_some_and(|u| !u.is_empty()) {
            return Ok(());
        }

        let explicit_project = self.project.as_deref().filter(|s| !s.is_empty());
        let explicit_location = self.location.as_deref().filter(|s| !s.is_empty());
        let has_explicit_regional = explicit_project.is_some() || explicit_location.is_some();
        let has_explicit_express = self.api_key.as_deref().is_some_and(|k| !k.is_empty());

        if has_explicit_regional && has_explicit_express {
            return Err(
                "Cannot specify both api_key (Express Mode) and project/location (Standard Mode) on VertexConfig"
                    .to_owned(),
            );
        }

        let has_express = has_explicit_express
            || (!has_explicit_regional && fallback_api_key.is_some_and(|k| !k.is_empty()));

        let resolved_project = explicit_project.map(str::to_owned).or_else(|| {
            (!has_express)
                .then(|| {
                    env.get("GOOGLE_CLOUD_PROJECT")
                        .cloned()
                        .or_else(|| crate::env_var("GOOGLE_CLOUD_PROJECT"))
                })
                .flatten()
                .filter(|s| !s.is_empty())
        });
        let resolved_location = explicit_location.map(str::to_owned).or_else(|| {
            (!has_express)
                .then(|| {
                    env.get("GOOGLE_CLOUD_LOCATION")
                        .cloned()
                        .or_else(|| crate::env_var("GOOGLE_CLOUD_LOCATION"))
                })
                .flatten()
                .filter(|s| !s.is_empty())
        });

        let has_any_regional = resolved_project.is_some() || resolved_location.is_some();
        let has_full_regional = resolved_project.is_some() && resolved_location.is_some();
        if has_any_regional && has_express {
            return Err(
                "Cannot specify both api_key (Express Mode) and project/location (Standard Mode) on VertexConfig"
                    .to_owned(),
            );
        }
        if !has_full_regional && !has_express {
            return Err(
                "For Vertex AI, either (project and location) or api_key (or base_url) must be set"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// Configuration for external OpenAI-compatible completions servers (Ollama, LM Studio, vLLM),
/// mirroring `google.antigravity.LocalOpenAIAgentConfig`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TypedBuilder)]
#[builder(field_defaults(default))]
pub struct LocalOpenAIConfig {
    /// Model identifier served by the local OpenAI-compatible endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    pub model: Option<String>,
    /// Base URL of the external OpenAI-compatible server (e.g. `"http://localhost:11434/v1"`).
    #[builder(setter(into))]
    pub base_url: String,
}

impl LocalOpenAIConfig {
    /// Create a [`LocalOpenAIConfig`] with a model name and server base URL.
    #[must_use]
    pub fn new(model: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            model: Some(model.into()),
            base_url: base_url.into(),
        }
    }

    /// Validate that `base_url` is non-empty and free of illegal control characters.
    ///
    /// # Errors
    ///
    /// Returns an error message when `base_url` is empty, whitespace-only, or contains C0 CTLs/DEL.
    pub fn validate(&self) -> Result<(), String> {
        super::validation::validate_http_url(&self.base_url, "LocalOpenAIConfig.base_url", false)
    }
}

/// Hardware execution backend for local LiteRT-LM models (`LiteRTBackend` in `0.1.24`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiteRTBackend {
    /// CPU execution backend.
    #[serde(alias = "CPU")]
    Cpu,
    /// GPU execution backend (the SDK default).
    #[default]
    #[serde(alias = "GPU")]
    Gpu,
    /// NPU execution backend.
    #[serde(alias = "NPU")]
    Npu,
}

impl LiteRTBackend {
    /// Lowercase wire string matching `google.antigravity.LiteRTBackend`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Npu => "npu",
        }
    }
}

impl std::fmt::Display for LiteRTBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Configuration for local Gemma / LiteRT-LM models (`LiteRTAgentConfig` in `0.1.24`).
///
/// Supported on `PythonRuntime` (requires the `litert-lm` Python package). Returns
/// [`Error::UnsupportedFeature`](crate::error::Error::UnsupportedFeature) on `NativeRuntime`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TypedBuilder)]
pub struct LiteRTConfig {
    /// Path to the `.litertlm` model file.
    #[builder(setter(into))]
    pub model_path: PathBuf,
    /// Primary hardware execution backend (`cpu`, `gpu`, `npu`).
    #[serde(default)]
    #[builder(default)]
    pub backend: LiteRTBackend,
    /// Enable speculative decoding (Multi-Token Prediction).
    #[serde(default)]
    #[builder(default)]
    pub enable_speculative_decoding: bool,
    /// Optional directory for `LiteRT` compilation caching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into, strip_option))]
    pub cache_dir: Option<PathBuf>,
    /// Optional backend override for audio processing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(strip_option))]
    pub audio_backend: Option<LiteRTBackend>,
    /// Optional backend override for vision/image processing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(strip_option))]
    pub vision_backend: Option<LiteRTBackend>,
    /// Local port to bind the managed `LiteRT` server to (`0` selects an ephemeral port).
    #[serde(default)]
    #[builder(default)]
    pub port: u16,
    /// Automatically download model weights when missing.
    #[serde(default)]
    #[builder(default)]
    pub download_if_missing: bool,
}

impl LiteRTConfig {
    /// Create a [`LiteRTConfig`] for `model_path` with SDK defaults (`LiteRTBackend::Gpu`).
    #[must_use]
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self::builder().model_path(model_path).build()
    }

    /// Validate that `model_path` is non-empty.
    ///
    /// # Errors
    ///
    /// Returns an error message when `model_path` is empty.
    pub fn validate(&self) -> Result<(), String> {
        if self.model_path.as_os_str().is_empty() {
            return Err("LiteRTConfig requires a non-empty model_path".to_owned());
        }
        Ok(())
    }
}
