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
mod tests;
