//! JSON serialization for Content values.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

use super::{
    media::MediaContent,
    types::{Content, ContentPrimitive},
};

// =============================================================================
// Python-side serialization helpers
// =============================================================================

/// Serialize a [`Content`] value into a JSON string suitable for the Python
/// chat helper script.
///
/// - `Content::Text` produces a plain JSON string (backward compatible).
/// - Media variants produce a JSON object with `"type"`, `"data"` (base64),
///   `"mime_type"`, and optional `"description"` fields.
/// - `Content::Multi` produces a JSON array of such objects.
///
/// # Errors
///
/// Returns [`Error::BackendError`] if serialization fails (should not
/// happen for well-formed content).
pub(crate) fn content_to_json(content: &Content) -> Result<String, crate::error::Error> {
    let value = content_to_value(content);
    serde_json::to_string(&value).map_err(|e| crate::error::Error::BackendError {
        message: format!("Content serialization failed: {e}"),
    })
}

/// Convert a [`Content`] to a `serde_json::Value` for the Python helper.
pub(crate) fn content_to_value(content: &Content) -> serde_json::Value {
    match content {
        Content::Text { text } => serde_json::Value::String(text.clone()),
        Content::Image(m) => typed_media_to_value(m),
        Content::Document(m) => typed_media_to_value(m),
        Content::Audio(m) => typed_media_to_value(m),
        Content::Video(m) => typed_media_to_value(m),
        Content::Multi { parts } => {
            let items: Vec<serde_json::Value> = parts.iter().map(primitive_to_value).collect();
            serde_json::Value::Array(items)
        }
    }
}

/// Convert a [`ContentPrimitive`] to a `serde_json::Value`.
fn primitive_to_value(prim: &ContentPrimitive) -> serde_json::Value {
    match prim {
        ContentPrimitive::Text { text } => serde_json::Value::String(text.clone()),
        ContentPrimitive::Image(m) => typed_media_to_value(m),
        ContentPrimitive::Document(m) => typed_media_to_value(m),
        ContentPrimitive::Audio(m) => typed_media_to_value(m),
        ContentPrimitive::Video(m) => typed_media_to_value(m),
    }
}

/// Build a JSON object for any [`MediaContent`] implementor with base64-encoded data.
fn typed_media_to_value<T: MediaContent>(media: &T) -> serde_json::Value {
    media_to_value(
        T::TYPE_NAME,
        media.data(),
        media.mime_type(),
        media.description(),
    )
}

/// Build a JSON object for a media primitive with base64-encoded data.
fn media_to_value(
    type_name: &str,
    data: &[u8],
    mime_type: &str,
    description: Option<&str>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("type".into(), serde_json::Value::String(type_name.into()));
    map.insert(
        "data".into(),
        serde_json::Value::String(BASE64.encode(data)),
    );
    map.insert(
        "mime_type".into(),
        serde_json::Value::String(mime_type.into()),
    );
    if let Some(desc) = description {
        map.insert(
            "description".into(),
            serde_json::Value::String((*desc).to_owned()),
        );
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::{
        super::media::{Audio, Document, Image},
        *,
    };

    #[test]
    fn typed_json_text_is_plain_string() {
        let content = Content::Text {
            text: "hello".to_string(),
        };
        let json = content_to_json(&content).unwrap();
        assert_eq!(json, r#""hello""#);
    }

    #[test]
    fn typed_json_image_has_base64_data() {
        let content = Content::Image(Image {
            data: vec![0x89, 0x50, 0x4E, 0x47],
            mime_type: "image/png".to_string(),
            description: Some("test".to_string()),
        });
        let json = content_to_json(&content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "Image");
        assert_eq!(parsed["mime_type"], "image/png");
        assert_eq!(parsed["description"], "test");
        // Verify base64 encoding
        let decoded = BASE64.decode(parsed["data"].as_str().unwrap()).unwrap();
        assert_eq!(decoded, vec![0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn typed_json_multi_is_array() {
        let content = Content::Multi {
            parts: vec![
                ContentPrimitive::Text {
                    text: "describe:".to_string(),
                },
                ContentPrimitive::Document(Document {
                    data: b"doc content".to_vec(),
                    mime_type: "text/plain".to_string(),
                    description: None,
                }),
            ],
        };
        let json = content_to_json(&content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str().unwrap(), "describe:");
        assert_eq!(arr[1]["type"], "Document");
    }

    #[test]
    fn typed_json_media_without_description_omits_field() {
        let content = Content::Audio(Audio {
            data: vec![1, 2, 3],
            mime_type: "audio/wav".to_string(),
            description: None,
        });
        let json = content_to_json(&content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("description").is_none());
    }
}
