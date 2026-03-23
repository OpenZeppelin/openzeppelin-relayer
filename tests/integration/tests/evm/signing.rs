//! EVM signing integration tests
//!
//! Validates that a relayer with a local signer can sign messages and typed data
//! through the API, producing valid signatures.

use crate::integration::common::{
    client::RelayerClient,
    context::{is_evm_network, run_multi_network_test},
    evm_helpers::setup_test_relayer,
};
use openzeppelin_relayer::models::relayer::RelayerResponse;
use serial_test::serial;
use tracing::{info, info_span};

/// Tests signing a raw message via the relayer's signer.
async fn run_sign_data_test(network: String, relayer_info: RelayerResponse) -> eyre::Result<()> {
    let _span = info_span!("sign_data", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting sign data test");

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    let result = client
        .sign_data(
            &relayer.id,
            serde_json::json!({
                "message": "0x48656c6c6f20576f726c64"
            }),
        )
        .await?;

    let sig = result
        .get("data")
        .and_then(|d| d.get("sig"))
        .or_else(|| result.get("data").and_then(|d| d.get("signature")))
        .or_else(|| result.get("sig"))
        .or_else(|| result.get("signature"))
        .and_then(|s| s.as_str());

    assert!(
        sig.is_some(),
        "sign_data should return a signature, got: {}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );

    let sig_str = sig.unwrap();
    // EVM signatures are 65 bytes = 130 hex chars, optionally with "0x" prefix
    let hex_len = if sig_str.starts_with("0x") {
        sig_str.len() - 2
    } else {
        sig_str.len()
    };
    assert!(
        hex_len >= 130,
        "Signature should be at least 65 bytes (got {} hex chars): {}",
        hex_len,
        sig_str
    );

    info!(sig_len = sig_str.len(), "Message signed successfully");
    Ok(())
}

/// Tests signing EIP-712 typed data via the relayer's signer.
async fn run_sign_typed_data_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span =
        info_span!("sign_typed_data", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting sign typed data test");

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    // Pre-computed domain separator and struct hash for testing
    let result = client
        .sign_typed_data(
            &relayer.id,
            serde_json::json!({
                "domain_separator": "0x0101010101010101010101010101010101010101010101010101010101010101",
                "hash_struct_message": "0x0202020202020202020202020202020202020202020202020202020202020202"
            }),
        )
        .await?;

    let sig = result
        .get("data")
        .and_then(|d| d.get("sig"))
        .or_else(|| result.get("data").and_then(|d| d.get("signature")))
        .or_else(|| result.get("sig"))
        .or_else(|| result.get("signature"))
        .and_then(|s| s.as_str());

    assert!(
        sig.is_some(),
        "sign_typed_data should return a signature, got: {}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );

    let sig_str = sig.unwrap();
    let hex_len = if sig_str.starts_with("0x") {
        sig_str.len() - 2
    } else {
        sig_str.len()
    };
    assert!(
        hex_len >= 130,
        "Typed data signature should be at least 65 bytes (got {} hex chars)",
        hex_len
    );

    info!(sig_len = sig_str.len(), "Typed data signed successfully");
    Ok(())
}

/// Tests the RPC passthrough endpoint (eth_blockNumber).
/// The RPC endpoint returns raw JSON-RPC responses (not wrapped in ApiResponse),
/// so we use a raw HTTP request instead of the client's relayer_rpc method.
async fn run_rpc_passthrough_test(
    network: String,
    relayer_info: RelayerResponse,
) -> eyre::Result<()> {
    let _span =
        info_span!("rpc_passthrough", network = %network, relayer = %relayer_info.id).entered();
    info!("Starting RPC passthrough test");

    let client = RelayerClient::from_env()?;
    let relayer = setup_test_relayer(&client, &relayer_info).await?;

    let base_url =
        std::env::var("RELAYER_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let api_key = std::env::var("API_KEY").expect("API_KEY must be set");

    let url = format!("{}/api/v1/relayers/{}/rpc", base_url, relayer.id);
    let response = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_blockNumber",
            "params": []
        }))
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    assert!(
        status.is_success(),
        "RPC should succeed, got {}: {}",
        status,
        body
    );

    let parsed: serde_json::Value = serde_json::from_str(&body)?;
    let block = parsed.get("result").and_then(|r| r.as_str());

    assert!(
        block.is_some(),
        "RPC response should have 'result' field, got: {}",
        body
    );

    let block_str = block.unwrap();
    assert!(
        block_str.starts_with("0x"),
        "Block number should be hex: {}",
        block_str
    );

    info!(block = block_str, "RPC passthrough working");
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_evm_sign_data() {
    run_multi_network_test("sign_data", is_evm_network, run_sign_data_test).await;
}

#[tokio::test]
#[serial]
async fn test_evm_sign_typed_data() {
    run_multi_network_test("sign_typed_data", is_evm_network, run_sign_typed_data_test).await;
}

#[tokio::test]
#[serial]
async fn test_evm_rpc_passthrough() {
    run_multi_network_test("rpc_passthrough", is_evm_network, run_rpc_passthrough_test).await;
}
