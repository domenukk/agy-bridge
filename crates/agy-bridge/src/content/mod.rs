//! Multimodal content types for chat input (text, image, document, audio, video).

#[cfg(feature = "python")]
mod serialization;
mod types;

pub mod media;

pub use media::*;
#[cfg(feature = "python")]
pub(crate) use serialization::content_to_json;
pub use types::*;
