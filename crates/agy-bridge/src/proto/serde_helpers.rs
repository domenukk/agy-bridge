//! `ProtoJSON` `serde` helpers for protobuf enums, 64-bit integers, numbers, and base64 bytes.

use std::{fmt, marker::PhantomData};

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, SeqAccess, Visitor},
    ser::SerializeSeq,
};

/// Returns `true` when `value` equals `T::default()`, used by `skip_serializing_if`.
pub fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Trait implemented by protobuf `#[repr(i32)]` enums for `ProtoJSON` string/integer conversion.
pub trait ProtoEnum: Copy + Sized + TryFrom<i32, Error = i32> {
    /// Canonical protobuf enum variant name (e.g. `"AGENT_BEHAVIOR_MINIMAL"`).
    fn as_str_name(&self) -> &'static str;

    /// Parses a protobuf enum variant from its canonical name.
    fn from_str_name(value: &str) -> Option<Self>;

    /// Converts this enum variant into its wire `i32` discriminant.
    fn into_i32(self) -> i32;

    /// Converts a wire `i32` discriminant into this enum variant.
    #[must_use]
    fn from_i32(value: i32) -> Option<Self> {
        match Self::try_from(value) {
            Ok(v) => Some(v),
            Err(unknown) => {
                tracing::debug!(discriminant = unknown, "unknown protobuf enum discriminant");
                None
            }
        }
    }
}

/// Defines a `#[repr(i32)]` protobuf enum and derives [`ProtoEnum`], [`TryFrom<i32>`],
/// [`serde::Serialize`], and [`serde::Deserialize`] from a single variant table.
macro_rules! define_proto_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            #[default]
            $default_var:ident = $default_tag:literal => $default_str:literal,
            $($var:ident = $tag:literal => $str_name:literal,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(i32)]
        pub enum $name {
            #[default]
            $default_var = $default_tag,
            $($var = $tag,)*
        }

        impl $name {
            /// String value of the enum field names used in the Protobuf definition.
            #[must_use]
            pub const fn as_str_name(&self) -> &'static str {
                match self {
                    Self::$default_var => $default_str,
                    $(Self::$var => $str_name,)*
                }
            }

            /// Creates an enum from field names used in the Protobuf definition.
            #[must_use]
            pub fn from_str_name(value: &str) -> Option<Self> {
                match value {
                    $default_str => Some(Self::$default_var),
                    $($str_name => Some(Self::$var),)*
                    _ => None,
                }
            }
        }

        impl TryFrom<i32> for $name {
            type Error = i32;

            fn try_from(value: i32) -> std::result::Result<Self, i32> {
                match value {
                    $default_tag => Ok(Self::$default_var),
                    $($tag => Ok(Self::$var),)*
                    other => Err(other),
                }
            }
        }

        impl $crate::proto::serde_helpers::ProtoEnum for $name {
            fn as_str_name(&self) -> &'static str {
                Self::as_str_name(self)
            }

            fn from_str_name(value: &str) -> Option<Self> {
                Self::from_str_name(value)
            }

            fn into_i32(self) -> i32 {
                self as i32
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                $crate::proto::serde_helpers::serialize_proto_enum(self, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                $crate::proto::serde_helpers::deserialize_proto_enum(deserializer)
            }
        }
    };
}

pub(crate) use define_proto_enum;

/// Serializes an enum implementing [`ProtoEnum`] as its canonical string name.
///
/// # Errors
///
/// Returns an error if the underlying serializer fails.
pub fn serialize_proto_enum<E, S>(value: &E, serializer: S) -> Result<S::Ok, S::Error>
where
    E: ProtoEnum,
    S: Serializer,
{
    serializer.serialize_str(value.as_str_name())
}

/// Deserializes an enum implementing [`ProtoEnum`] from either its canonical string name or integer tag.
///
/// # Errors
///
/// Returns an error if the input is neither a known variant string nor a valid integer tag.
pub fn deserialize_proto_enum<'de, E, D>(deserializer: D) -> Result<E, D::Error>
where
    E: ProtoEnum,
    D: Deserializer<'de>,
{
    struct EnumVisitor<E>(PhantomData<E>);

    impl<E: ProtoEnum> Visitor<'_> for EnumVisitor<E> {
        type Value = E;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a protobuf enum string name or integer tag")
        }

        fn visit_str<Err>(self, value: &str) -> Result<Self::Value, Err>
        where
            Err: de::Error,
        {
            E::from_str_name(value)
                .ok_or_else(|| Err::custom(format!("unknown protobuf enum variant: {value}")))
        }

        fn visit_i64<Err>(self, value: i64) -> Result<Self::Value, Err>
        where
            Err: de::Error,
        {
            let tag = i32::try_from(value)
                .map_err(|_err| Err::invalid_value(de::Unexpected::Signed(value), &self))?;
            E::from_i32(tag).ok_or_else(|| Err::invalid_value(de::Unexpected::Signed(value), &self))
        }

        fn visit_u64<Err>(self, value: u64) -> Result<Self::Value, Err>
        where
            Err: de::Error,
        {
            let tag = i32::try_from(value)
                .map_err(|_err| Err::invalid_value(de::Unexpected::Unsigned(value), &self))?;
            E::from_i32(tag)
                .ok_or_else(|| Err::invalid_value(de::Unexpected::Unsigned(value), &self))
        }
    }

    deserializer.deserialize_any(EnumVisitor(PhantomData))
}

