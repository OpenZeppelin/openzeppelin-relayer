//! Transaction conversion logic for Stellar

use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
use crate::models::transaction::repository::StellarTransactionData;
use crate::models::transaction::stellar::helpers::valid_until_to_time_bounds;
use crate::models::SignerError;
use soroban_rs::xdr::{
    Memo, MuxedAccount as XdrMuxedAccount, MuxedAccountMed25519, Operation, Preconditions,
    SequenceNumber, TimeBounds, TimePoint, Transaction, TransactionExt, Uint256, VecM,
};
use std::convert::TryFrom;
use stellar_strkey::ed25519::{MuxedAccount, PublicKey};

pub type DecoratedSignature = soroban_rs::xdr::DecoratedSignature;

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

        // Apply transaction extension data from simulation if available
        let ext = match &data.simulation_transaction_data {
            Some(xdr_data) => {
                use soroban_rs::xdr::{Limits, ReadXdr, SorobanTransactionData};
                match SorobanTransactionData::from_xdr_base64(xdr_data, Limits::none()) {
                    Ok(tx_data) => {
                        log::info!("Applied transaction extension data from simulation");
                        TransactionExt::V1(tx_data)
                    }
                    Err(e) => {
                        log::warn!("Failed to decode transaction data XDR: {}, using V0", e);
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
            operations: vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000,
                asset: AssetSpec::Native,
            }],
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
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
            operations: vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000,
                asset: AssetSpec::Native,
            }],
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
        };

        let tx = Transaction::try_from(data).unwrap();
        if let Preconditions::Time(tb) = tx.cond {
            assert_eq!(tb.max_time.0, 1735689600);
        } else {
            panic!("Expected time bounds");
        }
    }
}
