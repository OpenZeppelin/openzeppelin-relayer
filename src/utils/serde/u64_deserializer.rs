//! Deserialization utilities for u64 values
//!
//! This module provides a custom deserializer for u64 values.

use std::fmt;

use serde::{de, Deserializer};

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
}

pub fn deserialize_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(U64Visitor)
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
}
