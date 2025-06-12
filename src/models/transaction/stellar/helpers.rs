//! Helper functions for Stellar transaction processing

use crate::models::SignerError;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use soroban_rs::xdr::{
    AccountId, BytesM, Hash, PublicKey as XdrPublicKey, ScAddress, ScBytes, ScString, ScVal, ScVec,
    StringM, Uint256, VecM,
};
use stellar_strkey::ed25519::PublicKey;
use utoipa::ToSchema;

/// Converts client-friendly JSON values to ScVal types for Soroban contract function arguments
///
/// Supports JSON format like:
/// - `{"U64": 1000}` -> `ScVal::U64(1000)`
/// - `{"Address": "GA..."}` -> `ScVal::Address(ScAddress::Account(...))`
/// - `{"String": "hello"}` -> `ScVal::String(...)`
/// - `{"Bool": true}` -> `ScVal::Bool(true)`
/// - `{"Bytes": "hex_string"}` -> `ScVal::Bytes(...)`
/// - `{"Vec": [{"U32": 1}, {"U32": 2}]}` -> `ScVal::Vec(...)`
pub fn json_to_scval(json: &serde_json::Value) -> Result<ScVal, SignerError> {
    match json {
        serde_json::Value::Object(map) if map.len() == 1 => {
            let (key, value) = map.iter().next().unwrap();
            match key.as_str() {
                "U64" => {
                    let val = value.as_u64().ok_or_else(|| {
                        SignerError::ConversionError("U64 value must be a valid u64".into())
                    })?;
                    Ok(ScVal::U64(val))
                }
                "I64" => {
                    let val = value.as_i64().ok_or_else(|| {
                        SignerError::ConversionError("I64 value must be a valid i64".into())
                    })?;
                    Ok(ScVal::I64(val))
                }
                "U32" => {
                    let val = value.as_u64().ok_or_else(|| {
                        SignerError::ConversionError("U32 value must be a valid number".into())
                    })? as u32;
                    Ok(ScVal::U32(val))
                }
                "I32" => {
                    let val = value.as_i64().ok_or_else(|| {
                        SignerError::ConversionError("I32 value must be a valid number".into())
                    })? as i32;
                    Ok(ScVal::I32(val))
                }
                "Bool" => {
                    let val = value.as_bool().ok_or_else(|| {
                        SignerError::ConversionError("Bool value must be true or false".into())
                    })?;
                    Ok(ScVal::Bool(val))
                }
                "String" => {
                    let val = value.as_str().ok_or_else(|| {
                        SignerError::ConversionError("String value must be a valid string".into())
                    })?;
                    let string_m = StringM::<{ u32::MAX }>::try_from(val).map_err(|e| {
                        SignerError::ConversionError(format!("Failed to convert string: {}", e))
                    })?;
                    Ok(ScVal::String(ScString::from(string_m)))
                }
                "Address" => {
                    let addr_str = value.as_str().ok_or_else(|| {
                        SignerError::ConversionError("Address value must be a string".into())
                    })?;

                    // Try to parse as account address (G...)
                    if let Ok(pk) = PublicKey::from_string(addr_str) {
                        let uint256 = Uint256(pk.0);
                        let xdr_pk = XdrPublicKey::PublicKeyTypeEd25519(uint256);
                        let account_id = AccountId(xdr_pk);
                        return Ok(ScVal::Address(ScAddress::Account(account_id)));
                    }

                    // Try to parse as contract address (C...)
                    if let Ok(contract) = stellar_strkey::Contract::from_string(addr_str) {
                        let hash = Hash(contract.0);
                        return Ok(ScVal::Address(ScAddress::Contract(hash)));
                    }

                    Err(SignerError::ConversionError(format!(
                        "Invalid address format: {}",
                        addr_str
                    )))
                }
                "Bytes" => {
                    let hex_str = value.as_str().ok_or_else(|| {
                        SignerError::ConversionError("Bytes value must be a hex string".into())
                    })?;
                    let bytes = hex::decode(hex_str).map_err(|e| {
                        SignerError::ConversionError(format!("Invalid hex string: {}", e))
                    })?;
                    let bytes_m = BytesM::<{ u32::MAX }>::try_from(bytes).map_err(|e| {
                        SignerError::ConversionError(format!("Failed to convert bytes: {}", e))
                    })?;
                    Ok(ScVal::Bytes(ScBytes::from(bytes_m)))
                }
                "Vec" => {
                    let array = value.as_array().ok_or_else(|| {
                        SignerError::ConversionError("Vec value must be an array".into())
                    })?;
                    let mut scvals = Vec::new();
                    for item in array {
                        scvals.push(json_to_scval(item)?);
                    }
                    let vec_m = VecM::try_from(scvals).map_err(|e| {
                        SignerError::ConversionError(format!("Failed to convert vector: {}", e))
                    })?;
                    Ok(ScVal::Vec(Some(ScVec::from(vec_m))))
                }
                _ => Err(SignerError::ConversionError(format!(
                    "Unsupported ScVal type: {}",
                    key
                ))),
            }
        }
        _ => Err(SignerError::ConversionError(
            "JSON must be an object with a single key-value pair representing the ScVal type"
                .into(),
        )),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TimeBoundsSpec {
    pub min_time: u64,
    pub max_time: u64,
}

pub fn valid_until_to_time_bounds(valid_until: Option<String>) -> Option<TimeBoundsSpec> {
    valid_until.and_then(|expiry| {
        if let Ok(expiry_time) = expiry.parse::<u64>() {
            Some(TimeBoundsSpec {
                min_time: 0,
                max_time: expiry_time,
            })
        } else if let Ok(dt) = DateTime::parse_from_rfc3339(&expiry) {
            Some(TimeBoundsSpec {
                min_time: 0,
                max_time: dt.timestamp() as u64,
            })
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_u64_conversion() {
        let json = json!({"U64": 1000});
        let scval = json_to_scval(&json).unwrap();
        assert_eq!(scval, ScVal::U64(1000));
    }

    #[test]
    fn test_i64_conversion() {
        let json = json!({"I64": -1000});
        let scval = json_to_scval(&json).unwrap();
        assert_eq!(scval, ScVal::I64(-1000));
    }

    #[test]
    fn test_string_conversion() {
        let json = json!({"String": "hello world"});
        let scval = json_to_scval(&json).unwrap();
        if let ScVal::String(sc_string) = scval {
            assert_eq!(sc_string.to_utf8_string_lossy(), "hello world");
        } else {
            panic!("Expected ScVal::String");
        }
    }

    #[test]
    fn test_vec_conversion() {
        let json = json!({"Vec": [{"U32": 1}, {"U32": 2}, {"Bool": true}]});
        let scval = json_to_scval(&json).unwrap();
        if let ScVal::Vec(Some(sc_vec)) = scval {
            let items: Vec<ScVal> = sc_vec.0.to_vec();
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], ScVal::U32(1));
            assert_eq!(items[1], ScVal::U32(2));
            assert_eq!(items[2], ScVal::Bool(true));
        } else {
            panic!("Expected ScVal::Vec");
        }
    }

    #[test]
    fn test_invalid_json_format() {
        let json = json!({"U64": 1000, "String": "test"});
        assert!(json_to_scval(&json).is_err());
    }

    #[test]
    fn test_numeric_string() {
        let tb = valid_until_to_time_bounds(Some("12345".to_string())).unwrap();
        assert_eq!(tb.max_time, 12_345);
        assert_eq!(tb.min_time, 0);
    }

    #[test]
    fn test_rfc3339_string() {
        let tb = valid_until_to_time_bounds(Some("2025-01-01T00:00:00Z".to_string())).unwrap();
        assert_eq!(tb.max_time, 1_735_689_600);
        assert_eq!(tb.min_time, 0);
    }

    #[test]
    fn test_invalid_string() {
        assert!(valid_until_to_time_bounds(Some("not a date".to_string())).is_none());
    }

    #[test]
    fn test_none() {
        assert!(valid_until_to_time_bounds(None).is_none());
    }
}
