//! JSON serialization for Content values.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde::Serialize;

use super::{
    media::MediaContent,
    types::{Content, ContentPrimitive},
};

/// Strongly-typed payload for media content serialization.
///
/// All fields are borrowed — the only allocation is the base64 `data` string,
/// which is unavoidable.
#[derive(Serialize)]
struct MediaPayload<'a> {
    #[serde(rename = "type")]
    media_type: &'a str,
    data: String,
    mime_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

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
/// Returns [`Error::BackendError`] if serialization fails.
pub(crate) fn content_to_json(content: &Content) -> Result<String, crate::error::Error> {
    let value = content_to_value(content).map_err(|e| crate::error::Error::BackendError {
        message: format!("Content serialization failed: {e}"),
    })?;
    serde_json::to_string(&value).map_err(|e| crate::error::Error::BackendError {
        message: format!("Content JSON encoding failed: {e}"),
    })
}

/// Convert a [`Content`] to a `serde_json::Value` for the Python helper.
///
/// # Errors
///
/// Returns `serde_json::Error` if any media payload fails to serialize.
pub(crate) fn content_to_value(content: &Content) -> Result<serde_json::Value, serde_json::Error> {
    match content {
        Content::Text { text } => Ok(serde_json::Value::String(text.clone())),
        Content::Image(m) => serialize_typed_media(m),
        Content::Document(m) => serialize_typed_media(m),
        Content::Audio(m) => serialize_typed_media(m),
        Content::Video(m) => serialize_typed_media(m),
        Content::Multi { parts } => {
            let items: Result<Vec<serde_json::Value>, _> =
                parts.iter().map(serialize_primitive).collect();
            items.map(serde_json::Value::Array)
        }
    }
}

/// Convert a [`ContentPrimitive`] to a `serde_json::Value`.
///
/// # Errors
///
/// Returns `serde_json::Error` if the media payload fails to serialize.
fn serialize_primitive(prim: &ContentPrimitive) -> Result<serde_json::Value, serde_json::Error> {
    match prim {
        ContentPrimitive::Text { text } => Ok(serde_json::Value::String(text.clone())),
        ContentPrimitive::Image(m) => serialize_typed_media(m),
        ContentPrimitive::Document(m) => serialize_typed_media(m),
        ContentPrimitive::Audio(m) => serialize_typed_media(m),
        ContentPrimitive::Video(m) => serialize_typed_media(m),
    }
}

/// Serialize any [`MediaContent`] implementor to a JSON value with
/// base64-encoded data.
///
/// # Errors
///
/// Returns `serde_json::Error` if the payload fails to serialize.
fn serialize_typed_media<T: MediaContent>(
    media: &T,
) -> Result<serde_json::Value, serde_json::Error> {
    serialize_media(
        T::TYPE_NAME,
        media.data(),
        media.mime_type(),
        media.description(),
    )
}

/// Build a JSON value for a media primitive with base64-encoded data.
///
/// # Errors
///
/// Returns `serde_json::Error` if `serde_json::to_value` fails.
fn serialize_media(
    type_name: &str,
    data: &[u8],
    mime_type: &str,
    description: Option<&str>,
) -> Result<serde_json::Value, serde_json::Error> {
    let payload = MediaPayload {
        media_type: type_name,
        data: BASE64.encode(data),
        mime_type,
        description,
    };
    serde_json::to_value(&payload)
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
