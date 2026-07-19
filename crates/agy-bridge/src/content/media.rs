//! Multimodal content types for chat input, mirroring the Python SDK's content
//! primitives.
//!
//! The Python SDK accepts `Content = str | Image | Document | Audio | Video |
//! list[ContentPrimitive]` as chat input. This module provides strongly-typed
//! Rust equivalents with serialization support and ergonomic `From` impls.

use serde::{Deserialize, Serialize};

// =============================================================================
// Media structs
// =============================================================================

/// Image content attachment primitive.
///
/// Binary image data with MIME type, mirroring `google.antigravity.types.Image`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// Raw image bytes (e.g. PNG, JPEG).
    pub data: Vec<u8>,
    /// MIME type of the image (e.g. `"image/png"`).
    pub mime_type: String,
    /// Optional text description of the image.
    #[serde(default)]
    pub description: Option<String>,
}

/// Document content attachment primitive.
///
/// Binary document data with MIME type, mirroring `google.antigravity.types.Document`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// Raw document bytes (e.g. PDF, JSON).
    pub data: Vec<u8>,
    /// MIME type of the document (e.g. `"application/pdf"`).
    pub mime_type: String,
    /// Optional text description of the document.
    #[serde(default)]
    pub description: Option<String>,
}

/// Audio content attachment primitive.
///
/// Binary audio data with MIME type, mirroring `google.antigravity.types.Audio`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audio {
    /// Raw audio bytes (e.g. WAV, MP3).
    pub data: Vec<u8>,
    /// MIME type of the audio (e.g. `"audio/wav"`).
    pub mime_type: String,
    /// Optional text description of the audio.
    #[serde(default)]
    pub description: Option<String>,
}

/// Video content attachment primitive.
///
/// Binary video data with MIME type, mirroring `google.antigravity.types.Video`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Video {
    /// Raw video bytes (e.g. MP4, `WebM`).
    pub data: Vec<u8>,
    /// MIME type of the video (e.g. `"video/mp4"`).
    pub mime_type: String,
    /// Optional text description of the video.
    #[serde(default)]
    pub description: Option<String>,
}

// =============================================================================
// MediaContent trait — shared interface for all media attachment types
// =============================================================================

/// Common interface for binary media attachment types ([`Image`], [`Document`],
/// [`Audio`], [`Video`]).
///
/// Introduced to reduce boilerplate in serialization helpers that previously
/// had to destructure each media struct individually.
pub trait MediaContent {
    /// The Python SDK type name used in the wire-format `"type"` field
    /// (e.g. `"Image"`, `"Audio"`).
    const TYPE_NAME: &'static str;

    /// Raw binary payload.
    fn data(&self) -> &[u8];
    /// MIME type string.
    fn mime_type(&self) -> &str;
    /// Optional human-readable description.
    fn description(&self) -> Option<&str>;
}

