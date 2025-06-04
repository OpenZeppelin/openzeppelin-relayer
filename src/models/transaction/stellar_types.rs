//! Defines types and conversions for representing Stellar transactions and related structures.
//!
//! This module provides Rust representations for Stellar XDR types like `Memo`, `Asset`,
//! `Operation`, and `Transaction`, along with `serde` serialization/deserialization
//! and `TryFrom` implementations for converting between custom spec types (e.g., `MemoSpec`)
//! and the underlying XDR types. It also includes helper functions and unit tests.

use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
use crate::models::transaction::repository::StellarTransactionData;
use crate::models::SignerError;
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use soroban_rs::xdr::{
    AccountId, AlphaNum12, AlphaNum4, Asset, AssetCode12, AssetCode4, Hash, Memo,
    MuxedAccount as XdrMuxedAccount, MuxedAccountMed25519, Operation, OperationBody, PaymentOp,
    Preconditions, PublicKey as XdrPublicKey, SequenceNumber, StringM, TimeBounds, TimePoint,
    Transaction, TransactionExt, Uint256, VecM,
};
use std::convert::TryFrom;
use std::str::FromStr;
use stellar_strkey::ed25519::MuxedAccount;
use stellar_strkey::ed25519::PublicKey;
use utoipa::ToSchema;

pub type DecoratedSignature = soroban_rs::xdr::DecoratedSignature;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AssetSpec {
    Native,
    Credit4 { code: String, issuer: String },
    Credit12 { code: String, issuer: String },
}

