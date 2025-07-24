//! Deserialization utilities for u128 values
//!
//! This module provides a custom deserializer for u128 values.

use std::fmt;

use serde::{de, Deserialize, Deserializer, Serializer};

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

// Deserialize optional u128
pub fn deserialize_optional_u128<'de, D>(deserializer: D) -> Result<Option<u128>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Helper(#[serde(deserialize_with = "deserialize_u128")] u128);

    let helper = Option::<Helper>::deserialize(deserializer)?;
    Ok(helper.map(|Helper(value)| value))
}

// Serialize u128 as string
pub fn serialize_u128<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

// Serialize optional u128 as string
pub fn serialize_optional_u128<S>(value: &Option<u128>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(v) => serializer.serialize_some(&v.to_string()),
        None => serializer.serialize_none(),
    }
}

pub fn serialize_optional_u128_as_number<S>(
    value: &Option<u128>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(v) => serializer.serialize_some(&v),
        None => serializer.serialize_none(),
    }
}

/// Deserialize optional u128 from number
pub fn deserialize_optional_u128_as_number<'de, D>(
    deserializer: D,
) -> Result<Option<u128>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Option<u128> = Option::deserialize(deserializer)?;
    Ok(value)
}

/// Serialize u128 as number (non-optional)
pub fn serialize_u128_as_number<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u128(*value)
}

/// Deserialize u128 from number (non-optional)
pub fn deserialize_u128_as_number<'de, D>(deserializer: D) -> Result<u128, D::Error>
where
    D: Deserializer<'de>,
{
    u128::deserialize(deserializer)
}

// Deserialize optional u64
pub fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Helper(#[serde(deserialize_with = "deserialize_u64")] u64);

    let helper = Option::<Helper>::deserialize(deserializer)?;
    Ok(helper.map(|Helper(value)| value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::{
        Error as ValueError, I64Deserializer, StringDeserializer, U64Deserializer,
    };
    use serde_json;

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

    #[derive(Deserialize)]
    struct TestStructOptionalU128 {
        #[serde(deserialize_with = "deserialize_optional_u128")]
        value: Option<u128>,
    }

    #[test]
    fn test_deserialize_optional_u128() {
        let json = r#"{"value": "12345"}"#;
        let result: TestStructOptionalU128 = serde_json::from_str(json).unwrap();
        assert_eq!(result.value, Some(12345u128));
    }

    #[test]
    fn test_deserialize_optional_u128_none() {
        let json = r#"{"value": null}"#;
        let result: TestStructOptionalU128 = serde_json::from_str(json).unwrap();
        assert_eq!(result.value, None);
    }

    #[derive(Deserialize)]
    struct TestStructOptionalU64 {
        #[serde(deserialize_with = "deserialize_optional_u64")]
        value: Option<u64>,
    }

    #[test]
    fn test_deserialize_optional_u64() {
        let json = r#"{"value": "12345"}"#;
        let result: TestStructOptionalU64 = serde_json::from_str(json).unwrap();
        assert_eq!(result.value, Some(12345u64));
    }

    #[test]
    fn test_deserialize_optional_u64_none() {
        let json = r#"{"value": null}"#;
        let result: TestStructOptionalU64 = serde_json::from_str(json).unwrap();
        assert_eq!(result.value, None);
    }

    // Test serialization functions
    #[test]
    fn test_serialize_u128() {
        let value: u128 = 340282366920938463463374607431768211455; // u128::MAX
        let serialized = serde_json::to_string_pretty(&serde_json::json!({
            "test": serde_json::to_value(value.to_string()).unwrap()
        }))
        .unwrap();

        assert!(serialized.contains("340282366920938463463374607431768211455"));
    }

    // Test round-trip serialization/deserialization
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct TestSerializeStruct {
        #[serde(
            serialize_with = "serialize_optional_u128",
            deserialize_with = "deserialize_optional_u128"
        )]
        value: Option<u128>,
    }

    #[test]
    fn test_serialize_deserialize_roundtrip_large_value() {
        let original = TestSerializeStruct {
            value: Some(u128::MAX),
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TestSerializeStruct = serde_json::from_str(&json).unwrap();

        assert_eq!(original, deserialized);
        assert!(json.contains("340282366920938463463374607431768211455"));
    }

    #[test]
    fn test_serialize_deserialize_roundtrip_none() {
        let original = TestSerializeStruct { value: None };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TestSerializeStruct = serde_json::from_str(&json).unwrap();

        assert_eq!(original, deserialized);
        assert!(json.contains("null"));
    }

    #[test]
    fn test_serialize_deserialize_roundtrip_small_value() {
        let original = TestSerializeStruct { value: Some(12345) };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TestSerializeStruct = serde_json::from_str(&json).unwrap();

        assert_eq!(original, deserialized);
        assert!(json.contains("12345"));
    }
}
