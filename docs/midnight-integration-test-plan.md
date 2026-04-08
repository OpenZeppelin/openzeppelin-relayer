# Midnight Integration Test Plan

End-to-end test against Midnight **testnet** (preview environment).

## Prerequisites

- Lace wallet with Midnight testnet enabled (you have this)
- Docker (for proof server)
- Redis running locally
- The relayer built with `--features midnight`

## Step 0: Update network config with real endpoints

Update `config/networks/midnight.json` to use the preview environment:

```json
{
  "networks": [
    {
      "type": "midnight",
      "network": "testnet",
      "rpc_urls": ["https://rpc.preview.midnight.network"],
      "explorer_urls": ["https://explorer.preview.midnight.network"],
      "average_blocktime_ms": 6000,
      "is_testnet": true,
      "indexer_urls": {
        "http": "https://indexer.preview.midnight.network/api/v4/graphql",
        "ws": "wss://indexer.preview.midnight.network/api/v4/graphql/ws"
      },
      "prover_url": "https://lace-proof-pub.preview.midnight.network"
    }
  ]
}
```

> If the public indexer uses v3 instead of v4, try:
> `https://indexer.preview.midnight.network/api/v3/graphql`

## Step 1: Generate a signer key

Create a 32-byte Ed25519 key for the relayer. The relayer uses the standard
encrypted keystore format (same as EVM).

```bash
# Generate a random 32-byte hex key
KEY_HEX=$(openssl rand -hex 32)
echo "Raw key: $KEY_HEX"

# Create a keystore JSON file using the relayer's built-in tooling,
# or use a simple Python script:
python3 -c "
import json, os, hashlib
from Crypto.Cipher import AES
from Crypto.Protocol.KDF import scrypt

key_hex = '$KEY_HEX'
passphrase = b'test-passphrase'
key_bytes = bytes.fromhex(key_hex)
salt = os.urandom(32)
dk = scrypt(passphrase, salt, 32, N=8192, r=8, p=1)
iv = os.urandom(16)
cipher = AES.new(dk[:16], AES.MODE_CTR, nonce=b'', initial_value=iv)
ciphertext = cipher.encrypt(key_bytes)
mac = hashlib.sha3_256(dk[16:] + ciphertext).hexdigest()
keystore = {
    'crypto': {
        'cipher': 'aes-128-ctr',
        'cipherparams': {'iv': iv.hex()},
        'ciphertext': ciphertext.hex(),
        'kdf': 'scrypt',
        'kdfparams': {'dklen': 32, 'n': 8192, 'p': 1, 'r': 8, 'salt': salt.hex()},
        'mac': mac
    },
    'id': '$(python3 -c "import uuid; print(uuid.uuid4())")',
    'version': 3
}
print(json.dumps(keystore, indent=2))
" > config/keys/midnight-signer.json
```

Alternatively, if the Python crypto dependency is a hassle, you can reuse an
existing test keystore and just note the address that appears in the logs.

**Simpler approach — use raw hex key via env var:**

Check if the relayer supports `MIDNIGHT_SIGNER_RAW_KEY` or similar. If not,
the fastest path is to reuse an existing EVM keystore file — the underlying
32-byte key works for Ed25519 too. The relayer will derive the Midnight
address on startup and log it.

## Step 2: Create the relayer config

Create `config/config.json`:

```json
{
  "relayers": [
    {
      "id": "midnight-testnet",
      "name": "Midnight Testnet Relayer",
      "network": "testnet",
      "paused": false,
      "signer_id": "midnight-signer",
      "network_type": "midnight",
      "policies": {
        "min_balance": 0
      }
    }
  ],
  "notifications": [],
  "signers": [
    {
      "id": "midnight-signer",
      "type": "local",
      "config": {
        "path": "config/keys/midnight-signer.json",
        "passphrase": {
          "type": "env",
          "value": "KEYSTORE_PASSPHRASE"
        }
      }
    }
  ],
  "networks": "./config/networks",
  "plugins": []
}
```

## Step 3: Set environment variables

```bash
export KEYSTORE_PASSPHRASE="test-passphrase"
export REDIS_URL="redis://localhost:6379"
export API_KEY="test-api-key-for-midnight"
```

## Step 4: Start infrastructure

```bash
# Redis (if not already running)
docker run -d --name redis -p 6379:6379 redis:7

# Proof server (optional for initial testing — only needed for real tx submission)
# docker run -d --name midnight-prover -p 6300:6300 midnightntwrk/proof-server:8.0.3
```

## Step 5: Build and run the relayer

```bash
cd /private/tmp/openzeppelin-relayer-midnight

# Build with midnight feature
cargo build --features midnight

# Run
cargo run --features midnight
```

### What to look for in logs:

```
✓ "Initializing Midnight relayer"
✓ "Midnight relayer initialized with sync" (or "Initial sync failed" if indexer is unreachable)
✓ The relayer address should be printed — this is what you fund from Lace
```

If you see the address in format `mn_<64-hex-chars>`, that's the relayer's
address. Copy it for the next step.

## Step 6: Verify the relayer is running

```bash
# Health check
curl -s http://localhost:8080/api/v1/relayers/midnight-testnet \
  -H "X-API-Key: test-api-key-for-midnight" | jq .

# Expected: relayer info with status
```

