//! Memo types and conversions for Stellar transactions

use crate::models::SignerError;
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
                let text = StringM::<28>::try_from(value.as_str()).map_err(|e| {
                    SignerError::ConversionError(format!("Invalid memo text: {}", e))
                })?;
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

        // Test Id
        let id = MemoSpec::Id { value: 12345 };
        let id_json = serde_json::to_value(&id).unwrap();
        assert_eq!(id_json, serde_json::json!({"type": "id", "value": 12345}));

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
