//! Deserialization utilities for u128 values
//!
//! This module provides a custom deserializer for u128 values.

use std::fmt;

use serde::{de, Deserializer};

use super::deserialize_u64;

#[derive(Debug)]
struct U128Visitor;

impl de::Visitor<'_> for U128Visitor {
    type Value = u128;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string containing a u128 number or a u128 integer")
    }

    // Handle string inputs like "340282366920938463463374607431768211455"
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        value.parse::<u128>().map_err(de::Error::custom)
    }

    // Handle u64 inputs
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(value as u128)
    }

    // Handle i64 inputs
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value < 0 {
            Err(de::Error::custom(
                "negative value cannot be converted to u128",
            ))
        } else {
            Ok(value as u128)
        }
    }
}

pub fn deserialize_u128<'de, D>(deserializer: D) -> Result<u128, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(U128Visitor)
}

pub fn deserialize_optional_u128<'de, D>(deserializer: D) -> Result<Option<u128>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(deserialize_u128(deserializer)?))
}

pub fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(deserialize_u64(deserializer)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::{
        Error as ValueError, I64Deserializer, StringDeserializer, U64Deserializer,
    };

    #[test]
    fn test_deserialize_u128_from_string() {
        let input = "12345";
        let deserializer = StringDeserializer::<ValueError>::new(input.to_string());
        let result = deserialize_u128(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 12345);
    }

    #[test]
    fn test_deserialize_u128_from_string_large_value() {
        let input = "340282366920938463463374607431768211455"; // u128::MAX
        let deserializer = StringDeserializer::<ValueError>::new(input.to_string());
        let result = deserialize_u128(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), u128::MAX);
    }

    #[test]
    fn test_deserialize_u128_from_invalid_string() {
        let input = "not a number";
        let deserializer = StringDeserializer::<ValueError>::new(input.to_string());
        let result = deserialize_u128(deserializer);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_u128_from_u64() {
        let input: u64 = 54321;
        let deserializer = U64Deserializer::<ValueError>::new(input);
        let result = deserialize_u128(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 54321u128);
    }

    #[test]
    fn test_deserialize_u128_from_i64_positive() {
        let input: i64 = 9876;
        let deserializer = I64Deserializer::<ValueError>::new(input);
        let result = deserialize_u128(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 9876u128);
    }

    #[test]
    fn test_deserialize_u128_from_i64_negative() {
        let input: i64 = -123;
        let deserializer = I64Deserializer::<ValueError>::new(input);
        let result = deserialize_u128(deserializer);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_optional_u128() {
        let input = "12345";
        let deserializer = StringDeserializer::<ValueError>::new(input.to_string());
        let result = deserialize_optional_u128(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(12345u128));
    }

    #[test]
    fn test_deserialize_optional_u64() {
        let input = "12345";
        let deserializer = StringDeserializer::<ValueError>::new(input.to_string());
        let result = deserialize_optional_u64(deserializer);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(12345u64));
    }
}
