//! Deserialization utilities for u64 values
//!
//! This module provides a custom deserializer for u64 values.
//! ```
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
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(value as u64)
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
