//! Transaction conversion logic for Stellar

use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
use crate::domain::string_to_muxed_account;
use crate::models::transaction::repository::StellarTransactionData;
use crate::models::SignerError;
use chrono::DateTime;
use soroban_rs::xdr::{
    Limits, Memo, Operation, Preconditions, ReadXdr, SequenceNumber, TimeBounds, TimePoint,
    Transaction, TransactionExt, VecM,
};
use std::convert::TryFrom;

pub type DecoratedSignature = soroban_rs::xdr::DecoratedSignature;

#[derive(Debug, Clone)]
pub struct TimeBoundsSpec {
    pub min_time: u64,
    pub max_time: u64,
}

fn valid_until_to_time_bounds(valid_until: Option<String>) -> Option<TimeBoundsSpec> {
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
        match &data.transaction_input {
            crate::models::TransactionInput::Operations(ops) => {
                // Build transaction from operations
                let converted_ops: Result<Vec<Operation>, SignerError> = ops
                    .iter()
                    .map(|op| Operation::try_from(op.clone()))
                    .collect();
                let operations = converted_ops?;

                let operations: VecM<Operation, 100> = operations
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

                let source_account =
                    string_to_muxed_account(&data.source_account).map_err(|e| {
                        SignerError::ConversionError(format!("Invalid source account: {}", e))
                    })?;

                // Apply transaction extension data from simulation if available
                let ext = match &data.simulation_transaction_data {
                    Some(xdr_data) => {
                        use soroban_rs::xdr::SorobanTransactionData;
                        match SorobanTransactionData::from_xdr_base64(xdr_data, Limits::none()) {
                            Ok(tx_data) => {
                                log::info!("Applied transaction extension data from simulation");
                                TransactionExt::V1(tx_data)
                            }
                            Err(e) => {
                                log::warn!(
                                    "Failed to decode transaction data XDR: {}, using V0",
                                    e
                                );
                                TransactionExt::V0
                            }
                        }
                    }
                    None => TransactionExt::V0,
                };

                Ok(Transaction {
                    source_account,
                    fee,
                    seq_num: SequenceNumber(sequence),
                    cond,
                    memo,
                    operations,
                    ext,
                })
            }
            crate::models::TransactionInput::UnsignedXdr(_)
            | crate::models::TransactionInput::SignedXdr { .. } => {
                // XDR inputs should not be converted to Transaction
                // The signer handles TransactionEnvelope XDR directly
                Err(SignerError::ConversionError(
                    "XDR inputs should not be converted to Transaction - use envelope directly"
                        .into(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::transaction::stellar::asset::AssetSpec;
    use crate::models::transaction::stellar::{MemoSpec, OperationSpec};

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    #[test]
    fn test_basic_transaction() {
        let data = StellarTransactionData {
            source_account: TEST_PK.to_string(),
            fee: Some(100),
            sequence_number: Some(1),
            memo: Some(MemoSpec::None),
            valid_until: None,
            transaction_input: crate::models::TransactionInput::Operations(vec![
                OperationSpec::Payment {
                    destination: TEST_PK.to_string(),
                    amount: 1000,
                    asset: AssetSpec::Native,
                },
            ]),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let tx = Transaction::try_from(data).unwrap();
        assert_eq!(tx.fee, 100);
        assert_eq!(tx.seq_num.0, 1);
        assert_eq!(tx.operations.len(), 1);
    }

    #[test]
    fn test_transaction_with_time_bounds() {
        let data = StellarTransactionData {
            source_account: TEST_PK.to_string(),
            fee: None,
            sequence_number: None,
            memo: None,
            valid_until: Some("1735689600".to_string()),
            transaction_input: crate::models::TransactionInput::Operations(vec![
                OperationSpec::Payment {
                    destination: TEST_PK.to_string(),
                    amount: 1000,
                    asset: AssetSpec::Native,
                },
            ]),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
            signed_envelope_xdr: None,
        };

        let tx = Transaction::try_from(data).unwrap();
        if let Preconditions::Time(tb) = tx.cond {
            assert_eq!(tb.max_time.0, 1735689600);
        } else {
            panic!("Expected time bounds");
        }
    }

    #[test]
    fn test_valid_until_numeric_string() {
        let tb = valid_until_to_time_bounds(Some("12345".to_string())).unwrap();
        assert_eq!(tb.max_time, 12_345);
        assert_eq!(tb.min_time, 0);
    }

    #[test]
    fn test_valid_until_rfc3339_string() {
        let tb = valid_until_to_time_bounds(Some("2025-01-01T00:00:00Z".to_string())).unwrap();
        assert_eq!(tb.max_time, 1_735_689_600);
        assert_eq!(tb.min_time, 0);
    }

    #[test]
    fn test_valid_until_invalid_string() {
        assert!(valid_until_to_time_bounds(Some("not a date".to_string())).is_none());
    }

    #[test]
    fn test_valid_until_none() {
        assert!(valid_until_to_time_bounds(None).is_none());
    }
}
