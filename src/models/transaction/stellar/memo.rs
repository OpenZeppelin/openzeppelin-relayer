//! Memo types and conversions for Stellar transactions

use crate::models::SignerError;
use crate::utils::{deserialize_u64, serialize_u64};
use serde::{Deserialize, Serialize};
use soroban_rs::xdr::{Hash, Memo, StringM};
use std::convert::TryFrom;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, PartialEq, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemoSpec {
    None,
    Text {
        value: String,
    }, // ≤ 28 UTF-8 bytes
    Id {
        // Stored in Redis and re-encoded by the partial_update Lua script;
        // serialized as a string so large memo IDs survive the cjson round-trip
        // without being floatified. Legacy integer/float forms still accepted.
        // `value_type = String` keeps the OpenAPI schema in sync with the wire
        // format (utoipa ignores serialize_with).
        #[serde(serialize_with = "serialize_u64", deserialize_with = "deserialize_u64")]
        #[schema(value_type = String)]
        value: u64,
    },
    Hash {
        #[serde(with = "hex::serde")]
        value: [u8; 32],
    },
    Return {
        #[serde(with = "hex::serde")]
        value: [u8; 32],
    },
}

impl TryFrom<MemoSpec> for Memo {
    type Error = SignerError;
    fn try_from(m: MemoSpec) -> Result<Self, Self::Error> {
        Ok(match m {
            MemoSpec::None => Memo::None,
            MemoSpec::Text { value } => {
                let text = StringM::<28>::try_from(value.as_str())
                    .map_err(|e| SignerError::ConversionError(format!("Invalid memo text: {e}")))?;
                Memo::Text(text)
            }
            MemoSpec::Id { value } => Memo::Id(value),
            MemoSpec::Hash { value } => Memo::Hash(Hash(value)),
            MemoSpec::Return { value } => Memo::Return(Hash(value)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // MemoSpec::Id.value is a u64 stored in Redis and re-encoded by the
    // partial_update Lua script, so it must serialize as a cjson-safe string.
    #[test]
    fn test_memo_id_serializes_as_string() {
        let memo = MemoSpec::Id {
            value: 18446744073709551615,
        };
        let json = serde_json::to_string(&memo).unwrap();
        assert!(
            json.contains(r#""value":"18446744073709551615""#),
            "memo id should serialize as a string, got: {json}"
        );
    }

    #[test]
    fn test_memo_id_deserializes_from_corrupted_float() {
        let mut value = serde_json::to_value(MemoSpec::Id {
            value: 1099511627776,
        })
        .unwrap();
        value["value"] = serde_json::json!(1099511627776.0);
        let json = serde_json::to_string(&value).unwrap();

        let parsed: MemoSpec = serde_json::from_str(&json).unwrap();
        match parsed {
            MemoSpec::Id { value } => assert_eq!(value, 1099511627776),
            other => panic!("expected Id, got {other:?}"),
        }
    }

    #[test]
    fn test_memo_none() {
        let spec = MemoSpec::None;
        let memo = Memo::try_from(spec).unwrap();
        assert!(matches!(memo, Memo::None));
    }

    #[test]
    fn test_memo_text() {
        let spec = MemoSpec::Text {
            value: "Hello World".to_string(),
        };
        let memo = Memo::try_from(spec).unwrap();
        assert!(matches!(memo, Memo::Text(_)));
    }

    #[test]
    fn test_memo_id() {
        let spec = MemoSpec::Id { value: 12345 };
        let memo = Memo::try_from(spec).unwrap();
        assert!(matches!(memo, Memo::Id(12345)));
    }

    #[test]
    fn test_memo_spec_serde() {
        let spec = MemoSpec::Text {
            value: "hello".to_string(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("text"));
        assert!(json.contains("hello"));

        let deserialized: MemoSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn test_memo_spec_json_format() {
        // Test None
        let none = MemoSpec::None;
        let none_json = serde_json::to_value(&none).unwrap();
        assert_eq!(none_json, serde_json::json!({"type": "none"}));

        // Test Text
        let text = MemoSpec::Text {
            value: "hello".to_string(),
        };
        let text_json = serde_json::to_value(&text).unwrap();
        assert_eq!(
            text_json,
            serde_json::json!({"type": "text", "value": "hello"})
        );

        // Test Id — value is serialized as a string so large memo IDs survive
        // the Redis/Lua cjson round-trip (deserialization still accepts the
        // legacy integer form for backward compatibility).
        let id = MemoSpec::Id { value: 12345 };
        let id_json = serde_json::to_value(&id).unwrap();
        assert_eq!(id_json, serde_json::json!({"type": "id", "value": "12345"}));

        // Test Hash
        let hash = MemoSpec::Hash { value: [0x42; 32] };
        let hash_json = serde_json::to_value(&hash).unwrap();
        assert_eq!(hash_json["type"], "hash");
        assert!(hash_json["value"].is_string()); // hex encoded

        // Test Return
        let ret = MemoSpec::Return { value: [0x42; 32] };
        let ret_json = serde_json::to_value(&ret).unwrap();
        assert_eq!(ret_json["type"], "return");
        assert!(ret_json["value"].is_string()); // hex encoded
    }
}
