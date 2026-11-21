//! Protobuf well-known types (`google.protobuf.Duration` and `google.protobuf.NullValue`).

use std::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
};

use super::serde_helpers::ProtoEnum;

const MAX_DURATION_SECONDS: i64 = 315_576_000_000;
const MAX_DURATION_NANOS: i32 = 999_999_999;

/// A signed, fixed-length span of time (`google.protobuf.Duration`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Duration {
    /// Signed seconds of the span of time.
    pub seconds: i64,
    /// Signed fractions of a second at nanosecond resolution.
    pub nanos: i32,
}

impl Duration {
    /// Validates `seconds` bounds (`±315,576,000,000`), `nanos` bounds (`±999,999,999`),
    /// and sign consistency per the `google.protobuf.Duration` specification.
    ///
    /// # Errors
    ///
    /// Returns a descriptive error string when out of range or when `seconds` and `nanos`
    /// have opposite non-zero signs.
    pub fn validate(&self) -> Result<(), String> {
        if self.seconds < -MAX_DURATION_SECONDS || self.seconds > MAX_DURATION_SECONDS {
            return Err(format!(
                "duration seconds {} out of valid range [{}, {}]",
                self.seconds, -MAX_DURATION_SECONDS, MAX_DURATION_SECONDS
            ));
        }
        if self.nanos < -MAX_DURATION_NANOS || self.nanos > MAX_DURATION_NANOS {
            return Err(format!(
                "duration nanos {} out of valid range [{}, {}]",
                self.nanos, -MAX_DURATION_NANOS, MAX_DURATION_NANOS
            ));
        }
        if (self.seconds > 0 && self.nanos < 0) || (self.seconds < 0 && self.nanos > 0) {
            return Err(format!(
                "duration seconds ({}) and nanos ({}) must have the same sign",
                self.seconds, self.nanos
            ));
        }
        Ok(())
    }
}

impl Serialize for Duration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
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
                            seconds = crate::proto::serde_helpers::i64_str::deserialize(
                                serde::de::value::MapAccessDeserializer::new(&mut map),
                            )
                            .map_err(|e| {
                                de::Error::custom(format!("invalid duration seconds field: {e}"))
                            })?;
                        }
                        "nanos" => {
                            nanos = map.next_value()?;
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let duration = Duration { seconds, nanos };
                duration.validate().map_err(de::Error::custom)?;
                Ok(duration)
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
    if unsigned_body.is_empty() || unsigned_body == "." {
        return Err(format!("duration string has no digits: {value}"));
    }
    let (sec_str, frac_str) = match unsigned_body.split_once('.') {
        Some((s, f)) => (s, Some(f)),
        None => (unsigned_body, None),
    };
    if !sec_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid duration seconds '{sec_str}'"));
    }
    let abs_secs: u64 = if sec_str.is_empty() {
        0
    } else {
        sec_str
            .parse()
            .map_err(|e| format!("invalid duration seconds '{sec_str}': {e}"))?
    };
    let abs_nanos: u32 = if let Some(frac) = frac_str {
        if frac.is_empty() || frac.len() > 9 || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("invalid duration fractional digits: '{frac}'"));
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
    let duration = if negative {
        Duration {
            seconds: -secs_i64,
            nanos: -nanos_i32,
        }
    } else {
        Duration {
            seconds: secs_i64,
            nanos: nanos_i32,
        }
    };
    duration.validate()?;
    Ok(duration)
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
