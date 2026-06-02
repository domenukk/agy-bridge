//! Multimodal content types for chat input (text, image, document, audio, video).

mod serialization;
mod types;

pub mod media;

pub use media::*;
pub(crate) use serialization::content_to_json;
pub use types::*;