/// Adapter for `i32` struct fields backed by a protobuf enum `E`.
pub struct ProtoEnumField<E>(PhantomData<E>);

impl<E: ProtoEnum> ProtoEnumField<E> {
    /// Serializes an `i32` enum field as its canonical protobuf variant string.
    ///
    /// # Errors
    ///
    /// Returns an error if `value` is not a valid discriminant of `E` or serialization fails.
    pub fn serialize<S>(value: &i32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let variant = E::from_i32(*value).ok_or_else(|| {
            serde::ser::Error::custom(format!("invalid protobuf enum discriminant: {value}"))
        })?;
        serializer.serialize_str(variant.as_str_name())
    }

    /// Deserializes an `i32` enum field from either its canonical variant string or integer tag.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialization fails.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<i32, D::Error>
    where
        D: Deserializer<'de>,
    {
        let variant: Option<E> = Option::<EnumWrapper<E>>::deserialize(deserializer)?.map(|w| w.0);
        Ok(variant.map_or(0, E::into_i32))
    }
}

/// Adapter for repeated `Vec<i32>` struct fields backed by a protobuf enum `E`.
pub struct ProtoEnumVec<E>(PhantomData<E>);

impl<E: ProtoEnum> ProtoEnumVec<E> {
    /// Serializes a slice of `i32` enum discriminants as an array of canonical variant strings.
    ///
    /// # Errors
    ///
    /// Returns an error if any element is not a valid discriminant of `E` or serialization fails.
    pub fn serialize<S>(values: &[i32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(values.len()))?;
        for &value in values {
            let variant = E::from_i32(value).ok_or_else(|| {
                serde::ser::Error::custom(format!("invalid protobuf enum discriminant: {value}"))
            })?;
            seq.serialize_element(variant.as_str_name())?;
        }
        seq.end()
    }

    /// Deserializes a `Vec<i32>` from a sequence of enum variant strings or integers.
    ///
    /// # Errors
    ///
    /// Returns an error if any element cannot be parsed as `E`.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<i32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VecVisitor<E>(PhantomData<E>);

        impl<'de, E: ProtoEnum> Visitor<'de> for VecVisitor<E> {
            type Value = Vec<i32>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence of protobuf enum values")
            }

            fn visit_unit<Err>(self) -> Result<Self::Value, Err>
            where
                Err: de::Error,
            {
                Ok(Vec::new())
            }

            fn visit_none<Err>(self) -> Result<Self::Value, Err>
            where
                Err: de::Error,
            {
                Ok(Vec::new())
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut out = match seq.size_hint() {
                    Some(cap) => Vec::with_capacity(cap),
                    None => Vec::new(),
                };
                while let Some(elem) = seq.next_element::<EnumWrapper<E>>()? {
                    out.push(elem.0.into_i32());
                }
                Ok(out)
            }
        }

        deserializer.deserialize_any(VecVisitor(PhantomData::<E>))
    }
}

struct EnumWrapper<E>(E);

impl<'de, E: ProtoEnum> Deserialize<'de> for EnumWrapper<E> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_proto_enum(deserializer).map(Self)
    }
}

/// `ProtoJSON` codec for `i64` fields (serialized as string, deserialized from string or number).
pub mod i64_str {
    use super::{Deserializer, Serializer, Visitor, de, fmt};

    /// Serializes an `i64` as a decimal string per `ProtoJSON`.
    ///
    /// # Errors
    ///
    /// Returns an error if the serializer fails.
    pub fn serialize<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    /// Deserializes an `i64` from a JSON number, string, or null.
    ///
    /// # Errors
    ///
    /// Returns an error if the value cannot be parsed as an `i64`.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<i64, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct I64Visitor;

