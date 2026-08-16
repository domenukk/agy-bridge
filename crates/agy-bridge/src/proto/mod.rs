//! Generated protobuf bindings for the Google Antigravity SDK local harness.

#[cfg(feature = "native")]
// NOLINT: Prost generated code cannot be modified directly
#[allow(clippy::all, clippy::pedantic, clippy::nursery)]
pub mod google {
    pub mod protobuf {
        pub use pbjson_types::*;
    }
}

#[cfg(feature = "native")]
// NOLINT: Prost generated code cannot be modified directly
#[allow(clippy::all, clippy::pedantic, clippy::nursery)]
pub mod genai {
    include!(concat!(env!("OUT_DIR"), "/genai.rs"));
    include!(concat!(env!("OUT_DIR"), "/genai.serde.rs"));
}

#[cfg(feature = "native")]
// NOLINT: Prost generated code cannot be modified directly
#[allow(clippy::all, clippy::pedantic, clippy::nursery)]
pub mod antigravity {
    pub mod localharness {
        include!(concat!(env!("OUT_DIR"), "/antigravity.localharness.rs"));
        include!(concat!(
            env!("OUT_DIR"),
            "/antigravity.localharness.serde.rs"
        ));
    }
}

#[cfg(feature = "native")]
pub use antigravity::localharness;

#[cfg(test)]
#[cfg(feature = "native")]
mod tests {
    use super::localharness::*;

    #[test]
    fn test_deserialize_init_response() {
        let json = r#"{"initializeConversationResponse":{"cascadeId":"eb92a4f4a84fac0795955a687f1d2eae"},"seqNum":1,"timestampMicros":1786720474186732}"#;
        let event: Result<OutputEvent, _> = serde_json::from_str(json);
        assert!(event.is_ok());
    }
}
