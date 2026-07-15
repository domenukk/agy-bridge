//! Content type definitions and conversions.

use serde::{Deserialize, Serialize};

use super::media::{Audio, Document, Image, Video};

// =============================================================================
// ContentPrimitive — a single content element (non-list)
// =============================================================================

/// A single content primitive within a [`Content::Multi`] list.
///
/// Mirrors the Python SDK's `ContentPrimitive = str | Image | Document | Audio | Video`.
///
/// **Why both `ContentPrimitive` and [`Content`]?**
///
/// `ContentPrimitive` represents a *single, non-compound* element —
/// it deliberately excludes the `Multi` variant that [`Content`] provides.
/// This separation enforces the invariant that multimodal lists are flat
/// (you cannot nest a `Content::Multi` inside another `Multi`), while
/// [`Content`] remains the top-level union accepted by `agent.chat()`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPrimitive {
    /// Plain text content.
    Text {
        /// The text value.
        text: String,
    },
    /// An image attachment.
    Image(Image),
    /// A document attachment.
    Document(Document),
    /// An audio attachment.
    Audio(Audio),
    /// A video attachment.
    Video(Video),
}

// =============================================================================
// Content — the top-level chat input union type
// =============================================================================

/// Chat input content, mirroring the Python SDK's
/// `Content = str | Image | Document | Audio | Video | list[ContentPrimitive]`.
///
/// This is the top-level union type accepted by [`crate::agent::AgentHandle::chat()`].
/// Unlike [`ContentPrimitive`], it includes the [`Multi`](Self::Multi) variant
/// for compound multimodal inputs. Scalar variants mirror `ContentPrimitive`
/// for convenience so callers do not have to wrap a single item in a list.
///
/// Use [`From<&str>`] or [`From<String>`] to create text content ergonomically:
/// ```rust
/// # use agy_bridge::content::Content;
/// let content: Content = "hello".into();
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Content {
    /// Plain text content (backward-compatible with `chat("hello")`).
    Text {
        /// The text value.
        text: String,
    },
    /// An image attachment.
    Image(Image),
    /// A document attachment.
    Document(Document),
    /// An audio attachment.
    Audio(Audio),
    /// A video attachment.
    Video(Video),
    /// A list of content primitives (multimodal).
    Multi {
        /// The individual content elements.
        parts: Vec<ContentPrimitive>,
    },
}

impl Content {
    /// Creates a [`Content::Text`] variant from any string-like value.
    ///
    /// This is a convenience constructor equivalent to `Content::Text { text: s.into() }`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Content;
    /// let c = Content::text("hello");
    /// assert!(c.is_text());
    /// assert_eq!(c.as_text(), Some("hello"));
    /// ```
    #[must_use]
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }

    /// Returns `true` if this content is a [`Content::Text`] variant.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::{Content, Image};
    /// assert!(Content::text("hi").is_text());
    /// assert!(!Content::Image(Image::png(vec![1])).is_text());
    /// ```
    #[must_use]
    pub const fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// Returns the text content if this is a [`Content::Text`] variant,
    /// or `None` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::{Content, Image};
    /// let text_content = Content::text("hello");
    /// assert_eq!(text_content.as_text(), Some("hello"));
    ///
    /// let image_content = Content::Image(Image::png(vec![1]));
    /// assert_eq!(image_content.as_text(), None);
    /// ```
    #[must_use]
    pub const fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text.as_str()),
            _ => None,
        }
    }
}

// =============================================================================
// Default + Display for Content
// =============================================================================

impl Default for Content {
    /// Defaults to an empty [`Content::Text`] variant.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Content;
    /// let c = Content::default();
    /// assert_eq!(c.as_text(), Some(""));
    /// ```
    fn default() -> Self {
        Self::Text {
            text: String::new(),
        }
    }
}

impl std::fmt::Display for Content {
    /// Renders a human-readable summary of the content.
    ///
    /// - `Text` → the text itself.
    /// - Media variants → `"[Image: image/png]"`, etc.
    /// - `Multi` → `"[Multi: 3 parts]"`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text { text } => f.write_str(text),
            Self::Image(m) => write!(f, "[Image: {}]", m.mime_type),
            Self::Document(m) => write!(f, "[Document: {}]", m.mime_type),
            Self::Audio(m) => write!(f, "[Audio: {}]", m.mime_type),
            Self::Video(m) => write!(f, "[Video: {}]", m.mime_type),
            Self::Multi { parts } => write!(f, "[Multi: {} parts]", parts.len()),
        }
    }
}

// =============================================================================
// Ergonomic From impls
// =============================================================================

impl From<&str> for Content {
    fn from(s: &str) -> Self {
        Self::Text { text: s.to_owned() }
    }
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        Self::Text { text: s }
    }
}

impl From<Image> for Content {
    fn from(img: Image) -> Self {
        Self::Image(img)
    }
}

impl From<Document> for Content {
    fn from(doc: Document) -> Self {
        Self::Document(doc)
    }
}

