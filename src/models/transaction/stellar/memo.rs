//! Memo types and conversions for Stellar transactions

use crate::models::SignerError;
use serde::{Deserialize, Serialize};
use soroban_rs::xdr::{Hash, Memo, StringM};
use std::convert::TryFrom;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, PartialEq, Deserialize, ToSchema)]
#[serde(tag = "type", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MemoSpec {
    None,
    Text(String), // ≤ 28 UTF-8 bytes
    Id(u64),
    Hash(#[serde(with = "hex::serde")] [u8; 32]),
    Return(#[serde(with = "hex::serde")] [u8; 32]),
}

impl TryFrom<MemoSpec> for Memo {
    type Error = SignerError;
    fn try_from(m: MemoSpec) -> Result<Self, Self::Error> {
        Ok(match m {
            MemoSpec::None => Memo::None,
            MemoSpec::Text(s) => {
                let text = StringM::<28>::try_from(s.as_str()).map_err(|e| {
                    SignerError::ConversionError(format!("Invalid memo text: {}", e))
                })?;
                Memo::Text(text)
            }
            MemoSpec::Id(i) => Memo::Id(i),
            MemoSpec::Hash(b) => Memo::Hash(Hash(b)),
            MemoSpec::Return(b) => Memo::Return(Hash(b)),
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
        let spec = MemoSpec::Text("Hello World".to_string());
        let memo = Memo::try_from(spec).unwrap();
        assert!(matches!(memo, Memo::Text(_)));
    }

    #[test]
    fn test_memo_id() {
        let spec = MemoSpec::Id(12345);
        let memo = Memo::try_from(spec).unwrap();
        assert!(matches!(memo, Memo::Id(12345)));
    }

    #[test]
    fn test_memo_spec_serde() {
        let spec = MemoSpec::Text("hello".to_string());
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("TEXT"));
        assert!(json.contains("hello"));

        let deserialized: MemoSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deserialized);
    }
}