```bash
# Get status (includes nonce = block height)
curl -s http://localhost:8080/api/v1/relayers/midnight-testnet/status \
  -H "X-API-Key: test-api-key-for-midnight" | jq .

# Expected:
# {
#   "midnight": {
#     "balance": "0",
#     "pending_transactions_count": 0,
#     "nonce": "12345",       <-- current block height
#     "system_disabled": false,
#     "paused": false
#   }
# }
```

## Step 7: Fund the relayer from Lace

1. Open Lace wallet (Midnight testnet)
2. Send tDUST to the relayer address from Step 5
3. Wait for confirmation (~6s block time)

If the relayer address format doesn't match what Lace expects (bech32m), you
may need to derive the proper bech32m address. This is a known placeholder —
see "Known Gaps" below.

## Step 8: Submit a test transaction

```bash
# Simple guaranteed offer (value transfer)
curl -s -X POST http://localhost:8080/api/v1/relayers/midnight-testnet/transactions \
  -H "X-API-Key: test-api-key-for-midnight" \
  -H "Content-Type: application/json" \
  -d '{
    "guaranteed_offer": {
      "inputs": [
        {
          "origin": "<relayer-seed-hex>",
          "token_type": "0000...native-token-hex",
          "value": "1000"
        }
      ],
      "outputs": [
        {
          "destination": "<recipient-address>",
          "token_type": "0000...native-token-hex",
          "value": "1000"
        }
      ]
    }
  }' | jq .

# Expected:
# {
#   "id": "uuid-here",
#   "status": "Pending",
#   "hash": null,
#   ...
# }
```

## Step 9: Monitor transaction status

```bash
# Poll for status
TX_ID="<uuid-from-step-8>"
curl -s http://localhost:8080/api/v1/relayers/midnight-testnet/transactions/$TX_ID \
  -H "X-API-Key: test-api-key-for-midnight" | jq .status

# Expected progression: Pending → Sent → Submitted → Confirmed (or Failed)
```

---

## Test Checkpoints

| # | Test | What validates | Pass criteria |
|---|------|---------------|---------------|
| 1 | Service starts with `--features midnight` | Config parsing, network loading, signer creation | No panic, relayer appears in logs |
| 2 | GET /relayers/midnight-testnet | Relayer factory, network resolution | 200 OK with relayer data |
| 3 | GET /relayers/midnight-testnet/status | Provider health check, block number query | Returns `nonce` > 0 |
| 4 | Health check passes | RPC + indexer reachability | Both `rpc_health` and `indexer_health` pass |
| 5 | POST /transactions (guaranteed_offer) | Request parsing, validation, repo creation, job enqueue | Returns 200 with transaction ID |
| 6 | Transaction moves to Sent | Job handler picks up, prepare_transaction runs | Status changes to Sent |
| 7 | Transaction moves to Submitted | submit_transaction sends extrinsic | Status changes, hash populated |
| 8 | Transaction reaches Confirmed/Failed | Status check job queries indexer | Final status with reason |

## Known Gaps (will fail until fixed)

These are expected failures at the current implementation stage:

1. **Address format** — The relayer generates `mn_<hex>` addresses, but Lace
   expects bech32m (`mn_shield-addr_test1...`). Funding will require manual
   address conversion or midnight-node crate integration.

2. **Transaction submission** — `submit_transaction()` currently sends raw hex
   to `author_submitExtrinsic`, but real Midnight transactions need proper
   extrinsic encoding with ZK proofs. This will fail at the RPC level.

3. **Balance query** — Returns `0` placeholder. Real balance needs ZK wallet
   sync with ledger context.

4. **Proof generation** — Not wired yet. Real transactions need the proof
   server to generate ZK proofs before submission.

## What WILL work (validates the integration)

Even with the gaps above, we can validate:

- **Checkpoints 1-4**: Service startup, config parsing, RPC connectivity,
  indexer connectivity, block height query, health checks
- **Checkpoint 5**: Transaction request parsing, validation, repository
  storage, job enqueue
- **Checkpoint 6**: Job handler pickup, prepare_transaction flow

Checkpoints 7-8 will fail at the RPC submission level, which is expected.
The failure message itself validates that the pipeline works up to the
submission point.

## Quick Smoke Test (minimal validation)

If you just want to verify the basic wiring works:

```bash
# 1. Start with midnight feature
cargo run --features midnight 2>&1 | head -50

# 2. In another terminal, check health:
curl -s http://localhost:8080/api/v1/relayers/midnight-testnet/status \
  -H "X-API-Key: $API_KEY" | jq .

# 3. If status returns with nonce > 0, the RPC + indexer connection works.
#    This validates: config loading → network resolution → provider creation
#    → RPC call → response parsing → API response serialization.
```

## Debugging

If the service fails to start:
- Check Redis is running: `redis-cli ping`
- Check RPC reachability: `curl -s -X POST https://rpc.preview.midnight.network -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"system_health","params":[]}' | jq .`
- Check indexer reachability: `curl -s -X POST https://indexer.preview.midnight.network/api/v4/graphql -H "Content-Type: application/json" -d '{"query":"{ __typename }"}' | jq .`
