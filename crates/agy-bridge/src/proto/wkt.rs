//! Protobuf well-known types (`google.protobuf.Duration` and `google.protobuf.NullValue`).

use std::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
};

use super::serde_helpers::ProtoEnum;

/// A signed, fixed-length span of time (`google.protobuf.Duration`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Duration {
    /// Signed seconds of the span of time.
    pub seconds: i64,
    /// Signed fractions of a second at nanosecond resolution.
    pub nanos: i32,
}

impl Serialize for Duration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.nanos == 0 {
            return serializer.serialize_str(&format!("{}s", self.seconds));
        }
        let sign = if self.seconds < 0 || self.nanos < 0 {
            "-"
        } else {
            ""
        };
        let secs = self.seconds.unsigned_abs();
        let nanos = self.nanos.unsigned_abs();
        let formatted = if nanos.is_multiple_of(1_000_000) {
            format!("{sign}{secs}.{:03}s", nanos / 1_000_000)
        } else if nanos.is_multiple_of(1_000) {
            format!("{sign}{secs}.{:06}s", nanos / 1_000)
        } else {
            format!("{sign}{secs}.{nanos:09}s")
        };
        serializer.serialize_str(&formatted)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DurationVisitor;

        impl<'de> Visitor<'de> for DurationVisitor {
            type Value = Duration;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a ProtoJSON duration string (e.g. \"1.5s\") or object")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                parse_duration_str(value).map_err(E::custom)
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut seconds = 0i64;
                let mut nanos = 0i32;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "seconds" => {
                            seconds = map.next_value()?;
                        }
                        "nanos" => {
                            nanos = map.next_value()?;
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(Duration { seconds, nanos })
            }
        }

        deserializer.deserialize_any(DurationVisitor)
    }
}

fn parse_duration_str(value: &str) -> Result<Duration, String> {
    let body = value
        .strip_suffix('s')
        .ok_or_else(|| format!("duration string must end with 's': {value}"))?;
    let (negative, unsigned_body) = if let Some(rest) = body.strip_prefix('-') {
        (true, rest)
    } else {
        (false, body)
    };
    let (sec_str, frac_str) = match unsigned_body.split_once('.') {
        Some((s, f)) => (s, Some(f)),
        None => (unsigned_body, None),
    };
    let abs_secs: u64 = if sec_str.is_empty() {
        0
    } else {
        sec_str
            .parse()
            .map_err(|e| format!("invalid duration seconds '{sec_str}': {e}"))?
    };
    let abs_nanos: u32 = if let Some(frac) = frac_str {
        if frac.len() > 9 {
            return Err(format!("duration fractional digits exceed 9: {frac}"));
        }
        let padded = format!("{frac:0<9}");
        padded
            .parse()
            .map_err(|e| format!("invalid duration nanos '{frac}': {e}"))?
    } else {
        0
    };
    let secs_i64 =
        i64::try_from(abs_secs).map_err(|e| format!("duration seconds out of range: {e}"))?;
    let nanos_i32 =
        i32::try_from(abs_nanos).map_err(|e| format!("duration nanos out of range: {e}"))?;
    if negative {
        Ok(Duration {
            seconds: -secs_i64,
            nanos: -nanos_i32,
        })
    } else {
        Ok(Duration {
            seconds: secs_i64,
            nanos: nanos_i32,
        })
    }
}

/// Singleton enum representing JSON `null` (`google.protobuf.NullValue`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(i32)]
pub enum NullValue {
    /// Null value.
    #[default]
    NullValue = 0,
}

impl NullValue {
    /// Returns the protobuf enum variant name.
    #[must_use]
    pub const fn as_str_name(&self) -> &'static str {
        "NULL_VALUE"
    }

    /// Parses a `NullValue` from its protobuf enum variant name.
    #[must_use]
    pub fn from_str_name(value: &str) -> Option<Self> {
        match value {
            "NULL_VALUE" => Some(Self::NullValue),
            _ => None,
        }
    }
}

impl TryFrom<i32> for NullValue {
    type Error = i32;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        if value == 0 {
            Ok(Self::NullValue)
        } else {
            Err(value)
        }
    }
}

impl ProtoEnum for NullValue {
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

impl Serialize for NullValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_none()
    }
}

impl<'de> Deserialize<'de> for NullValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct NullVisitor;

        impl<'de> Visitor<'de> for NullVisitor {
            type Value = NullValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("null, \"NULL_VALUE\", or 0")
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(NullValue::NullValue)
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(NullValue::NullValue)
            }

            fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
            where
                D2: Deserializer<'de>,
            {
                NullValue::deserialize(deserializer)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                NullValue::from_str_name(value)
                    .ok_or_else(|| E::custom(format!("expected NULL_VALUE, got {value}")))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                if value == 0 {
                    Ok(NullValue::NullValue)
                } else {
                    Err(E::invalid_value(de::Unexpected::Signed(value), &self))
                }
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                if value == 0 {
                    Ok(NullValue::NullValue)
                } else {
                    Err(E::invalid_value(de::Unexpected::Unsigned(value), &self))
                }
            }
        }

        deserializer.deserialize_any(NullVisitor)
    }
}