impl MediaContent for Image {
    const TYPE_NAME: &'static str = "Image";
    fn data(&self) -> &[u8] {
        &self.data
    }
    fn mime_type(&self) -> &str {
        &self.mime_type
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl MediaContent for Document {
    const TYPE_NAME: &'static str = "Document";
    fn data(&self) -> &[u8] {
        &self.data
    }
    fn mime_type(&self) -> &str {
        &self.mime_type
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl MediaContent for Audio {
    const TYPE_NAME: &'static str = "Audio";
    fn data(&self) -> &[u8] {
        &self.data
    }
    fn mime_type(&self) -> &str {
        &self.mime_type
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl MediaContent for Video {
    const TYPE_NAME: &'static str = "Video";
    fn data(&self) -> &[u8] {
        &self.data
    }
    fn mime_type(&self) -> &str {
        &self.mime_type
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

// =============================================================================
// MIME type constants
// =============================================================================

/// Common image MIME types.
pub mod mime {
    /// MIME type for PNG images.
    pub const IMAGE_PNG: &str = "image/png";
    /// MIME type for JPEG images.
    pub const IMAGE_JPEG: &str = "image/jpeg";
    /// MIME type for BMP images.
    pub const IMAGE_BMP: &str = "image/bmp";
    /// MIME type for WebP images.
    pub const IMAGE_WEBP: &str = "image/webp";

    /// MIME type for PDF documents.
    pub const APPLICATION_PDF: &str = "application/pdf";
    /// MIME type for plain text documents.
    pub const TEXT_PLAIN: &str = "text/plain";
    /// MIME type for JSON documents.
    pub const APPLICATION_JSON: &str = "application/json";
    /// MIME type for CSS stylesheets.
    pub const TEXT_CSS: &str = "text/css";
    /// MIME type for CSV data.
    pub const TEXT_CSV: &str = "text/csv";
    /// MIME type for HTML documents.
    pub const TEXT_HTML: &str = "text/html";
    /// MIME type for JavaScript.
    pub const TEXT_JAVASCRIPT: &str = "text/javascript";
    /// MIME type for RTF documents.
    pub const TEXT_RTF: &str = "text/rtf";
    /// MIME type for XML documents.
    pub const TEXT_XML: &str = "text/xml";

    /// MIME type for MP3 audio.
    pub const AUDIO_MPEG: &str = "audio/mpeg";
    /// MIME type for WAV audio.
    pub const AUDIO_WAV: &str = "audio/wav";
    /// MIME type for OGG audio.
    pub const AUDIO_OGG: &str = "audio/ogg";
    /// MIME type for FLAC audio.
    pub const AUDIO_FLAC: &str = "audio/flac";
    /// MIME type for AAC audio.
    pub const AUDIO_AAC: &str = "audio/aac";
    /// MIME type for Opus audio.
    pub const AUDIO_OPUS: &str = "audio/opus";
    /// MIME type for M4A audio.
    pub const AUDIO_M4A: &str = "audio/m4a";

    /// MIME type for MP4 video.
    pub const VIDEO_MP4: &str = "video/mp4";
    /// MIME type for `WebM` video.
    pub const VIDEO_WEBM: &str = "video/webm";
    /// MIME type for 3GPP video.
    pub const VIDEO_3GPP: &str = "video/3gpp";
    /// MIME type for AVI video.
    pub const VIDEO_AVI: &str = "video/avi";
    /// MIME type for MPEG video.
    pub const VIDEO_MPEG: &str = "video/mpeg";
    /// MIME type for `QuickTime` video.
    pub const VIDEO_QUICKTIME: &str = "video/quicktime";
    /// MIME type for WMV video.
    pub const VIDEO_WMV: &str = "video/wmv";
    /// MIME type for FLV video.
    pub const VIDEO_X_FLV: &str = "video/x-flv";

    /// Infer a MIME type from a file extension.
    ///
    /// Returns `None` if the extension is unrecognized.
    ///
    /// The supported set matches the Python SDK's `SUPPORTED_*_MIMES`
    /// allowlists. If the SDK adds new types, this function should be
    /// updated to match.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<&'static str> {
        match ext.to_ascii_lowercase().as_str() {
            // Images
            "png" => Some(IMAGE_PNG),
            "jpg" | "jpeg" => Some(IMAGE_JPEG),
            "bmp" => Some(IMAGE_BMP),
            "webp" => Some(IMAGE_WEBP),
            // Documents
            "pdf" => Some(APPLICATION_PDF),
            "txt" => Some(TEXT_PLAIN),
            "json" => Some(APPLICATION_JSON),
            "css" => Some(TEXT_CSS),
            "csv" => Some(TEXT_CSV),
            "html" | "htm" => Some(TEXT_HTML),
            "js" | "mjs" => Some(TEXT_JAVASCRIPT),
            "rtf" => Some(TEXT_RTF),
            "xml" => Some(TEXT_XML),
            // Audio
            "mp3" => Some(AUDIO_MPEG),
            "wav" => Some(AUDIO_WAV),
            "ogg" | "oga" => Some(AUDIO_OGG),
            "flac" => Some(AUDIO_FLAC),
            "aac" => Some(AUDIO_AAC),
            "opus" => Some(AUDIO_OPUS),
            "m4a" => Some(AUDIO_M4A),
            // Video
            "mp4" | "m4v" => Some(VIDEO_MP4),
            "webm" => Some(VIDEO_WEBM),
            "3gp" | "3gpp" => Some(VIDEO_3GPP),
            "avi" => Some(VIDEO_AVI),
            "mpeg" | "mpg" => Some(VIDEO_MPEG),
            "mov" => Some(VIDEO_QUICKTIME),
            "wmv" => Some(VIDEO_WMV),
            "flv" => Some(VIDEO_X_FLV),
            _ => None,
        }
    }
}

/// Shared implementation for loading media from a file path.
///
/// Extracts the file extension, looks up the MIME type, validates the prefix,
/// reads the file, and returns `(data, mime_type)`.
fn from_file_inner(
    path: &std::path::Path,
    type_label: &str,
    mime_prefixes: &[&str],
) -> std::io::Result<(Vec<u8>, String)> {
    let ext = path.extension().and_then(|e| e.to_str()).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing file extension")
    })?;
    let mime_type = mime::from_extension(ext).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unrecognized {type_label} extension: {ext}"),
        )
    })?;
    if !mime_prefixes
        .iter()
        .any(|prefix| mime_type.starts_with(prefix))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("MIME type '{mime_type}' is not {type_label} type"),
        ));
    }
    let data = std::fs::read(path)?;
    Ok((data, mime_type.to_owned()))
}