        impl Visitor<'_> for I64Visitor {
            type Value = i64;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an i64 integer or decimal string")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(v)
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                i64::try_from(v)
                    .map_err(|_err| E::invalid_value(de::Unexpected::Unsigned(v), &self))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse::<i64>()
                    .map_err(|_err| E::invalid_value(de::Unexpected::Str(v), &self))
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(0)
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(0)
            }
        }

        deserializer.deserialize_any(I64Visitor)
    }
}

/// `ProtoJSON` codec for `u64` fields (serialized as string, deserialized from string or number).
pub mod u64_str {
    use super::{Deserializer, Serializer, Visitor, de, fmt};

    /// Serializes a `u64` as a decimal string per `ProtoJSON`.
    ///
    /// # Errors
    ///
    /// Returns an error if the serializer fails.
    pub fn serialize<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    /// Deserializes a `u64` from a JSON number, string, or null.
    ///
    /// # Errors
    ///
    /// Returns an error if the value cannot be parsed as a `u64`.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct U64Visitor;

        impl Visitor<'_> for U64Visitor {
            type Value = u64;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a u64 integer or decimal string")
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(v)
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                u64::try_from(v).map_err(|_err| E::invalid_value(de::Unexpected::Signed(v), &self))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse::<u64>()
                    .map_err(|_err| E::invalid_value(de::Unexpected::Str(v), &self))
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(0)
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(0)
            }
        }

        deserializer.deserialize_any(U64Visitor)
    }
}

/// `ProtoJSON` codec for `bytes` (`Vec<u8>`) fields using base64 encoding.
pub mod base64_bytes {
    use base64::Engine as _;

    use super::{
        Deserializer, STANDARD, STANDARD_NO_PAD, Serializer, URL_SAFE, URL_SAFE_NO_PAD, Visitor,
        de, fmt,
    };

    /// Serializes a byte slice as a standard base64 string.
    ///
    /// # Errors
    ///
    /// Returns an error if the serializer fails.
    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    /// Deserializes a `Vec<u8>` from a standard or URL-safe base64 string.
    ///
    /// # Errors
    ///
    /// Returns an error if the string is not valid base64.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BytesVisitor;

        impl Visitor<'_> for BytesVisitor {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a base64-encoded byte string")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                STANDARD
                    .decode(v)
                    .or_else(|_err| STANDARD_NO_PAD.decode(v))
                    .or_else(|_err| URL_SAFE.decode(v))
                    .or_else(|_err| URL_SAFE_NO_PAD.decode(v))
                    .map_err(|e| E::custom(format!("invalid base64 bytes: {e}")))
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                Ok(v.to_vec())
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(Vec::new())
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(Vec::new())
            }
        }

        deserializer.deserialize_any(BytesVisitor)
    }
}

/// Deserializes an `i32` from either a JSON number or string.
///
/// # Errors
///
/// Returns an error if the value cannot be parsed as `i32`.
pub fn deserialize_i32<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    struct I32Visitor;

    impl Visitor<'_> for I32Visitor {
        type Value = i32;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an i32 integer or decimal string")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            i32::try_from(v).map_err(|_err| E::invalid_value(de::Unexpected::Signed(v), &self))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            i32::try_from(v).map_err(|_err| E::invalid_value(de::Unexpected::Unsigned(v), &self))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse::<i32>()
                .map_err(|_err| E::invalid_value(de::Unexpected::Str(v), &self))
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(0)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(0)
        }
    }

    deserializer.deserialize_any(I32Visitor)
}

/// Deserializes a `u32` from either a JSON number or string.
///
/// # Errors
///
/// Returns an error if the value cannot be parsed as `u32`.
pub fn deserialize_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    struct U32Visitor;

    impl Visitor<'_> for U32Visitor {
        type Value = u32;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a u32 integer or decimal string")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            u32::try_from(v).map_err(|_err| E::invalid_value(de::Unexpected::Unsigned(v), &self))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            u32::try_from(v).map_err(|_err| E::invalid_value(de::Unexpected::Signed(v), &self))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse::<u32>()
                .map_err(|_err| E::invalid_value(de::Unexpected::Str(v), &self))
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(0)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(0)
        }
    }

    deserializer.deserialize_any(U32Visitor)
}

