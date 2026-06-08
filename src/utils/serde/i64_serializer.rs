//! Serde helpers for i64 values stored as strings.
//!
//! Large i64 values (e.g. Stellar sequence numbers) must be serialized as JSON
//! strings so they survive a Redis/Lua `cjson` decode -> encode round-trip.
//! Redis bundles Lua 5.1, which represents every number as an IEEE-754 double,
//! so a bare integer is re-emitted as a float (e.g. `643918676885760.0`). That
//! both loses precision above 2^53 and fails to deserialize back into `i64`
//! ("invalid type: floating point ..., expected i64"). A quoted string is
//! opaque to `cjson` and round-trips untouched.
//!
//! The deserializer accepts three historical on-disk forms for backward
//! compatibility:
//! - the string form (`"643918676885760"`) written after this fix,
//! - the legacy bare integer (`643918676885760`) written before it, and
//! - the corrupted float (`643918676885760.0`) already sitting in Redis.

use std::fmt;

use serde::{de, Deserialize, Deserializer, Serializer};

use super::safe_float::f64_to_safe_integer;

#[derive(Debug)]
struct I64Visitor;

impl de::Visitor<'_> for I64Visitor {
    type Value = i64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string, integer, or float representing an i64")
    }

    // String form written after this fix: "643918676885760".
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        value.parse::<i64>().map_err(de::Error::custom)
    }

    // Legacy bare integer form: 643918676885760.
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(value)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        i64::try_from(value).map_err(de::Error::custom)
    }

    // Corrupted form left in Redis by a Lua cjson round-trip: 643918676885760.0.
    // Values beyond 2^53 are rejected (see `safe_float`) because precision was
    // already lost and the original integer cannot be recovered.
    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let n = f64_to_safe_integer::<E>(value)?;
        i64::try_from(n).map_err(de::Error::custom)
    }
}

/// Deserialize a non-optional `i64`, accepting string, integer, or float JSON
/// forms. See the module docs for why all three forms must be tolerated.
pub fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(I64Visitor)
}

/// Serialize a non-optional `i64` as a JSON string so it survives a Redis/Lua
/// `cjson` round-trip.
pub fn serialize_i64<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

/// Deserialize `Option<i64>`, accepting string, integer, or float JSON forms.
///
/// See the module docs for why all three forms must be tolerated.
pub fn deserialize_optional_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Helper(#[serde(deserialize_with = "deserialize_i64")] i64);

    let helper = Option::<Helper>::deserialize(deserializer)?;
    Ok(helper.map(|Helper(value)| value))
}

/// Serialize `Option<i64>` as a JSON string (or `null`) so it survives a
/// Redis/Lua `cjson` round-trip.
pub fn serialize_optional_i64<S>(value: &Option<i64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(v) => serializer.serialize_some(&v.to_string()),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Wrapper {
        #[serde(
            serialize_with = "serialize_optional_i64",
            deserialize_with = "deserialize_optional_i64",
            default
        )]
        seq: Option<i64>,
    }

    #[test]
    fn serializes_as_string() {
        let json = serde_json::to_string(&Wrapper {
            seq: Some(643918676885760),
        })
        .unwrap();
        assert_eq!(json, r#"{"seq":"643918676885760"}"#);
    }

    #[test]
    fn serializes_none_as_null() {
        let json = serde_json::to_string(&Wrapper { seq: None }).unwrap();
        assert_eq!(json, r#"{"seq":null}"#);
    }

    #[test]
    fn deserializes_from_string() {
        let w: Wrapper = serde_json::from_str(r#"{"seq":"643918676885760"}"#).unwrap();
        assert_eq!(w.seq, Some(643918676885760));
    }

    #[test]
    fn deserializes_from_legacy_integer() {
        let w: Wrapper = serde_json::from_str(r#"{"seq":643918676885760}"#).unwrap();
        assert_eq!(w.seq, Some(643918676885760));
    }

    #[test]
    fn deserializes_from_corrupted_float() {
        let w: Wrapper = serde_json::from_str(r#"{"seq":643918676885760.0}"#).unwrap();
        assert_eq!(w.seq, Some(643918676885760));
    }

    #[test]
    fn deserializes_negative() {
        let w: Wrapper = serde_json::from_str(r#"{"seq":-5}"#).unwrap();
        assert_eq!(w.seq, Some(-5));
        let w: Wrapper = serde_json::from_str(r#"{"seq":"-5"}"#).unwrap();
        assert_eq!(w.seq, Some(-5));
    }

    #[test]
    fn deserializes_null_and_missing() {
        let w: Wrapper = serde_json::from_str(r#"{"seq":null}"#).unwrap();
        assert_eq!(w.seq, None);
        let w: Wrapper = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(w.seq, None);
    }

    #[test]
    fn rejects_non_integral_float() {
        assert!(serde_json::from_str::<Wrapper>(r#"{"seq":1.5}"#).is_err());
    }

    #[test]
    fn rejects_float_beyond_safe_integer_range() {
        // 2^53 + 1 cannot be represented exactly; recovering it from a float
        // would silently return the wrong value, so we reject it instead.
        assert!(serde_json::from_str::<Wrapper>(r#"{"seq":9007199254740993.0}"#).is_err());
    }

    #[test]
    fn deserializes_large_string_beyond_safe_range() {
        // The going-forward string form has no precision limit.
        let w: Wrapper = serde_json::from_str(r#"{"seq":"9007199254740993"}"#).unwrap();
        assert_eq!(w.seq, Some(9007199254740993));
    }
}
