//! Serde utilities for u64 values.
//!
//! Provides a tolerant deserializer (string / integer / float) and a
//! string serializer so large u64 values survive a Redis/Lua `cjson`
//! round-trip. See `super::safe_float` for why the float form is tolerated and
//! why values beyond 2^53 are rejected rather than silently truncated.

use std::fmt;

use serde::{de, Deserializer, Serializer};

use super::safe_float::f64_to_safe_integer;

#[derive(Debug)]
struct U64Visitor;

impl de::Visitor<'_> for U64Visitor {
    type Value = u64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string containing a u64 number or a u64 integer")
    }

    // Handle string inputs like "340282366920938463463374607431768211455"
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        value.parse::<u64>().map_err(de::Error::custom)
    }

    // Handle u64 inputs
    #[allow(clippy::unnecessary_cast)]
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(value)
    }

    // Handle i64 inputs
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value < 0 {
            Err(de::Error::custom(
                "negative value cannot be converted to u64",
            ))
        } else {
            Ok(value as u64)
        }
    }

    // Corrupted form left in Redis by a Lua cjson round-trip (e.g. 123.0).
    // Values beyond 2^53 are rejected because precision was already lost.
    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let n = f64_to_safe_integer::<E>(value)?;
        u64::try_from(n).map_err(de::Error::custom)
    }
}

pub fn deserialize_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(U64Visitor)
}

/// Serialize a u64 as a JSON string so it survives a Redis/Lua `cjson`
/// round-trip.
pub fn serialize_u64<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::{
        Error as ValueError, I64Deserializer, StringDeserializer, U64Deserializer,
    };

    #[test]
    fn test_deserialize_from_string() {
        let input = "12345";
        let deserializer = StringDeserializer::<ValueError>::new(input.to_string());
        let result = deserialize_u64(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 12345);
    }

    #[test]
    fn test_deserialize_from_string_max_u64() {
        let input = "18446744073709551615"; // u64::MAX
        let deserializer = StringDeserializer::<ValueError>::new(input.to_string());
        let result = deserialize_u64(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), u64::MAX);
    }

    #[test]
    fn test_deserialize_from_invalid_string() {
        let input = "not a number";
        let deserializer = StringDeserializer::<ValueError>::new(input.to_string());
        let result = deserialize_u64(deserializer);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_from_u64() {
        let input: u64 = 54321;
        let deserializer = U64Deserializer::<ValueError>::new(input);
        let result = deserialize_u64(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 54321);
    }

    #[test]
    fn test_deserialize_from_i64_positive() {
        let input: i64 = 9876;
        let deserializer = I64Deserializer::<ValueError>::new(input);
        let result = deserialize_u64(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 9876);
    }

    #[test]
    fn test_deserialize_from_i64_negative() {
        let input: i64 = -123;
        let deserializer = I64Deserializer::<ValueError>::new(input);
        let result = deserialize_u64(deserializer);
        assert!(result.is_err());
    }

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Wrapper {
        #[serde(serialize_with = "serialize_u64", deserialize_with = "deserialize_u64")]
        id: u64,
    }

    #[test]
    fn serializes_as_string() {
        let json = serde_json::to_string(&Wrapper {
            id: 18446744073709551615,
        })
        .unwrap();
        assert_eq!(json, r#"{"id":"18446744073709551615"}"#);
    }

    #[test]
    fn deserializes_from_string_integer_and_corrupted_float() {
        let from_str: Wrapper = serde_json::from_str(r#"{"id":"1099511627776"}"#).unwrap();
        assert_eq!(from_str.id, 1099511627776);
        let from_int: Wrapper = serde_json::from_str(r#"{"id":1099511627776}"#).unwrap();
        assert_eq!(from_int.id, 1099511627776);
        let from_float: Wrapper = serde_json::from_str(r#"{"id":1099511627776.0}"#).unwrap();
        assert_eq!(from_float.id, 1099511627776);
    }

    #[test]
    fn rejects_corrupted_float_beyond_safe_range() {
        assert!(serde_json::from_str::<Wrapper>(r#"{"id":9007199254740993.0}"#).is_err());
    }
}
