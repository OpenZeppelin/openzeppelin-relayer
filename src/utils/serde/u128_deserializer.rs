//! Deserialization utilities for u128 values
//!
//! This module provides a custom deserializer for u128 values.
//! ```
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
    use serde::Deserialize;
    use serde_json::from_str;

    #[test]
    fn test_deserialize_string_u128_max() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Test {
            #[serde(deserialize_with = "deserialize_u128")]
            value: u128,
        }
        let json = r#"{"value": "340282366920938463463374607431768211455"}"#;
        let result: Test = from_str(json).unwrap();
        assert_eq!(result.value, u128::MAX);
    }

    #[test]
    fn test_deserialize_numeric_u128() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Test {
            #[serde(deserialize_with = "deserialize_u128")]
            value: u128,
        }

        let json = r#"{"value": 10}"#;
        let result: Test = from_str(json).unwrap();
        assert_eq!(result.value, 10);
    }

    #[test]
    fn test_deserialize_negative_fails() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Test {
            #[serde(deserialize_with = "deserialize_u128")]
            value: u128,
        }

        let json = r#"{"value": -1}"#;
        let result = from_str::<Test>(json);
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("negative value cannot be converted to u128"));
    }

    #[test]
    fn test_deserialize_invalid_string_fails() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Test {
            #[serde(deserialize_with = "deserialize_u128")]
            value: u128,
        }

        let json = r#"{"value": "not a number"}"#;
        let result = from_str::<Test>(json);
        assert!(result.is_err());
    }
}