// =============================================================================
// Media convenience constructors
// =============================================================================

impl Image {
    /// Creates a new [`Image`] with the given data and MIME type.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Image;
    /// let img = Image::new(vec![0x89, 0x50], "image/png");
    /// assert_eq!(img.mime_type, "image/png");
    /// assert_eq!(img.data, vec![0x89, 0x50]);
    /// assert!(img.description.is_none());
    /// ```
    pub fn new(data: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self {
            data,
            mime_type: mime_type.into(),
            description: None,
        }
    }

    /// Creates a new [`Image`] with MIME type `image/png`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Image;
    /// let img = Image::png(vec![1, 2, 3]);
    /// assert_eq!(img.mime_type, "image/png");
    /// assert_eq!(img.data, vec![1, 2, 3]);
    /// ```
    #[must_use]
    pub fn png(data: Vec<u8>) -> Self {
        Self::new(data, mime::IMAGE_PNG)
    }

    /// Creates a new [`Image`] with MIME type `image/jpeg`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Image;
    /// let img = Image::jpeg(vec![0xFF, 0xD8]);
    /// assert_eq!(img.mime_type, "image/jpeg");
    /// ```
    #[must_use]
    pub fn jpeg(data: Vec<u8>) -> Self {
        Self::new(data, mime::IMAGE_JPEG)
    }

    /// Creates a new [`Image`] with MIME type `image/webp`.
    #[must_use]
    pub fn webp(data: Vec<u8>) -> Self {
        Self::new(data, mime::IMAGE_WEBP)
    }

    /// Creates a new [`Image`] with MIME type `image/bmp`.
    #[must_use]
    pub fn bmp(data: Vec<u8>) -> Self {
        Self::new(data, mime::IMAGE_BMP)
    }

    /// Sets a description on this image, consuming and returning `self`.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Load an image from a file path, inferring the MIME type from the extension.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the file cannot be read or the extension
    /// is unrecognized.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let (data, mime_type) = from_file_inner(path.as_ref(), "an image", &["image/"])?;
        Ok(Self::new(data, mime_type))
    }
}

impl Document {
    /// Creates a new [`Document`] with the given data and MIME type.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Document;
    /// let doc = Document::new(b"%PDF".to_vec(), "application/pdf");
    /// assert_eq!(doc.mime_type, "application/pdf");
    /// assert!(doc.description.is_none());
    /// ```
    pub fn new(data: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self {
            data,
            mime_type: mime_type.into(),
            description: None,
        }
    }

    /// Creates a new [`Document`] with MIME type `application/pdf`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Document;
    /// let doc = Document::pdf(b"%PDF-1.4".to_vec());
    /// assert_eq!(doc.mime_type, "application/pdf");
    /// ```
    #[must_use]
    pub fn pdf(data: Vec<u8>) -> Self {
        Self::new(data, mime::APPLICATION_PDF)
    }

    /// Creates a new [`Document`] with MIME type `text/plain`.
    #[must_use]
    pub fn plain_text(data: Vec<u8>) -> Self {
        Self::new(data, mime::TEXT_PLAIN)
    }

    /// Creates a new [`Document`] with MIME type `application/json`.
    #[must_use]
    pub fn json(data: Vec<u8>) -> Self {
        Self::new(data, mime::APPLICATION_JSON)
    }

