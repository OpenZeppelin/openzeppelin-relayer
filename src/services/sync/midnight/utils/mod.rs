use crate::services::midnight::{
    indexer::{CollapsedUpdateInfo, TransactionData, ViewingKeyFormat},
    SyncError,
};

use bech32::{Bech32m, Hrp};
use midnight_ledger_prototype::transient_crypto::merkle_tree::MerkleTreeCollapsedUpdate;
use midnight_node_ledger_helpers::{
    deserialize, DefaultDB, NetworkId, Proof, Serializable, Transaction, Wallet,
};

/// Parse raw transaction hex into a Transaction type.
///
/// This method decodes and deserializes the transaction data for further processing.
pub fn parse_transaction(
    raw_hex: &str,
    network: NetworkId,
) -> Result<Transaction<Proof, DefaultDB>, SyncError> {
    let tx_bytes = hex::decode(raw_hex)
        .map_err(|e| SyncError::ParseError(format!("Failed to decode hex: {}", e)))?;

    let transaction: Transaction<Proof, DefaultDB> = deserialize(&tx_bytes[..], network)
        .map_err(|e| SyncError::ParseError(format!("Failed to deserialize transaction: {}", e)))?;

    Ok(transaction)
}

/// Process a transaction data object into a parsed transaction.
///
/// Returns None if the transaction data does not contain raw data.
pub fn process_transaction(
    transaction_data: &TransactionData,
    network: NetworkId,
) -> Result<Option<Transaction<Proof, DefaultDB>>, SyncError> {
    if let Some(raw_hex) = &transaction_data.raw {
        let parsed_tx = parse_transaction(raw_hex, network)?;
        Ok(Some(parsed_tx))
    } else {
        Ok(None)
    }
}

/// Parse raw collapsed update hex into a MerkleTreeCollapsedUpdate type.
///
/// This method decodes and deserializes the update data for further processing.
pub fn parse_collapsed_update(
    update_info: &CollapsedUpdateInfo,
    network: NetworkId,
) -> Result<MerkleTreeCollapsedUpdate, SyncError> {
    let update_bytes = hex::decode(&update_info.update_data)
        .map_err(|e| SyncError::MerkleTreeUpdateError(format!("Failed to decode hex: {}", e)))?;

    let collapsed_update: MerkleTreeCollapsedUpdate = deserialize(&update_bytes[..], network)
        .map_err(|e| {
            SyncError::MerkleTreeUpdateError(format!(
                "Failed to deserialize collapsed update: {}",
                e
            ))
        })?;

    Ok(collapsed_update)
}

/// Derive viewing key from wallet for the specified network.
///
/// Used internally to generate the viewing key for relevant transaction sync.
pub fn derive_viewing_key(
    wallet: &Wallet<DefaultDB>,
    network: NetworkId,
) -> Result<ViewingKeyFormat, SyncError> {
    let secret_keys = &wallet.secret_keys;
    let enc_secret_key = &secret_keys.encryption_secret_key;
    let mut enc_secret_bytes = Vec::new();
    Serializable::serialize(enc_secret_key, &mut enc_secret_bytes).map_err(|e| {
        SyncError::ViewingKeyError(format!("Failed to serialize encryption secret key: {}", e))
    })?;

    let network_suffix = match network {
        NetworkId::MainNet => "",
        NetworkId::TestNet => "_test",
        NetworkId::DevNet => "_dev",
        NetworkId::Undeployed => "_undeployed",
        _ => "",
    };

    let hrp_str = format!("mn_shield-esk{}", network_suffix);
    let hrp = Hrp::parse(&hrp_str)
        .map_err(|e| SyncError::ViewingKeyError(format!("Invalid HRP for viewing key: {}", e)))?;

    let viewing_key_bech32 = bech32::encode::<Bech32m>(hrp, &enc_secret_bytes).map_err(|e| {
        SyncError::ViewingKeyError(format!("Failed to encode viewing key in Bech32m: {}", e))
    })?;

    Ok(ViewingKeyFormat::Bech32m(viewing_key_bech32))
}
