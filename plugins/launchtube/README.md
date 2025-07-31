# Launchtube V3 Plugin

A plugin for OpenZeppelin Relayer that provides launchtube capabilities for submitting Stellar Soroban transactions.

## What is Launchtube?

Getting Stellar transactions successfully submitted to the network can be challenging with XLM fees, sequence numbers, retries, and more. This gets even trickier with Soroban smart contract invocations.

Launchtube simplifies this by accepting Soroban operations and handling submission to the network. Just simulate and sign your Soroban ops, submit them to Launchtube, and it handles getting them on-chain.

## Setup

### 1. Generate Stellar Keys

Create keys for the fund account (pays fees) and sequence accounts (submit transactions):

```bash
# Create directory for keys
mkdir -p config/keys/launchtube

# Generate fund account key
cargo run --example create_key -- \
    --password "YourSecurePassword" \
    --output-dir "config/keys/launchtube" \
    --filename "launchtube-fund-signer.json"

# Generate sequence account keys
cargo run --example create_key -- \
    --password "YourSecurePassword" \
    --output-dir "config/keys/launchtube" \
    --filename "launchtube-seq-001-signer.json"

cargo run --example create_key -- \
    --password "YourSecurePassword" \
    --output-dir "config/keys/launchtube" \
    --filename "launchtube-seq-002-signer.json"
```

### 2. Register Signers and Create Relayers

1. Register the generated signers with your OpenZeppelin Relayer
2. Create relayers using these signers with IDs matching your config
3. Fund all accounts on Stellar with XLM (fund account needs more for fees)

### 3. Register Plugin in Relayer

Add the launchtube plugin to your relayer's configuration file (`config.json` in the relayer root):

```json
{
  "plugins": [
    {
      "id": "launchtube",
      "path": "plugins/launchtube/dist/index.js"
    }
  ]
}
```

### 4. Configure Plugin

```bash
cd plugins/launchtube
cp config.example.json config.json
```

Edit `config.json`:

```json
{
  "fundRelayerId": "launchtube-fund",
  "sequenceRelayerIds": ["launchtube-seq-001", "launchtube-seq-002"],
  "maxFee": 1000000,
  "network": "testnet",
  "rpcUrl": "https://soroban-testnet.stellar.org"
}
```

## API Usage

### Submit Transaction

```bash
# With full transaction XDR
curl -X POST http://localhost:8080/api/v1/plugins/launchtube/call \
  -H "Authorization: Bearer YOUR_RELAYER_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "xdr": "AAAAAgAAAAA...",
      "sim": true
    }
  }'

# With separate function and auth
curl -X POST http://localhost:8080/api/v1/plugins/launchtube/call \
  -H "Authorization: Bearer YOUR_RELAYER_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "func": "AAAABAAAAAA...",
      "auth": ["AAAACAAA..."],
      "sim": false
    }
  }'
```

### Response

```json
{
  "transactionId": "tx_123456",
  "status": "submitted",
  "hash": "1234567890abcdef..."
}
```