    /// Sets a description on this document, consuming and returning `self`.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Load a document from a file path, inferring the MIME type from the extension.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the file cannot be read or the extension
    /// is unrecognized.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let (data, mime_type) =
            from_file_inner(path.as_ref(), "a document", &["application/", "text/"])?;
        Ok(Self::new(data, mime_type))
    }
}

impl Audio {
    /// Creates a new [`Audio`] with the given data and MIME type.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Audio;
    /// let audio = Audio::new(vec![0xFF, 0xFB], "audio/mpeg");
    /// assert_eq!(audio.mime_type, "audio/mpeg");
    /// assert!(audio.description.is_none());
    /// ```
    pub fn new(data: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self {
            data,
            mime_type: mime_type.into(),
            description: None,
        }
    }

    /// Creates a new [`Audio`] with MIME type `audio/mpeg` (MP3).
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Audio;
    /// let audio = Audio::mp3(vec![0xFF, 0xFB]);
    /// assert_eq!(audio.mime_type, "audio/mpeg");
    /// ```
    #[must_use]
    pub fn mp3(data: Vec<u8>) -> Self {
        Self::new(data, mime::AUDIO_MPEG)
    }

    /// Creates a new [`Audio`] with MIME type `audio/wav`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Audio;
    /// let audio = Audio::wav(vec![0x52, 0x49, 0x46, 0x46]);
    /// assert_eq!(audio.mime_type, "audio/wav");
    /// ```
    #[must_use]
    pub fn wav(data: Vec<u8>) -> Self {
        Self::new(data, mime::AUDIO_WAV)
    }

    /// Creates a new [`Audio`] with MIME type `audio/ogg`.
    #[must_use]
    pub fn ogg(data: Vec<u8>) -> Self {
        Self::new(data, mime::AUDIO_OGG)
    }

    /// Creates a new [`Audio`] with MIME type `audio/flac`.
    #[must_use]
    pub fn flac(data: Vec<u8>) -> Self {
        Self::new(data, mime::AUDIO_FLAC)
    }

    /// Sets a description on this audio, consuming and returning `self`.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Load audio from a file path, inferring the MIME type from the extension.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the file cannot be read or the extension
    /// is unrecognized.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let (data, mime_type) = from_file_inner(path.as_ref(), "an audio", &["audio/"])?;
        Ok(Self::new(data, mime_type))
    }
}

impl Video {
    /// Creates a new [`Video`] with the given data and MIME type.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Video;
    /// let video = Video::new(vec![0x00, 0x00], "video/mp4");
    /// assert_eq!(video.mime_type, "video/mp4");
    /// assert!(video.description.is_none());
    /// ```
    pub fn new(data: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self {
            data,
            mime_type: mime_type.into(),
            description: None,
        }
    }

    /// Creates a new [`Video`] with MIME type `video/mp4`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use agy_bridge::content::Video;
    /// let video = Video::mp4(vec![0x00, 0x00, 0x00, 0x1C]);
    /// assert_eq!(video.mime_type, "video/mp4");
    /// ```
    #[must_use]
    pub fn mp4(data: Vec<u8>) -> Self {
        Self::new(data, mime::VIDEO_MP4)
    }

    /// Creates a new [`Video`] with MIME type `video/webm`.
    #[must_use]
    pub fn webm(data: Vec<u8>) -> Self {
        Self::new(data, mime::VIDEO_WEBM)
    }

    /// Sets a description on this video, consuming and returning `self`.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Load video from a file path, inferring the MIME type from the extension.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the file cannot be read or the extension
    /// is unrecognized.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let (data, mime_type) = from_file_inner(path.as_ref(), "a video", &["video/"])?;
        Ok(Self::new(data, mime_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_struct_serde_roundtrip() {
        let img = Image {
            data: vec![10, 20, 30],
            mime_type: "image/bmp".to_string(),
            description: Some("bitmap".to_string()),
        };
        let json = serde_json::to_string(&img).unwrap();
        let parsed: Image = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, img);
    }

    #[test]
    fn document_struct_serde_roundtrip() {
        let doc = Document {
            data: b"{}".to_vec(),
            mime_type: "application/json".to_string(),
            description: None,
        };
        let json = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, doc);
    }

