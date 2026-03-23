//! Relayer transaction/status/signing/rpc integration tests
//!
//! Focuses on endpoint behavior for invalid relayer IDs to cover route/controller wiring.

use crate::integration::common::client::RelayerClient;
use serial_test::serial;

const MISSING_RELAYER_ID: &str = "nonexistent-relayer-id";

fn assert_not_found_error(error: eyre::Report) {
    let msg = error.to_string().to_lowercase();
    assert!(
        msg.contains("404") || msg.contains("not found"),
        "Expected not found error, got: {}",
        msg
    );
}

#[tokio::test]
#[serial]
async fn test_relayer_status_and_balance_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    assert_not_found_error(
        client
            .get_relayer_status(MISSING_RELAYER_ID)
            .await
            .expect_err("Expected relayer status to fail"),
    );
    assert_not_found_error(
        client
            .get_relayer_balance(MISSING_RELAYER_ID)
            .await
            .expect_err("Expected relayer balance to fail"),
    );
}

#[tokio::test]
#[serial]
async fn test_relayer_transaction_routes_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    assert_not_found_error(
        client
            .list_relayer_transactions(MISSING_RELAYER_ID, 1, 10)
            .await
            .expect_err("Expected list transactions to fail"),
    );
    assert_not_found_error(
        client
            .get_transaction_by_nonce(MISSING_RELAYER_ID, 0)
            .await
            .expect_err("Expected get by nonce to fail"),
    );
    assert_not_found_error(
        client
            .delete_pending_transactions(MISSING_RELAYER_ID)
            .await
            .expect_err("Expected delete pending transactions to fail"),
    );
    assert_not_found_error(
        client
            .cancel_transaction(MISSING_RELAYER_ID, "tx-does-not-exist")
            .await
            .expect_err("Expected cancel transaction to fail"),
    );
    assert_not_found_error(
        client
            .replace_transaction(
                MISSING_RELAYER_ID,
                "tx-does-not-exist",
                serde_json::json!({
                    "speed": "fast"
                }),
            )
            .await
            .expect_err("Expected replace transaction to fail"),
    );
}

#[tokio::test]
#[serial]
async fn test_relayer_signing_and_rpc_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    assert_not_found_error(
        client
            .relayer_rpc(
                MISSING_RELAYER_ID,
                serde_json::json!({
                    "method": "eth_blockNumber",
                    "params": []
                }),
            )
            .await
            .expect_err("Expected relayer RPC to fail"),
    );
    assert_not_found_error(
        client
            .sign_data(
                MISSING_RELAYER_ID,
                serde_json::json!({
                    "message": "0x1234"
                }),
            )
            .await
            .expect_err("Expected sign_data to fail"),
    );
    assert_not_found_error(
        client
            .sign_typed_data(
                MISSING_RELAYER_ID,
                serde_json::json!({
                    "domain_separator": "0x0000000000000000000000000000000000000000000000000000000000000000",
                    "hash_struct_message": "0x0000000000000000000000000000000000000000000000000000000000000000"
                }),
            )
            .await
            .expect_err("Expected sign_typed_data to fail"),
    );
    assert_not_found_error(
        client
            .sign_transaction_payload(
                MISSING_RELAYER_ID,
                serde_json::json!({
                    "unsigned_xdr": "AAAA"
                }),
            )
            .await
            .expect_err("Expected sign_transaction to fail"),
    );
}

#[tokio::test]
#[serial]
async fn test_relayer_sponsored_routes_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    assert_not_found_error(
        client
            .quote_sponsored_transaction(
                MISSING_RELAYER_ID,
                serde_json::json!({
                    "transaction": "AAAA",
                    "fee_token": "So11111111111111111111111111111111111111112"
                }),
            )
            .await
            .expect_err("Expected sponsored quote to fail"),
    );
    assert_not_found_error(
        client
            .build_sponsored_transaction(
                MISSING_RELAYER_ID,
                serde_json::json!({
                    "transaction": "AAAA",
                    "fee_token": "So11111111111111111111111111111111111111112"
                }),
            )
            .await
            .expect_err("Expected sponsored build to fail"),
    );
}
