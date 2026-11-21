//! Multimodal content and media messages (`genai`).

use super::{
    FileCitation, MediaResolution, PlaceCitation, ThoughtContent, ToolCallContent,
    ToolResultContent, UrlCitation,
};

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Content {
    #[serde(
        rename = "type",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub r#type: Option<content::Type>,
}
/// Nested message and enum types in `Content`.
pub mod content {
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum Type {
        Text(super::TextContent),
        Image(super::ImageContent),
        Audio(super::AudioContent),
        Document(super::DocumentContent),
        Video(super::VideoContent),
        Thought(super::ThoughtContent),
        #[serde(alias = "tool_call")]
        ToolCall(super::ToolCallContent),
        #[serde(alias = "tool_result")]
        ToolResult(super::ToolResultContent),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContentList {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub contents: Vec<Content>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WordInfo {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub text: String,
    #[serde(
        alias = "start_offset",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub start_offset: Option<crate::proto::google::protobuf::Duration>,
    #[serde(alias = "end_offset", skip_serializing_if = "Option::is_none", default)]
    pub end_offset: Option<crate::proto::google::protobuf::Duration>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub speaker: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpeechAnnotation {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub speaker: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub style: String,
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TextContent {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub annotations: Vec<text_content::Annotation>,
}
/// Nested message and enum types in `TextContent`.
pub mod text_content {
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct Annotation {
        #[serde(
            alias = "start_index",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub start_index: i32,
        #[serde(
            alias = "end_index",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub end_index: i32,
        #[serde(
            rename = "type",
            flatten,
            skip_serializing_if = "Option::is_none",
            default
        )]
        pub r#type: Option<annotation::Type>,
    }
    /// Nested message and enum types in `Annotation`.
    pub mod annotation {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum Type {
            #[serde(alias = "url_citation")]
            UrlCitation(super::super::UrlCitation),
            #[serde(alias = "file_citation")]
            FileCitation(super::super::FileCitation),
            #[serde(alias = "place_citation")]
            PlaceCitation(super::super::PlaceCitation),
            #[serde(alias = "word_info")]
            WordInfo(super::super::WordInfo),
            #[serde(alias = "speech_metadata")]
            SpeechMetadata(super::super::SpeechAnnotation),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageContent {
    #[serde(
        alias = "mime_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<image_content::MimeType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mime_type: i32,
    #[serde(
        alias = "mime_type_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub mime_type_string: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<MediaResolution>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub resolution: i32,
    #[serde(
        alias = "data_or_uri",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data_or_uri: Option<image_content::DataOrUri>,
}
/// Nested message and enum types in `ImageContent`.
pub mod image_content {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum MimeType {
            #[default]
            Unspecified = 0 => "TYPE_UNSPECIFIED",
            Png = 1 => "TYPE_PNG",
            Jpeg = 2 => "TYPE_JPEG",
            Webp = 3 => "TYPE_WEBP",
            Heic = 4 => "TYPE_HEIC",
            Heif = 5 => "TYPE_HEIF",
            Gif = 7 => "TYPE_GIF",
            Bmp = 8 => "TYPE_BMP",
            Tiff = 9 => "TYPE_TIFF",
            VideoFrameJpeg2000 = 10 => "TYPE_VIDEO_FRAME_JPEG2000",
            VideoJpeg2000 = 11 => "TYPE_VIDEO_JPEG2000",
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum DataOrUri {
        Data(#[serde(with = "crate::proto::serde_helpers::base64_bytes")] Vec<u8>),
        Uri(String),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioContent {
    #[serde(
        alias = "mime_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<audio_content::MimeType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mime_type: i32,
    #[serde(
        alias = "mime_type_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub mime_type_string: String,
    #[serde(
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub channels: i32,
    #[serde(
        alias = "sample_rate",
        deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub sample_rate: i32,
    #[serde(
        alias = "data_or_uri",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data_or_uri: Option<audio_content::DataOrUri>,
    #[serde(
        alias = "processing_oneof",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub processing_oneof: Option<audio_content::ProcessingOneof>,
}
/// Nested message and enum types in `AudioContent`.
pub mod audio_content {
    #[derive(
        Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
    )]
    #[serde(rename_all = "camelCase", default)]
    pub struct StaticMediaProcessing {
        #[serde(
            alias = "start_offset",
            skip_serializing_if = "Option::is_none",
            default
        )]
        pub start_offset: Option<crate::proto::google::protobuf::Duration>,
        #[serde(alias = "end_offset", skip_serializing_if = "Option::is_none", default)]
        pub end_offset: Option<crate::proto::google::protobuf::Duration>,
    }
    #[derive(
        Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
    )]
    #[serde(rename_all = "camelCase", default)]
    pub struct MediaProcessing {
        #[serde(
            rename = "type",
            flatten,
            skip_serializing_if = "Option::is_none",
            default
        )]
        pub r#type: Option<media_processing::Type>,
    }
    /// Nested message and enum types in `MediaProcessing`.
    pub mod media_processing {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum Type {
            Static(super::StaticMediaProcessing),
        }
    }
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum MimeType {
            #[default]
            Unspecified = 0 => "TYPE_UNSPECIFIED",
            Wav = 1 => "TYPE_WAV",
            Mp3 = 2 => "TYPE_MP3",
            Aiff = 3 => "TYPE_AIFF",
            Aac = 4 => "TYPE_AAC",
            Ogg = 5 => "TYPE_OGG",
            Flac = 6 => "TYPE_FLAC",
            Mpeg = 7 => "TYPE_MPEG",
            M4a = 8 => "TYPE_M4A",
            L16 = 9 => "TYPE_L16",
            S16le = 10 => "TYPE_S16LE",
            Opus = 11 => "TYPE_OPUS",
            Alaw = 12 => "TYPE_ALAW",
            Mulaw = 13 => "TYPE_MULAW",
            VideoAudioS16le = 14 => "TYPE_VIDEO_AUDIO_S16LE",
            Webm = 15 => "TYPE_WEBM",
        }
    }
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Processing {
            #[default]
            Unspecified = 0 => "PROCESSING_UNSPECIFIED",
            Static = 1 => "STATIC",
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum DataOrUri {
        Data(#[serde(with = "crate::proto::serde_helpers::base64_bytes")] Vec<u8>),
        Uri(String),
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum ProcessingOneof {
        #[serde(alias = "processing_type")]
        ProcessingType(
            #[serde(with = "crate::proto::serde_helpers::ProtoEnumField::<Processing>")] i32,
        ),
        #[serde(alias = "processing_config")]
        ProcessingConfig(MediaProcessing),
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DocumentContent {
    #[serde(
        alias = "mime_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<document_content::MimeType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mime_type: i32,
    #[serde(
        alias = "mime_type_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub mime_type_string: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<MediaResolution>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub resolution: i32,
    #[serde(
        alias = "data_or_uri",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data_or_uri: Option<document_content::DataOrUri>,
    #[serde(
        alias = "processing_oneof",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub processing_oneof: Option<document_content::ProcessingOneof>,
}
/// Nested message and enum types in `DocumentContent`.
pub mod document_content {
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum MimeType {
            #[default]
            Unspecified = 0 => "TYPE_UNSPECIFIED",
            Pdf = 1 => "TYPE_PDF",
            Csv = 2 => "TYPE_CSV",
            Pptx = 3 => "TYPE_PPTX",
            Ppt = 4 => "TYPE_PPT",
            Odp = 5 => "TYPE_ODP",
            Docx = 6 => "TYPE_DOCX",
            Doc = 7 => "TYPE_DOC",
            Dotx = 8 => "TYPE_DOTX",
            Odt = 9 => "TYPE_ODT",
            Xlsx = 10 => "TYPE_XLSX",
            Xls = 11 => "TYPE_XLS",
            Ods = 12 => "TYPE_ODS",
        }
    }
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Processing {
            #[default]
            Unspecified = 0 => "PROCESSING_UNSPECIFIED",
            Static = 1 => "STATIC",
        }
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct PageProcessingOptions {
        #[serde(
            alias = "start_page",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub start_page: i32,
        #[serde(
            alias = "end_page",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub end_page: i32,
        #[serde(
            alias = "content_layers",
            with = "crate::proto::serde_helpers::ProtoEnumVec::<page_processing_options::ContentLayer>",
            skip_serializing_if = "Vec::is_empty",
            default
        )]
        pub content_layers: Vec<i32>,
    }
    /// Nested message and enum types in `PageProcessingOptions`.
    pub mod page_processing_options {
        crate::proto::serde_helpers::define_proto_enum! {
            pub enum ContentLayer {
                #[default]
                Unspecified = 0 => "CONTENT_LAYER_UNSPECIFIED",
                PageImage = 1 => "PAGE_IMAGE",
                PageText = 2 => "PAGE_TEXT",
            }
        }
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct SpreadsheetProcessingOptions {
        #[serde(
            alias = "visible_cells",
            skip_serializing_if = "String::is_empty",
            default
        )]
        pub visible_cells: String,
        #[serde(
            alias = "sheet_index",
            deserialize_with = "crate::proto::serde_helpers::deserialize_i32",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub sheet_index: i32,
        #[serde(
            alias = "content_layers",
            with = "crate::proto::serde_helpers::ProtoEnumVec::<spreadsheet_processing_options::ContentLayer>",
            skip_serializing_if = "Vec::is_empty",
            default
        )]
        pub content_layers: Vec<i32>,
    }
    /// Nested message and enum types in `SpreadsheetProcessingOptions`.
    pub mod spreadsheet_processing_options {
        crate::proto::serde_helpers::define_proto_enum! {
            pub enum ContentLayer {
                #[default]
                Unspecified = 0 => "CONTENT_LAYER_UNSPECIFIED",
                Visual = 1 => "VISUAL",
                Data = 2 => "DATA",
                Formulas = 3 => "FORMULAS",
            }
        }
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct StaticMediaProcessing {
        #[serde(
            alias = "format_options",
            flatten,
            skip_serializing_if = "Option::is_none",
            default
        )]
        pub format_options: Option<static_media_processing::FormatOptions>,
    }
    /// Nested message and enum types in `StaticMediaProcessing`.
    pub mod static_media_processing {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum FormatOptions {
            #[serde(alias = "page_options")]
            PageOptions(super::PageProcessingOptions),
            #[serde(alias = "spreadsheet_options")]
            SpreadsheetOptions(super::SpreadsheetProcessingOptions),
        }
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct MediaProcessing {
        #[serde(
            rename = "type",
            flatten,
            skip_serializing_if = "Option::is_none",
            default
        )]
        pub r#type: Option<media_processing::Type>,
    }
    /// Nested message and enum types in `MediaProcessing`.
    pub mod media_processing {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum Type {
            Static(super::StaticMediaProcessing),
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum DataOrUri {
        Data(#[serde(with = "crate::proto::serde_helpers::base64_bytes")] Vec<u8>),
        Uri(String),
    }
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum ProcessingOneof {
        #[serde(alias = "processing_type")]
        ProcessingType(
            #[serde(with = "crate::proto::serde_helpers::ProtoEnumField::<Processing>")] i32,
        ),
        #[serde(alias = "processing_config")]
        ProcessingConfig(MediaProcessing),
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoContent {
    #[serde(
        alias = "mime_type",
        with = "crate::proto::serde_helpers::ProtoEnumField::<video_content::MimeType>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub mime_type: i32,
    #[serde(
        alias = "mime_type_string",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub mime_type_string: String,
    #[serde(
        with = "crate::proto::serde_helpers::ProtoEnumField::<MediaResolution>",
        skip_serializing_if = "crate::proto::serde_helpers::is_default",
        default
    )]
    pub resolution: i32,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub name: String,
    #[serde(
        alias = "data_or_uri",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data_or_uri: Option<video_content::DataOrUri>,
    #[serde(
        alias = "processing_oneof",
        flatten,
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub processing_oneof: Option<video_content::ProcessingOneof>,
}
/// Nested message and enum types in `VideoContent`.
pub mod video_content {
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct StaticMediaProcessing {
        #[serde(
            alias = "start_offset",
            skip_serializing_if = "Option::is_none",
            default
        )]
        pub start_offset: Option<crate::proto::google::protobuf::Duration>,
        #[serde(alias = "end_offset", skip_serializing_if = "Option::is_none", default)]
        pub end_offset: Option<crate::proto::google::protobuf::Duration>,
        #[serde(
            with = "crate::proto::serde_helpers::f64_num",
            skip_serializing_if = "crate::proto::serde_helpers::is_default",
            default
        )]
        pub fps: f64,
        #[serde(
            alias = "content_layers",
            with = "crate::proto::serde_helpers::ProtoEnumVec::<static_media_processing::ContentLayer>",
            skip_serializing_if = "Vec::is_empty",
            default
        )]
        pub content_layers: Vec<i32>,
    }
    /// Nested message and enum types in `StaticMediaProcessing`.
    pub mod static_media_processing {
        crate::proto::serde_helpers::define_proto_enum! {
            pub enum ContentLayer {
                #[default]
                Unspecified = 0 => "CONTENT_LAYER_UNSPECIFIED",
                Frames = 1 => "FRAMES",
                Audio = 2 => "AUDIO",
                Transcript = 3 => "TRANSCRIPT",
            }
        }
    }
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct MediaProcessing {
        #[serde(
            rename = "type",
            flatten,
            skip_serializing_if = "Option::is_none",
            default
        )]
        pub r#type: Option<media_processing::Type>,
    }
    /// Nested message and enum types in `MediaProcessing`.
    pub mod media_processing {
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum Type {
            Static(super::StaticMediaProcessing),
        }
    }
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum MimeType {
            #[default]
            Unspecified = 0 => "TYPE_UNSPECIFIED",
            TypeMp4 = 1 => "TYPE_MP4",
            Mpeg = 2 => "TYPE_MPEG",
            TypeMpg = 3 => "TYPE_MPG",
            TypeMov = 4 => "TYPE_MOV",
            TypeAvi = 5 => "TYPE_AVI",
            TypeXFlv = 6 => "TYPE_X_FLV",
            Webm = 7 => "TYPE_WEBM",
            TypeWmv = 8 => "TYPE_WMV",
            Type3gpp = 9 => "TYPE_3GPP",
            TypeJpeg2000 = 11 => "TYPE_JPEG2000",
        }
    }
    crate::proto::serde_helpers::define_proto_enum! {
        pub enum Processing {
            #[default]
            Unspecified = 0 => "PROCESSING_UNSPECIFIED",
            Static = 1 => "STATIC",
            Agentic = 2 => "AGENTIC",
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum DataOrUri {
        Data(#[serde(with = "crate::proto::serde_helpers::base64_bytes")] Vec<u8>),
        Uri(String),
    }
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub enum ProcessingOneof {
        #[serde(alias = "processing_type")]
        ProcessingType(
            #[serde(with = "crate::proto::serde_helpers::ProtoEnumField::<Processing>")] i32,
        ),
        #[serde(alias = "processing_config")]
        ProcessingConfig(MediaProcessing),
    }
}