impl From<Audio> for Content {
    fn from(audio: Audio) -> Self {
        Self::Audio(audio)
    }
}

impl From<Video> for Content {
    fn from(video: Video) -> Self {
        Self::Video(video)
    }
}

impl From<Vec<ContentPrimitive>> for Content {
    fn from(parts: Vec<ContentPrimitive>) -> Self {
        Self::Multi { parts }
    }
}

impl From<ContentPrimitive> for Content {
    fn from(prim: ContentPrimitive) -> Self {
        match prim {
            ContentPrimitive::Text { text } => Self::Text { text },
            ContentPrimitive::Image(img) => Self::Image(img),
            ContentPrimitive::Document(doc) => Self::Document(doc),
            ContentPrimitive::Audio(audio) => Self::Audio(audio),
            ContentPrimitive::Video(video) => Self::Video(video),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_ref_creates_text_content() {
        let content: Content = "hello".into();
        assert_eq!(
            content,
            Content::Text {
                text: "hello".to_string()
            }
        );
    }

    #[test]
    fn from_string_creates_text_content() {
        let content: Content = String::from("world").into();
        assert_eq!(
            content,
            Content::Text {
                text: "world".to_string()
            }
        );
    }

    #[test]
    fn from_image_creates_image_content() {
        let img = Image {
            data: vec![0x89, 0x50, 0x4E, 0x47],
            mime_type: "image/png".to_string(),
            description: Some("test image".to_string()),
        };
        let content: Content = img.clone().into();
        assert_eq!(content, Content::Image(img));
    }

    #[test]
    fn from_document_creates_document_content() {
        let doc = Document {
            data: b"%PDF".to_vec(),
            mime_type: "application/pdf".to_string(),
            description: None,
        };
        let content: Content = doc.clone().into();
        assert_eq!(content, Content::Document(doc));
    }

    #[test]
    fn from_audio_creates_audio_content() {
        let audio = Audio {
            data: vec![0xFF, 0xFB],
            mime_type: "audio/mp3".to_string(),
            description: None,
        };
        let content: Content = audio.clone().into();
        assert_eq!(content, Content::Audio(audio));
    }

    #[test]
    fn from_video_creates_video_content() {
        let video = Video {
            data: vec![0x00, 0x00, 0x00, 0x1C],
            mime_type: "video/mp4".to_string(),
            description: Some("test video".to_string()),
        };
        let content: Content = video.clone().into();
        assert_eq!(content, Content::Video(video));
    }

    #[test]
    fn from_vec_creates_multi_content() {
        let parts = vec![
            ContentPrimitive::Text {
                text: "describe this:".to_string(),
            },
            ContentPrimitive::Image(Image {
                data: vec![1, 2, 3],
                mime_type: "image/png".to_string(),
                description: None,
            }),
        ];
        let content: Content = parts.clone().into();
        assert_eq!(content, Content::Multi { parts });
    }

    // ── Serde roundtrip ─────────────────────────────────────────────

    #[test]
    fn content_text_serde_roundtrip() {
        let content = Content::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: Content = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn content_image_serde_roundtrip() {
        let content = Content::Image(Image {
            data: vec![0x89, 0x50, 0x4E, 0x47],
            mime_type: "image/png".to_string(),
            description: Some("a PNG".to_string()),
        });
        let json = serde_json::to_string(&content).unwrap();
        let parsed: Content = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn content_document_serde_roundtrip() {
        let content = Content::Document(Document {
            data: b"%PDF-1.4".to_vec(),
            mime_type: "application/pdf".to_string(),
            description: None,
        });
        let json = serde_json::to_string(&content).unwrap();
        let parsed: Content = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn content_audio_serde_roundtrip() {
        let content = Content::Audio(Audio {
            data: vec![0xFF, 0xFB, 0x90],
            mime_type: "audio/mp3".to_string(),
            description: None,
        });
        let json = serde_json::to_string(&content).unwrap();
        let parsed: Content = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn content_video_serde_roundtrip() {
        let content = Content::Video(Video {
            data: vec![0x00, 0x00, 0x00, 0x1C, 0x66],
            mime_type: "video/mp4".to_string(),
            description: Some("clip".to_string()),
        });
        let json = serde_json::to_string(&content).unwrap();
        let parsed: Content = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn content_multi_serde_roundtrip() {
        let content = Content::Multi {
            parts: vec![
                ContentPrimitive::Text {
                    text: "look at this".to_string(),
                },
                ContentPrimitive::Image(Image {
                    data: vec![1, 2, 3],
                    mime_type: "image/jpeg".to_string(),
                    description: None,
                }),
            ],
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: Content = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn content_primitive_text_serde_roundtrip() {
        let prim = ContentPrimitive::Text {
            text: "hi".to_string(),
        };
        let json = serde_json::to_string(&prim).unwrap();
        let parsed: ContentPrimitive = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, prim);
    }

    #[test]
    fn content_primitive_image_serde_roundtrip() {
        let prim = ContentPrimitive::Image(Image {
            data: vec![9, 8, 7],
            mime_type: "image/webp".to_string(),
            description: Some("webp img".to_string()),
        });
        let json = serde_json::to_string(&prim).unwrap();
        let parsed: ContentPrimitive = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, prim);
    }

    #[test]
    fn content_text_creates_text_variant() {
        let c = Content::text("hello");
        assert_eq!(
            c,
            Content::Text {
                text: "hello".to_string()
            }
        );
    }

    #[test]
    fn content_text_accepts_string() {
        let c = Content::text(String::from("world"));
        assert_eq!(
            c,
            Content::Text {
                text: "world".to_string()
            }
        );
    }

    #[test]
    fn content_is_text_returns_true_for_text() {
        assert!(Content::text("hello").is_text());
    }

    #[test]
    fn content_is_text_returns_false_for_image() {
        let content = Content::Image(Image::png(vec![1]));
        assert!(!content.is_text());
    }

    #[test]
    fn content_is_text_returns_false_for_document() {
        let content = Content::Document(Document::pdf(vec![1]));
        assert!(!content.is_text());
    }

    #[test]
    fn content_is_text_returns_false_for_audio() {
        let content = Content::Audio(Audio::mp3(vec![1]));
        assert!(!content.is_text());
    }

    #[test]
    fn content_is_text_returns_false_for_video() {
        let content = Content::Video(Video::mp4(vec![1]));
        assert!(!content.is_text());
    }

    #[test]
    fn content_is_text_returns_false_for_multi() {
        let content = Content::Multi { parts: vec![] };
        assert!(!content.is_text());
    }

    #[test]
    fn content_as_text_returns_some_for_text() {
        let c = Content::text("hello");
        assert_eq!(c.as_text(), Some("hello"));
    }

    #[test]
    fn content_as_text_returns_none_for_image() {
        let c = Content::Image(Image::png(vec![1]));
        assert_eq!(c.as_text(), None);
    }

    #[test]
    fn content_as_text_returns_none_for_document() {
        let c = Content::Document(Document::pdf(vec![1]));
        assert_eq!(c.as_text(), None);
    }

    #[test]
    fn content_as_text_returns_none_for_audio() {
        let c = Content::Audio(Audio::mp3(vec![1]));
        assert_eq!(c.as_text(), None);
    }

    #[test]
    fn content_as_text_returns_none_for_video() {
        let c = Content::Video(Video::mp4(vec![1]));
        assert_eq!(c.as_text(), None);
    }

    #[test]
    fn content_as_text_returns_none_for_multi() {
        let c = Content::Multi { parts: vec![] };
        assert_eq!(c.as_text(), None);
    }

    // ── Display tests ───────────────────────────────────────────────

    #[test]
    fn display_text_renders_content() {
        let c = Content::text("hello world");
        assert_eq!(format!("{c}"), "hello world");
    }

    #[test]
    fn display_image_shows_mime_type() {
        let c = Content::Image(Image::png(vec![1]));
        assert_eq!(format!("{c}"), "[Image: image/png]");
    }

    #[test]
    fn display_document_shows_mime_type() {
        let c = Content::Document(Document::pdf(vec![1]));
        assert_eq!(format!("{c}"), "[Document: application/pdf]");
    }

    #[test]
    fn display_audio_shows_mime_type() {
        let c = Content::Audio(Audio::mp3(vec![1]));
        assert_eq!(format!("{c}"), "[Audio: audio/mpeg]");
    }

    #[test]
    fn display_video_shows_mime_type() {
        let c = Content::Video(Video::mp4(vec![1]));
        assert_eq!(format!("{c}"), "[Video: video/mp4]");
    }

    #[test]
    fn display_multi_shows_part_count() {
        let c = Content::Multi {
            parts: vec![
                ContentPrimitive::Text {
                    text: "a".to_string(),
                },
                ContentPrimitive::Text {
                    text: "b".to_string(),
                },
                ContentPrimitive::Text {
                    text: "c".to_string(),
                },
            ],
        };
        assert_eq!(format!("{c}"), "[Multi: 3 parts]");
    }

    #[test]
    fn display_empty_text() {
        let c = Content::text("");
        assert_eq!(format!("{c}"), "");
    }

    // ── From<ContentPrimitive> tests ────────────────────────────────

    #[test]
    fn from_content_primitive_text() {
        let prim = ContentPrimitive::Text {
            text: "hello".to_string(),
        };
        let content: Content = prim.into();
        assert_eq!(
            content,
            Content::Text {
                text: "hello".to_string()
            }
        );
    }

    #[test]
    fn from_content_primitive_image() {
        let prim = ContentPrimitive::Image(Image::png(vec![1, 2, 3]));
        let content: Content = prim.into();
        assert!(matches!(content, Content::Image(_)));
    }
}