    #[test]
    fn audio_struct_serde_roundtrip() {
        let audio = Audio {
            data: vec![0xAA, 0xBB],
            mime_type: "audio/wav".to_string(),
            description: Some("beep".to_string()),
        };
        let json = serde_json::to_string(&audio).unwrap();
        let parsed: Audio = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, audio);
    }

    #[test]
    fn video_struct_serde_roundtrip() {
        let video = Video {
            data: vec![0xCC, 0xDD, 0xEE],
            mime_type: "video/webm".to_string(),
            description: None,
        };
        let json = serde_json::to_string(&video).unwrap();
        let parsed: Video = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, video);
    }

    #[test]
    fn image_description_defaults_to_none() {
        let json = r#"{"data":[1,2,3],"mime_type":"image/png"}"#;
        let img: Image = serde_json::from_str(json).unwrap();
        assert!(img.description.is_none());
    }

    #[test]
    fn image_new_creates_correct_image() {
        let img = Image::new(vec![10, 20], "image/webp");
        assert_eq!(img.data, vec![10, 20]);
        assert_eq!(img.mime_type, "image/webp");
        assert!(img.description.is_none());
    }

    #[test]
    fn image_png_creates_correct_image() {
        let img = Image::png(vec![1, 2, 3]);
        assert_eq!(img.data, vec![1, 2, 3]);
        assert_eq!(img.mime_type, "image/png");
        assert!(img.description.is_none());
    }

    #[test]
    fn image_jpeg_creates_correct_image() {
        let img = Image::jpeg(vec![0xFF, 0xD8]);
        assert_eq!(img.data, vec![0xFF, 0xD8]);
        assert_eq!(img.mime_type, "image/jpeg");
        assert!(img.description.is_none());
    }

    #[test]
    fn document_new_creates_correct_document() {
        let doc = Document::new(b"data".to_vec(), "text/plain");
        assert_eq!(doc.data, b"data".to_vec());
        assert_eq!(doc.mime_type, "text/plain");
        assert!(doc.description.is_none());
    }

    #[test]
    fn document_pdf_creates_correct_document() {
        let doc = Document::pdf(b"%PDF-1.4".to_vec());
        assert_eq!(doc.data, b"%PDF-1.4".to_vec());
        assert_eq!(doc.mime_type, "application/pdf");
        assert!(doc.description.is_none());
    }

    #[test]
    fn audio_new_creates_correct_audio() {
        let audio = Audio::new(vec![0xAA], "audio/ogg");
        assert_eq!(audio.data, vec![0xAA]);
        assert_eq!(audio.mime_type, "audio/ogg");
        assert!(audio.description.is_none());
    }

    #[test]
    fn audio_mp3_creates_correct_audio() {
        let audio = Audio::mp3(vec![0xFF, 0xFB]);
        assert_eq!(audio.data, vec![0xFF, 0xFB]);
        assert_eq!(audio.mime_type, "audio/mpeg");
        assert!(audio.description.is_none());
    }

    #[test]
    fn audio_wav_creates_correct_audio() {
        let audio = Audio::wav(vec![0x52, 0x49, 0x46, 0x46]);
        assert_eq!(audio.data, vec![0x52, 0x49, 0x46, 0x46]);
        assert_eq!(audio.mime_type, "audio/wav");
        assert!(audio.description.is_none());
    }

    #[test]
    fn video_new_creates_correct_video() {
        let video = Video::new(vec![0x00], "video/webm");
        assert_eq!(video.data, vec![0x00]);
        assert_eq!(video.mime_type, "video/webm");
        assert!(video.description.is_none());
    }

    #[test]
    fn video_mp4_creates_correct_video() {
        let video = Video::mp4(vec![0x00, 0x00, 0x00, 0x1C]);
        assert_eq!(video.data, vec![0x00, 0x00, 0x00, 0x1C]);
        assert_eq!(video.mime_type, "video/mp4");
        assert!(video.description.is_none());
    }

    #[test]
    fn image_new_accepts_string_type() {
        let img = Image::new(vec![1], String::from("image/bmp"));
        assert_eq!(img.mime_type, "image/bmp");
    }

    // ── with_description() builder ──────────────────────────────────

    #[test]
    fn image_with_description_sets_description() {
        let img = Image::png(vec![1]).with_description("a logo");
        assert_eq!(img.description.as_deref(), Some("a logo"));
        assert_eq!(img.mime_type, "image/png");
    }

    #[test]
    fn document_with_description_sets_description() {
        let doc = Document::pdf(vec![1]).with_description("invoice");
        assert_eq!(doc.description.as_deref(), Some("invoice"));
        assert_eq!(doc.mime_type, "application/pdf");
    }

    #[test]
    fn audio_with_description_sets_description() {
        let audio = Audio::mp3(vec![1]).with_description("intro jingle");
        assert_eq!(audio.description.as_deref(), Some("intro jingle"));
        assert_eq!(audio.mime_type, "audio/mpeg");
    }

    #[test]
    fn video_with_description_sets_description() {
        let video = Video::mp4(vec![1]).with_description("demo clip");
        assert_eq!(video.description.as_deref(), Some("demo clip"));
        assert_eq!(video.mime_type, "video/mp4");
    }

    // ── Convenience constructors ────────────────────────────────────

    #[test]
    fn image_webp_creates_correct_image() {
        let img = Image::webp(vec![1, 2]);
        assert_eq!(img.mime_type, "image/webp");
        assert_eq!(img.data, vec![1, 2]);
        assert!(img.description.is_none());
    }

    #[test]
    fn image_bmp_creates_correct_image() {
        let img = Image::bmp(vec![0x42, 0x4D]);
        assert_eq!(img.mime_type, "image/bmp");
        assert_eq!(img.data, vec![0x42, 0x4D]);
        assert!(img.description.is_none());
    }

    #[test]
    fn convenience_constructors_use_sdk_allowed_mimes() {
        // Mirrors google.antigravity.types SUPPORTED_*_MIMES (the source of
        // truth). If the SDK allowlist changes, update these arrays and the
        // constructors together.
        const SDK_IMAGE: &[&str] = &["image/bmp", "image/jpeg", "image/png", "image/webp"];
        const SDK_DOCUMENT: &[&str] = &[
            "application/pdf",
            "application/json",
            "text/css",
            "text/csv",
            "text/html",
            "text/javascript",
            "text/plain",
            "text/rtf",
            "text/xml",
        ];
        const SDK_AUDIO: &[&str] = &[
            "audio/wav",
            "audio/mp3",
            "audio/aac",
            "audio/ogg",
            "audio/flac",
            "audio/opus",
            "audio/mpeg",
            "audio/m4a",
            "audio/l16",
        ];
        const SDK_VIDEO: &[&str] = &[
            "video/3gpp",
            "video/avi",
            "video/mp4",
            "video/mpeg",
            "video/mpg",
            "video/quicktime",
            "video/webm",
            "video/wmv",
            "video/x-flv",
        ];

        let d = vec![0u8];
        for m in [
            Image::png(d.clone()).mime_type,
            Image::jpeg(d.clone()).mime_type,
            Image::webp(d.clone()).mime_type,
            Image::bmp(d.clone()).mime_type,
        ] {
            assert!(
                SDK_IMAGE.contains(&m.as_str()),
                "image constructor mime {m} not in SDK allowlist"
            );
        }
        for m in [
            Document::pdf(d.clone()).mime_type,
            Document::json(d.clone()).mime_type,
            Document::plain_text(d.clone()).mime_type,
        ] {
            assert!(
                SDK_DOCUMENT.contains(&m.as_str()),
                "document constructor mime {m} not in SDK allowlist"
            );
        }
        for m in [
            Audio::mp3(d.clone()).mime_type,
            Audio::wav(d.clone()).mime_type,
            Audio::ogg(d.clone()).mime_type,
            Audio::flac(d.clone()).mime_type,
        ] {
            assert!(
                SDK_AUDIO.contains(&m.as_str()),
                "audio constructor mime {m} not in SDK allowlist"
            );
        }
        for m in [
            Video::mp4(d.clone()).mime_type,
            Video::webm(d.clone()).mime_type,
        ] {
            assert!(
                SDK_VIDEO.contains(&m.as_str()),
                "video constructor mime {m} not in SDK allowlist"
            );
        }

        // Every extension the bridge infers must map to an SDK-allowed MIME.
        for ext in [
            "png", "jpg", "jpeg", "bmp", "webp", "pdf", "txt", "json", "css", "csv", "html", "htm",
            "js", "mjs", "rtf", "xml", "mp3", "wav", "ogg", "oga", "flac", "aac", "opus", "m4a",
            "mp4", "m4v", "webm", "3gp", "3gpp", "avi", "mpeg", "mpg", "mov", "wmv", "flv",
        ] {
            let mime = mime::from_extension(ext).expect("known extension");
            let allowed = SDK_IMAGE.contains(&mime)
                || SDK_DOCUMENT.contains(&mime)
                || SDK_AUDIO.contains(&mime)
                || SDK_VIDEO.contains(&mime);
            assert!(allowed, "extension .{ext} maps to non-SDK mime {mime}");
        }
    }

    #[test]
    fn audio_ogg_creates_correct_audio() {
        let audio = Audio::ogg(vec![0x4F, 0x67]);
        assert_eq!(audio.mime_type, "audio/ogg");
        assert_eq!(audio.data, vec![0x4F, 0x67]);
        assert!(audio.description.is_none());
    }

    #[test]
    fn audio_flac_creates_correct_audio() {
        let audio = Audio::flac(vec![0x66, 0x4C]);
        assert_eq!(audio.mime_type, "audio/flac");
        assert_eq!(audio.data, vec![0x66, 0x4C]);
        assert!(audio.description.is_none());
    }

    #[test]
    fn document_plain_text_creates_correct_document() {
        let doc = Document::plain_text(b"hello".to_vec());
        assert_eq!(doc.mime_type, "text/plain");
        assert_eq!(doc.data, b"hello");
        assert!(doc.description.is_none());
    }

    #[test]
    fn document_json_creates_correct_document() {
        let doc = Document::json(b"{}".to_vec());
        assert_eq!(doc.mime_type, "application/json");
        assert_eq!(doc.data, b"{}");
        assert!(doc.description.is_none());
    }

    #[test]
    fn video_webm_creates_correct_video() {
        let video = Video::webm(vec![0x1A, 0x45]);
        assert_eq!(video.mime_type, "video/webm");
        assert_eq!(video.data, vec![0x1A, 0x45]);
        assert!(video.description.is_none());
    }

    // ── from_file() — Image ────────────────────────────────────────

    #[test]
    fn image_from_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        std::fs::write(&path, b"\x89PNG").unwrap();
        let img = Image::from_file(&path).unwrap();
        assert_eq!(img.data, b"\x89PNG");
        assert_eq!(img.mime_type, "image/png");
        assert!(img.description.is_none());
    }

    #[test]
    fn image_from_file_unknown_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.tiff");
        std::fs::write(&path, b"II").unwrap();
        let err = Image::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("unrecognized"),
            "expected 'unrecognized' in: {err}"
        );
    }

    #[test]
    fn image_from_file_wrong_mime_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_image.mp3");
        std::fs::write(&path, b"\xFF\xFB").unwrap();
        let err = Image::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not an image type"),
            "expected MIME prefix error in: {err}"
        );
    }

    #[test]
    fn image_from_file_missing_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noext");
        std::fs::write(&path, b"data").unwrap();
        let err = Image::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("missing file extension"),
            "expected 'missing file extension' in: {err}"
        );
    }

    // ── from_file() — Document ─────────────────────────────────────

    #[test]
    fn document_from_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"%PDF-1.4").unwrap();
        let doc = Document::from_file(&path).unwrap();
        assert_eq!(doc.data, b"%PDF-1.4");
        assert_eq!(doc.mime_type, "application/pdf");
        assert!(doc.description.is_none());
    }

    #[test]
    fn document_from_file_text_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"hello world").unwrap();
        let doc = Document::from_file(&path).unwrap();
        assert_eq!(doc.data, b"hello world");
        assert_eq!(doc.mime_type, "text/plain");
    }

    #[test]
    fn document_from_file_json_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, b"{}").unwrap();
        let doc = Document::from_file(&path).unwrap();
        assert_eq!(doc.data, b"{}");
        assert_eq!(doc.mime_type, "application/json");
    }

    #[test]
    fn document_from_file_unknown_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.xyz");
        std::fs::write(&path, b"stuff").unwrap();
        let err = Document::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("unrecognized"),
            "expected 'unrecognized' in: {err}"
        );
    }

    #[test]
    fn document_from_file_wrong_mime_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_doc.png");
        std::fs::write(&path, b"\x89PNG").unwrap();
        let err = Document::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not a document type"),
            "expected MIME prefix error in: {err}"
        );
    }

    #[test]
    fn document_from_file_missing_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noext");
        std::fs::write(&path, b"data").unwrap();
        let err = Document::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("missing file extension"),
            "expected 'missing file extension' in: {err}"
        );
    }

    // ── from_file() — Audio ────────────────────────────────────────

    #[test]
    fn audio_from_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp3");
        std::fs::write(&path, b"\xFF\xFB\x90").unwrap();
        let audio = Audio::from_file(&path).unwrap();
        assert_eq!(audio.data, b"\xFF\xFB\x90");
        assert_eq!(audio.mime_type, "audio/mpeg");
        assert!(audio.description.is_none());
    }

    #[test]
    fn audio_from_file_wav_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.wav");
        std::fs::write(&path, b"RIFF").unwrap();
        let audio = Audio::from_file(&path).unwrap();
        assert_eq!(audio.mime_type, "audio/wav");
    }

    #[test]
    fn audio_from_file_unknown_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sound.mid");
        std::fs::write(&path, b"data").unwrap();
        let err = Audio::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("unrecognized"),
            "expected 'unrecognized' in: {err}"
        );
    }

    #[test]
    fn audio_from_file_wrong_mime_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_audio.png");
        std::fs::write(&path, b"\x89PNG").unwrap();
        let err = Audio::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not an audio type"),
            "expected MIME prefix error in: {err}"
        );
    }

    #[test]
    fn audio_from_file_missing_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noext");
        std::fs::write(&path, b"data").unwrap();
        let err = Audio::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("missing file extension"),
            "expected 'missing file extension' in: {err}"
        );
    }

    // ── from_file() — Video ────────────────────────────────────────

    #[test]
    fn video_from_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, b"\x00\x00\x00\x1Cftyp").unwrap();
        let video = Video::from_file(&path).unwrap();
        assert_eq!(video.data, b"\x00\x00\x00\x1Cftyp");
        assert_eq!(video.mime_type, "video/mp4");
        assert!(video.description.is_none());
    }

    #[test]
    fn video_from_file_webm_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.webm");
        std::fs::write(&path, b"\x1A\x45\xDF\xA3").unwrap();
        let video = Video::from_file(&path).unwrap();
        assert_eq!(video.mime_type, "video/webm");
    }

    #[test]
    fn video_from_file_unknown_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mkv");
        std::fs::write(&path, b"RIFF").unwrap();
        let err = Video::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("unrecognized"),
            "expected 'unrecognized' in: {err}"
        );
    }

    #[test]
    fn video_from_file_wrong_mime_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_video.png");
        std::fs::write(&path, b"\x89PNG").unwrap();
        let err = Video::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not a video type"),
            "expected MIME prefix error in: {err}"
        );
    }

    #[test]
    fn video_from_file_missing_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noext");
        std::fs::write(&path, b"data").unwrap();
        let err = Video::from_file(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("missing file extension"),
            "expected 'missing file extension' in: {err}"
        );
    }

    // ── from_file() — file does not exist ──────────────────────────

    #[test]
    fn image_from_file_nonexistent_file() {
        let err = Image::from_file("/tmp/agy_bridge_test_nonexistent_8f3a.png").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn document_from_file_nonexistent_file() {
        let err = Document::from_file("/tmp/agy_bridge_test_nonexistent_8f3a.pdf").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn audio_from_file_nonexistent_file() {
        let err = Audio::from_file("/tmp/agy_bridge_test_nonexistent_8f3a.mp3").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn video_from_file_nonexistent_file() {
        let err = Video::from_file("/tmp/agy_bridge_test_nonexistent_8f3a.mp4").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    // ── mime::from_extension ────────────────────────────────────────

    #[test]
    fn mime_from_extension_case_insensitive() {
        assert_eq!(mime::from_extension("PNG"), Some("image/png"));
        assert_eq!(mime::from_extension("Jpeg"), Some("image/jpeg"));
        assert_eq!(mime::from_extension("MP4"), Some("video/mp4"));
    }

    #[test]
    fn mime_from_extension_unknown_returns_none() {
        assert_eq!(mime::from_extension("tiff"), None);
        assert_eq!(mime::from_extension("tga"), None);
        assert_eq!(mime::from_extension(""), None);
    }
}
