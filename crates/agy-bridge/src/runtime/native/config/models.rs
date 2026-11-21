//! Model endpoint protobuf builders (`GeminiApiEndpoint`, `VertexEndpoint`, `GemmaEndpoint`).

use std::collections::HashMap;

use crate::{config::AgentConfig, proto};

pub(super) fn to_proto_gemini_options(
    gen_cfg: Option<&crate::config::GenerationConfig>,
) -> Option<proto::localharness::GeminiModelOptions> {
    let g = gen_cfg?;
    if g.thinking_level.is_none() && g.service_tier.is_none() && g.max_output_tokens.is_none() {
        return None;
    }
    let mut opts = proto::localharness::GeminiModelOptions::default();
    if let Some(t) = g.thinking_level {
        opts.thinking_level = t.as_str().to_string();
    }
    if let Some(s) = g.service_tier {
        opts.service_tier = s.as_str().to_string();
    }
    if let Some(max_tokens) = g.max_output_tokens {
        opts.max_output_tokens = max_tokens;
    }
    Some(opts)
}

fn resolve_image_model_name(
    explicit_entry_name: Option<&str>,
    caps: Option<&crate::config::CapabilitiesConfig>,
) -> String {
    if let Some(name) = explicit_entry_name
        && !name.is_empty()
        && name != crate::config::DEFAULT_IMAGE_GENERATION_MODEL
    {
        return name.to_owned();
    }
    if let Some(c) = caps
        && !c.image_model.is_empty()
        && c.image_model != crate::config::DEFAULT_IMAGE_GENERATION_MODEL
    {
        return c.image_model.clone();
    }
    explicit_entry_name.filter(|n| !n.is_empty()).map_or_else(
        || crate::config::DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
        str::to_owned,
    )
}

fn opt_string_or_empty(opt: Option<String>) -> String {
    let mut out = String::new();
    if let Some(v) = opt {
        out = v;
    }
    out
}

fn build_vertex_models_proto(
    config: &AgentConfig,
    vertex: &crate::config::VertexConfig,
) -> Vec<proto::localharness::ModelConfig> {
    let base_url = opt_string_or_empty(vertex.base_url.clone());
    let has_explicit_regional = vertex.project.as_deref().is_some_and(|s| !s.is_empty())
        || vertex.location.as_deref().is_some_and(|s| !s.is_empty());
    let express_key = opt_string_or_empty(
        vertex
            .api_key
            .clone()
            .filter(|k| !k.is_empty())
            .or_else(|| {
                (!has_explicit_regional)
                    .then(|| config.api_key.clone())
                    .flatten()
                    .filter(|k| !k.is_empty())
            }),
    );
    let use_ambient = base_url.is_empty() && express_key.is_empty();
    let project = opt_string_or_empty(vertex.project.clone().filter(|s| !s.is_empty()).or_else(
        || {
            use_ambient
                .then(|| {
                    config
                        .env
                        .get("GOOGLE_CLOUD_PROJECT")
                        .cloned()
                        .or_else(|| crate::env_var("GOOGLE_CLOUD_PROJECT"))
                })
                .flatten()
        },
    ));
    let location = opt_string_or_empty(vertex.location.clone().filter(|s| !s.is_empty()).or_else(
        || {
            use_ambient
                .then(|| {
                    config
                        .env
                        .get("GOOGLE_CLOUD_LOCATION")
                        .cloned()
                        .or_else(|| crate::env_var("GOOGLE_CLOUD_LOCATION"))
                })
                .flatten()
        },
    ));

    let text_name = if !vertex.models.default.name.is_empty()
        && vertex.models.default.name != crate::config::DEFAULT_MODEL
    {
        vertex.models.default.name.clone()
    } else if !config.model.is_empty() {
        config.model.clone()
    } else {
        crate::config::DEFAULT_MODEL.to_string()
    };
    let text_key = vertex
        .models
        .default
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| express_key.clone());
    let text_opts = to_proto_gemini_options(Some(&vertex.models.default.generation));
    let text_model = proto::localharness::ModelConfig {
        name: text_name,
        types: vec![proto::localharness::ModelType::Text as i32],
        endpoint: Some(proto::localharness::model_config::Endpoint::Vertex(
            proto::localharness::VertexEndpoint {
                base_url: base_url.clone(),
                http_headers: vertex.http_headers.clone(),
                project: project.clone(),
                location: location.clone(),
                options: text_opts,
                api_key: text_key,
            },
        )),
    };

    let image_name = resolve_image_model_name(
        Some(&vertex.models.image_generation.name),
        config.capabilities.as_ref(),
    );
    let image_key = vertex
        .models
        .image_generation
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .unwrap_or(express_key);
    let image_opts = to_proto_gemini_options(Some(&vertex.models.image_generation.generation));
    let image_model = proto::localharness::ModelConfig {
        name: image_name,
        types: vec![proto::localharness::ModelType::Image as i32],
        endpoint: Some(proto::localharness::model_config::Endpoint::Vertex(
            proto::localharness::VertexEndpoint {
                base_url,
                http_headers: vertex.http_headers.clone(),
                project,
                location,
                options: image_opts,
                api_key: image_key,
            },
        )),
    };

    vec![text_model, image_model]
}