/// Deserializes an `f64` from either a JSON number or string (including `"NaN"`, `"Infinity"`, `"-Infinity"`).
///
/// # Errors
///
/// Returns an error if the value cannot be parsed as `f64`.
pub fn deserialize_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    struct F64Visitor;

    impl Visitor<'_> for F64Visitor {
        type Value = f64;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an f64 number or numeric string")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            v.to_string()
                .parse::<f64>()
                .map_err(|_err| E::invalid_value(de::Unexpected::Signed(v), &self))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            v.to_string()
                .parse::<f64>()
                .map_err(|_err| E::invalid_value(de::Unexpected::Unsigned(v), &self))
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(v)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse::<f64>()
                .map_err(|_err| E::invalid_value(de::Unexpected::Str(v), &self))
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(0.0)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(0.0)
        }
    }

    deserializer.deserialize_any(F64Visitor)
}

/// Codec for `f64` inside tuple enum variants or struct fields.
pub mod f64_num {
    use super::{Deserializer, Serialize, Serializer, deserialize_f64};

    /// Serializes an `f64` as a JSON number.
    ///
    /// # Errors
    ///
    /// Returns an error if the serializer fails.
    pub fn serialize<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.serialize(serializer)
    }

    /// Deserializes an `f64` from a JSON number or numeric string.
    ///
    /// # Errors
    ///
    /// Returns an error if the value cannot be parsed as `f64`.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<f64, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_f64(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    define_proto_enum! {
        pub enum SampleMode {
            #[default]
            Unspecified = 0 => "SAMPLE_MODE_UNSPECIFIED",
            Fast = 1 => "SAMPLE_MODE_FAST",
            Thorough = 2 => "SAMPLE_MODE_THOROUGH",
        }
    }

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct SampleEnvelope {
        #[serde(
            with = "ProtoEnumField::<SampleMode>",
            skip_serializing_if = "is_default"
        )]
        mode: i32,
        #[serde(
            with = "ProtoEnumVec::<SampleMode>",
            skip_serializing_if = "Vec::is_empty"
        )]
        modes: Vec<i32>,
        #[serde(with = "i64_str", skip_serializing_if = "is_default")]
        signed_64: i64,
        #[serde(with = "u64_str", skip_serializing_if = "is_default")]
        unsigned_64: u64,
        #[serde(with = "base64_bytes", skip_serializing_if = "Vec::is_empty")]
        payload: Vec<u8>,
        #[serde(deserialize_with = "deserialize_i32")]
        count_i32: i32,
        #[serde(deserialize_with = "deserialize_u32")]
        count_u32: u32,
        #[serde(with = "f64_num")]
        ratio: f64,
    }

    #[test]
    fn define_proto_enum_and_codec_roundtrip() {
        assert_eq!(SampleMode::default(), SampleMode::Unspecified);
        assert_eq!(SampleMode::Fast.as_str_name(), "SAMPLE_MODE_FAST");
        assert_eq!(
            SampleMode::from_str_name("SAMPLE_MODE_THOROUGH"),
            Some(SampleMode::Thorough)
        );
        assert_eq!(SampleMode::from_i32(1), Some(SampleMode::Fast));
        assert_eq!(SampleMode::from_i32(99), None);

        let env = SampleEnvelope {
            mode: SampleMode::Fast as i32,
            modes: vec![SampleMode::Fast as i32, SampleMode::Thorough as i32],
            signed_64: -9_007_199_254_740_993,
            unsigned_64: 18_446_744_073_709_551_610,
            payload: b"hello\x00world".to_vec(),
            count_i32: -42,
            count_u32: 42,
            ratio: 1.5,
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("\"SAMPLE_MODE_FAST\""));
        assert!(json.contains("\"-9007199254740993\""));
        assert!(json.contains("\"18446744073709551610\""));

        let decoded: SampleEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, env);
    }

    #[test]
    fn protojson_accepts_numeric_tags_string_numbers_and_url_safe_base64() {
        let raw = r#"{
            "mode": 2,
            "modes": [1, "SAMPLE_MODE_THOROUGH"],
            "signed_64": -123,
            "unsigned_64": 456,
            "payload": "aGVsbG8_d29ybGQ",
            "count_i32": "-7",
            "count_u32": "9",
            "ratio": "2.25"
        }"#;
        let decoded: SampleEnvelope = serde_json::from_str(raw).unwrap();
        assert_eq!(decoded.mode, SampleMode::Thorough as i32);
        assert_eq!(
            decoded.modes,
            vec![SampleMode::Fast as i32, SampleMode::Thorough as i32]
        );
        assert_eq!(decoded.signed_64, -123);
        assert_eq!(decoded.unsigned_64, 456);
        assert_eq!(decoded.payload, b"hello?world");
        assert_eq!(decoded.count_i32, -7);
        assert_eq!(decoded.count_u32, 9);
        assert!((decoded.ratio - 2.25).abs() < f64::EPSILON);
    }
}
