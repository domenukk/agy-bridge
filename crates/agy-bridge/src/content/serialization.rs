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

impl<'a> MediaPayload<'a> {
    fn from_media<T: MediaContent>(media: &'a T) -> Self {
        Self {
            media_type: T::TYPE_NAME,
            data: BASE64.encode(media.data()),
            mime_type: media.mime_type(),
            description: media.description(),
        }
    }
}

/// Wire representation of a single [`ContentPrimitive`] without intermediate `Value` allocation.
#[derive(Serialize)]
#[serde(untagged)]
enum PrimitiveWire<'a> {
    Text(&'a str),
    Media(MediaPayload<'a>),
}

impl<'a> From<&'a ContentPrimitive> for PrimitiveWire<'a> {
    fn from(prim: &'a ContentPrimitive) -> Self {
        match prim {
            ContentPrimitive::Text { text } => Self::Text(text),
            ContentPrimitive::Image(m) => Self::Media(MediaPayload::from_media(m)),
            ContentPrimitive::Document(m) => Self::Media(MediaPayload::from_media(m)),
            ContentPrimitive::Audio(m) => Self::Media(MediaPayload::from_media(m)),
            ContentPrimitive::Video(m) => Self::Media(MediaPayload::from_media(m)),
        }
    }
}

/// Wire representation of [`Content`] for the Python helper script.
#[derive(Serialize)]
#[serde(untagged)]
enum ContentWire<'a> {
    Text(&'a str),
    Media(MediaPayload<'a>),
    Multi(Vec<PrimitiveWire<'a>>),
}

impl<'a> From<&'a Content> for ContentWire<'a> {
    fn from(content: &'a Content) -> Self {
        match content {
            Content::Text { text } => Self::Text(text),
            Content::Image(m) => Self::Media(MediaPayload::from_media(m)),
            Content::Document(m) => Self::Media(MediaPayload::from_media(m)),
            Content::Audio(m) => Self::Media(MediaPayload::from_media(m)),
            Content::Video(m) => Self::Media(MediaPayload::from_media(m)),
            Content::Multi { parts } => {
                Self::Multi(parts.iter().map(PrimitiveWire::from).collect())
            }
        }
    }
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
    let wire = ContentWire::from(content);
    serde_json::to_string(&wire).map_err(|e| crate::error::Error::BackendError {
        message: format!("Content JSON encoding failed: {e}"),
    })
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

    #[test]
    fn typed_json_video_has_base64_data() {
        let content = Content::Video(super::super::media::Video {
            data: vec![0x00, 0x00, 0x00, 0x1C, 0x66],
            mime_type: "video/mp4".to_string(),
            description: Some("clip".to_string()),
        });
        let json = content_to_json(&content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "Video");
        assert_eq!(parsed["mime_type"], "video/mp4");
        assert_eq!(parsed["description"], "clip");
        let decoded = BASE64.decode(parsed["data"].as_str().unwrap()).unwrap();
        assert_eq!(decoded, vec![0x00, 0x00, 0x00, 0x1C, 0x66]);
    }
}