fn build_gemini_model_config(
    name: String,
    model_type: proto::localharness::ModelType,
    base_url: String,
    http_headers: HashMap<String, String>,
    api_key: String,
    options: Option<proto::localharness::GeminiModelOptions>,
) -> proto::localharness::ModelConfig {
    proto::localharness::ModelConfig {
        name,
        types: vec![model_type as i32],
        endpoint: Some(proto::localharness::model_config::Endpoint::GeminiApi(
            proto::localharness::GeminiApiEndpoint {
                base_url,
                http_headers,
                api_key,
                options,
            },
        )),
    }
}

pub(super) fn build_models_proto(config: &AgentConfig) -> Vec<proto::localharness::ModelConfig> {
    if let Some(ref oai) = config.local_openai {
        let model_name = oai
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| config.model.clone());
        return vec![proto::localharness::ModelConfig {
            name: model_name,
            types: vec![proto::localharness::ModelType::Text as i32],
            endpoint: Some(proto::localharness::model_config::Endpoint::Gemma(
                proto::localharness::GemmaEndpoint {
                    base_url: oai.base_url.clone(),
                },
            )),
        }];
    }

    if let Some(ref vertex) = config.vertex {
        return build_vertex_models_proto(config, vertex);
    }

    let model_name = if let Some(ref g) = config.gemini
        && !g.models.default.name.is_empty()
        && (config.model == crate::config::DEFAULT_MODEL || config.model.is_empty())
    {
        g.models.default.name.clone()
    } else if !config.model.is_empty() {
        config.model.clone()
    } else {
        crate::config::DEFAULT_MODEL.to_string()
    };

    let base_url = opt_string_or_empty(config.effective_base_url());
    let http_headers = match config.gemini.as_ref() {
        Some(g) => g.http_headers.clone(),
        None => HashMap::new(),
    };
    let fallback_api_key = opt_string_or_empty(config.effective_api_key().or_else(|| {
        (!base_url.is_empty()).then(|| crate::config::PROXY_AUTH_SENTINEL.to_string())
    }));

    let text_api_key = config
        .gemini
        .as_ref()
        .and_then(|g| g.models.default.api_key.clone())
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| fallback_api_key.clone());
    let text_options =
        to_proto_gemini_options(config.gemini.as_ref().map(|g| &g.models.default.generation));
    let text_model = build_gemini_model_config(
        model_name,
        proto::localharness::ModelType::Text,
        base_url.clone(),
        http_headers.clone(),
        text_api_key,
        text_options,
    );

    let image_model_name = resolve_image_model_name(
        config
            .gemini
            .as_ref()
            .map(|g| g.models.image_generation.name.as_str()),
        config.capabilities.as_ref(),
    );
    let image_api_key = config
        .gemini
        .as_ref()
        .and_then(|g| g.models.image_generation.api_key.clone())
        .filter(|k| !k.is_empty())
        .unwrap_or(fallback_api_key);
    let image_options = to_proto_gemini_options(
        config
            .gemini
            .as_ref()
            .map(|g| &g.models.image_generation.generation),
    );
    let image_model = build_gemini_model_config(
        image_model_name,
        proto::localharness::ModelType::Image,
        base_url,
        http_headers,
        image_api_key,
        image_options,
    );

    vec![text_model, image_model]
}