impl TryFrom<AssetSpec> for Asset {
    type Error = SignerError;
    fn try_from(a: AssetSpec) -> Result<Self, Self::Error> {
        Ok(match a {
            AssetSpec::Native => Asset::Native,
            AssetSpec::Credit4 { code, issuer } => {
                let b = code.as_bytes();
                if !(1..=4).contains(&b.len()) {
                    return Err(SignerError::ConversionError("asset code 1-4 chars".into()));
                }
                let mut buf = [0u8; 4];
                buf[..b.len()].copy_from_slice(b);

                let issuer_pk = PublicKey::from_str(&issuer)
                    .map_err(|e| SignerError::ConversionError(format!("Invalid issuer: {}", e)))?;

                let uint256 = Uint256(issuer_pk.0);
                let pk = XdrPublicKey::PublicKeyTypeEd25519(uint256);
                let account_id = AccountId(pk);

                Asset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(buf),
                    issuer: account_id,
                })
            }
            AssetSpec::Credit12 { code, issuer } => {
                let b = code.as_bytes();
                if !(5..=12).contains(&b.len()) {
                    return Err(SignerError::ConversionError("asset code 5-12 chars".into()));
                }
                let mut buf = [0u8; 12];
                buf[..b.len()].copy_from_slice(b);

                let issuer_pk = PublicKey::from_str(&issuer)
                    .map_err(|e| SignerError::ConversionError(format!("Invalid issuer: {}", e)))?;

                let uint256 = Uint256(issuer_pk.0);
                let pk = XdrPublicKey::PublicKeyTypeEd25519(uint256);
                let account_id = AccountId(pk);

                Asset::CreditAlphanum12(AlphaNum12 {
                    asset_code: AssetCode12(buf),
                    issuer: account_id,
                })
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OperationSpec {
    Payment {
        destination: String,
        amount: i64,
        asset: AssetSpec,
    },
}

impl TryFrom<OperationSpec> for Operation {
    type Error = SignerError;

    fn try_from(op: OperationSpec) -> Result<Self, Self::Error> {
        match op {
            OperationSpec::Payment {
                destination,
                amount,
                asset,
            } => {
                let dest = if let Ok(m) = MuxedAccount::from_string(&destination) {
                    // accept M... muxed accounts
                    XdrMuxedAccount::MuxedEd25519(MuxedAccountMed25519 {
                        id: m.id,
                        ed25519: Uint256(m.ed25519),
                    })
                } else {
                    // fall-back to plain G... public key
                    let pk = PublicKey::from_string(&destination).map_err(|e| {
                        SignerError::ConversionError(format!("Invalid destination: {}", e))
                    })?;
                    XdrMuxedAccount::Ed25519(Uint256(pk.0))
                };

                Ok(Operation {
                    source_account: None,
                    body: OperationBody::Payment(PaymentOp {
                        destination: dest,
                        asset: asset.try_into()?,
                        amount,
                    }),
                })
            }
        }
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

impl TryFrom<StellarTransactionData> for Transaction {
    type Error = SignerError;

    fn try_from(data: StellarTransactionData) -> Result<Self, Self::Error> {
        let operations: Result<Vec<Operation>, SignerError> = data
            .operations
            .iter()
            .map(|op| Operation::try_from(op.clone()))
            .collect();
        let operations: VecM<Operation, 100> = operations?
            .try_into()
            .map_err(|_| SignerError::ConversionError("op count > 100".into()))?;

        let time_bounds = valid_until_to_time_bounds(data.valid_until);
        let cond = match time_bounds {
            None => Preconditions::None,
            Some(tb) => Preconditions::Time(TimeBounds {
                min_time: TimePoint(tb.min_time),
                max_time: TimePoint(tb.max_time),
            }),
        };

        let memo = match &data.memo {
            Some(memo_spec) => Memo::try_from(memo_spec.clone())?,
            None => Memo::None,
        };

        let fee = data.fee.unwrap_or(STELLAR_DEFAULT_TRANSACTION_FEE);
        let sequence = data.sequence_number.unwrap_or(0);

        let source_account = {
            let addr = &data.source_account;
            if let Ok(m) = MuxedAccount::from_string(addr) {
                Ok::<_, SignerError>(XdrMuxedAccount::MuxedEd25519(MuxedAccountMed25519 {
                    id: m.id,
                    ed25519: Uint256(m.ed25519),
                }))
            } else {
                let pk = PublicKey::from_string(addr).map_err(|e| {
                    SignerError::ConversionError(format!("Invalid source account: {}", e))
                })?;
                Ok::<_, SignerError>(XdrMuxedAccount::Ed25519(Uint256(pk.0)))
            }
        }?;

        Ok(Transaction {
            source_account,
            fee,
            seq_num: SequenceNumber(sequence),
            cond,
            memo,
            operations,
            ext: TransactionExt::V0,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::transaction::repository::StellarTransactionData;
    use serde_json;

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    mod memospec {
        use super::*;

        #[test]
        fn memo_text_valid() {
            let memo_spec = MemoSpec::Text("1234567890123456789012345678".to_string());
            assert!(Memo::try_from(memo_spec).is_ok());
        }

        #[test]
        fn memo_text_too_long() {
            let memo_spec = MemoSpec::Text("12345678901234567890123456789".to_string());
            assert!(Memo::try_from(memo_spec).is_err());
        }

        #[test]
        fn memo_hash_ok() {
            let memo_spec = MemoSpec::Hash([0u8; 32]);
            assert!(Memo::try_from(memo_spec).is_ok());
        }

        #[test]
        fn memo_id_conversion() {
            let memo_spec = MemoSpec::Id(123456);
            let memo = Memo::try_from(memo_spec).unwrap();
            assert!(matches!(memo, Memo::Id(123456)));
        }

        #[test]
        fn memo_return_conversion() {
            let memo_spec = MemoSpec::Return([1u8; 32]);
            let memo = Memo::try_from(memo_spec).unwrap();
            if let Memo::Return(hash) = memo {
                assert_eq!(hash.0, [1u8; 32]);
            } else {
                panic!("Expected Memo::Return");
            }
        }

        #[test]
        fn memo_spec_none_serde() {
            let spec = MemoSpec::None;
            let json = serde_json::to_string(&spec).unwrap();
            assert_eq!(json, r#"{"type":"NONE"}"#);
            let de: MemoSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn memo_spec_text_serde() {
            let spec = MemoSpec::Text("hola".to_string());
            let json = serde_json::to_string(&spec).unwrap();
            assert_eq!(json, r#"{"type":"TEXT","value":"hola"}"#);
            let de: MemoSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn memo_spec_hash_serde() {
            let spec = MemoSpec::Hash([0u8; 32]);
            let json = serde_json::to_string(&spec).unwrap();
            assert_eq!(
                json,
                r#"{"type":"HASH","value":"0000000000000000000000000000000000000000000000000000000000000000"}"#
            );
            let de: MemoSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn memo_spec_return_serde() {
            let spec = MemoSpec::Return([0xAB; 32]);
            let json = serde_json::to_string(&spec).unwrap();
            assert!(json.contains(r#""type":"RETURN""#));
            let de: MemoSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn memo_spec_text_missing_value_fails() {
            let json = r#"{"type":"TEXT"}"#;
            let res: Result<MemoSpec, _> = serde_json::from_str(json);
            assert!(res.is_err());
        }

        #[test]
        fn memo_spec_id_serde() {
            let spec = MemoSpec::Id(12345);
            let json = serde_json::to_string(&spec).unwrap();
            assert_eq!(json, r#"{"type":"ID","value":12345}"#);
            let de: MemoSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }
    }

    mod assetspec {
        use super::*;

        #[test]
        fn asset_native_ok() {
            assert!(Asset::try_from(AssetSpec::Native).is_ok());
        }

        #[test]
        fn asset_credit4_valid() {
            let spec = AssetSpec::Credit4 {
                code: "USDC".to_string(),
                issuer: TEST_PK.to_string(),
            };
            assert!(Asset::try_from(spec).is_ok());
        }

        #[test]
        fn asset_credit4_invalid_code_len() {
            let spec = AssetSpec::Credit4 {
                code: "TOOLONG".to_string(),
                issuer: TEST_PK.to_string(),
            };
            assert!(Asset::try_from(spec).is_err());
        }

        #[test]
        fn asset_credit12_invalid_code_len() {
            let spec = AssetSpec::Credit12 {
                code: "AAA".to_string(),
                issuer: TEST_PK.to_string(),
            };
            assert!(Asset::try_from(spec).is_err());
        }

        #[test]
        fn asset_spec_native_serde() {
            let spec = AssetSpec::Native;
            let json = serde_json::to_string(&spec).unwrap();
            assert_eq!(json, "\"NATIVE\"");
            let de: AssetSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn asset_spec_credit4_serde() {
            let spec = AssetSpec::Credit4 {
                code: "USDC".into(),
                issuer: TEST_PK.into(),
            };
            let json = serde_json::to_string(&spec).unwrap();
            assert!(json.contains("\"CREDIT4\""));
            assert!(json.contains("\"code\":\"USDC\""));
            let de: AssetSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn asset_spec_credit12_serde() {
            let spec = AssetSpec::Credit12 {
                code: "LONGTOKEN".into(),
                issuer: TEST_PK.into(),
            };
            let json = serde_json::to_string(&spec).unwrap();
            assert!(json.contains("\"CREDIT12\""));
            assert!(json.contains("\"code\":\"LONGTOKEN\""));
            let de: AssetSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn try_from_native_ok() {
            assert!(Asset::try_from(AssetSpec::Native).is_ok());
        }

        #[test]
        fn try_from_credit4_ok() {
            let spec = AssetSpec::Credit4 {
                code: "EURT".into(),
                issuer: TEST_PK.into(),
            };
            assert!(Asset::try_from(spec).is_ok());
        }

        #[test]
        fn try_from_credit4_invalid_code_err() {
            let spec = AssetSpec::Credit4 {
                code: "TOOLONG".into(),
                issuer: TEST_PK.into(),
            };
            assert!(Asset::try_from(spec).is_err());
        }

        #[test]
        fn try_from_credit4_invalid_issuer_err() {
            let spec = AssetSpec::Credit4 {
                code: "USDC".into(),
                issuer: "BADISSUER".into(),
            };
            assert!(Asset::try_from(spec).is_err());
        }

        #[test]
        fn try_from_credit12_invalid_code_len_err() {
            let spec = AssetSpec::Credit12 {
                code: "SHRT".into(),
                issuer: TEST_PK.into(),
            };
            assert!(Asset::try_from(spec).is_err());
        }

        #[test]
        fn try_from_credit12_ok() {
            let spec = AssetSpec::Credit12 {
                code: "LONGERTOKEN".into(),
                issuer: TEST_PK.into(),
            };
            assert!(Asset::try_from(spec).is_ok());
        }

        #[test]
        fn try_from_credit12_invalid_issuer_err() {
            let spec = AssetSpec::Credit12 {
                code: "VALIDTOKEN".into(),
                issuer: "BADISSUER".into(),
            };
            assert!(Asset::try_from(spec).is_err());
        }
    }

    mod operationspec {
        use super::*;

        #[test]
        fn payment_conversion_ok() {
            let op_spec = OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1_000_000,
                asset: AssetSpec::Native,
            };
            assert!(Operation::try_from(op_spec).is_ok());
        }

        #[test]
        fn payment_destination_invalid() {
            let op_spec = OperationSpec::Payment {
                destination: "invalid".to_string(),
                amount: 1_000_000,
                asset: AssetSpec::Native,
            };
            assert!(Operation::try_from(op_spec).is_err());
        }

        #[test]
        fn operation_spec_payment_native_serde() {
            let spec = OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 5_000,
                asset: AssetSpec::Native,
            };
            let json = serde_json::to_string(&spec).unwrap();
            assert!(json.contains(r#""op":"payment""#));
            assert!(json.contains(r#""asset":"NATIVE""#));
            let de: OperationSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn operation_spec_payment_credit4_serde() {
            let spec = OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1,
                asset: AssetSpec::Credit4 {
                    code: "USDC".to_string(),
                    issuer: TEST_PK.to_string(),
                },
            };
            let json = serde_json::to_string(&spec).unwrap();
            assert!(json.contains(r#""op":"payment""#));
            assert!(json.contains("\"CREDIT4\""));
            assert!(json.contains(r#""code":"USDC""#));
            let de: OperationSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(de, spec);
        }

        #[test]
        fn try_from_payment_native_ok() {
            let spec = OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 100,
                asset: AssetSpec::Native,
            };
            let op = Operation::try_from(spec).unwrap();
            matches!(op.body, OperationBody::Payment(_));
        }

        #[test]
        fn try_from_payment_invalid_destination_err() {
            let spec = OperationSpec::Payment {
                destination: "BAD".into(),
                amount: 1,
                asset: AssetSpec::Native,
            };
            assert!(Operation::try_from(spec).is_err());
        }

        #[test]
        fn try_from_payment_invalid_asset_err() {
            let spec = OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1,
                asset: AssetSpec::Credit4 {
                    code: "TOOLONG".into(),
                    issuer: TEST_PK.to_string(),
                },
            };
            assert!(Operation::try_from(spec).is_err());
        }
    }

    mod time_bounds {
        use super::*;

        #[test]
        fn valid_until_numeric() {
            let tb = valid_until_to_time_bounds(Some("12345".to_string())).unwrap();
            assert_eq!(tb.max_time, 12_345);
        }

        #[test]
        fn valid_until_rfc3339() {
            let tb = valid_until_to_time_bounds(Some("2025-01-01T00:00:00Z".to_string())).unwrap();
            assert_eq!(tb.max_time, 1_735_689_600);
        }

        #[test]
        fn valid_until_invalid() {
            assert!(valid_until_to_time_bounds(Some("not a date".to_string())).is_none());
        }

        #[test]
        fn valid_until_none() {
            let result = valid_until_to_time_bounds(None);
            assert!(result.is_none());
        }
    }

    mod stellar_transaction {
        use super::*;

        fn payment_op(destination: &str) -> OperationSpec {
            OperationSpec::Payment {
                destination: destination.to_string(),
                amount: 1000,
                asset: AssetSpec::Native,
            }
        }

        #[test]
        fn stellar_tx_try_from_ok() {
            let data = StellarTransactionData {
                source_account: TEST_PK.to_string(),
                fee: Some(100),
                sequence_number: Some(1),
                memo: Some(MemoSpec::None),
                valid_until: None,
                operations: vec![payment_op(TEST_PK)],
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: Vec::new(),
                hash: None,
            };
            let tx = Transaction::try_from(data).unwrap();
            assert_eq!(tx.fee, 100);
            assert_eq!(tx.seq_num.0, 1);
            assert_eq!(tx.operations.len(), 1);
        }

        #[test]
        fn stellar_tx_invalid_source_err() {
            let data = StellarTransactionData {
                source_account: "BAD".into(),
                fee: Some(100),
                sequence_number: Some(1),
                memo: None,
                valid_until: None,
                operations: vec![payment_op(TEST_PK)],
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: Vec::new(),
                hash: None,
            };
            assert!(Transaction::try_from(data).is_err());
        }

        #[test]
        fn stellar_tx_too_many_ops_err() {
            let ops = vec![payment_op(TEST_PK); 101];
            let data = StellarTransactionData {
                source_account: TEST_PK.into(),
                fee: Some(100),
                sequence_number: Some(1),
                memo: None,
                valid_until: None,
                operations: ops,
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: Vec::new(),
                hash: None,
            };
            assert!(Transaction::try_from(data).is_err());
        }

        #[test]
        fn stellar_tx_with_time_bounds() {
            let data = StellarTransactionData {
                source_account: TEST_PK.to_string(),
                fee: Some(100),
                sequence_number: Some(1),
                memo: None,
                valid_until: Some("1735689600".to_string()), // "2025-01-01T00:00:00Z"
                operations: vec![payment_op(TEST_PK)],
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: Vec::new(),
                hash: None,
            };
            let tx = Transaction::try_from(data).unwrap();
            if let Preconditions::Time(time_bounds) = tx.cond {
                assert_eq!(time_bounds.min_time.0, 0);
                assert_eq!(time_bounds.max_time.0, 1735689600);
            } else {
                panic!("Expected Preconditions::Time");
            }
        }

        #[test]
        fn stellar_tx_with_memo() {
            let data = StellarTransactionData {
                source_account: TEST_PK.to_string(),
                fee: None,
                sequence_number: None,
                memo: Some(MemoSpec::Id(12345)),
                valid_until: None,
                operations: vec![payment_op(TEST_PK)],
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: Vec::new(),
                hash: None,
            };
            let tx = Transaction::try_from(data).unwrap();
            assert_eq!(tx.fee, 100); // Default fee
            assert_eq!(tx.seq_num.0, 0); // Default sequence
            if let Memo::Id(id) = tx.memo {
                assert_eq!(id, 12345);
            } else {
                panic!("Expected Memo::Id");
            }
        }
    }
}
