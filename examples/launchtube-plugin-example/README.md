# OpenZeppelin Relayer — LaunchTube Plugin Example

Run the LaunchTube plugin with OpenZeppelin Relayer to simplify Stellar Soroban transactions. LaunchTube handles fees, sequence numbers, simulation, and retries automatically.

## Quick Start

```bash
# Clone and navigate to this example:
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer/examples/launchtube-plugin-example

# Then follow the Setup steps below
```

## Prerequisites

- Docker and Docker Compose
- Rust (for generating keys and IDs)
- Node.js >= 18 and pnpm >= 10

## Setup

You only need to:

1. Install and build the LaunchTube plugin
2. Create the keys for LaunchTube accounts
3. Set up environment variables
4. Start Docker to get account addresses
5. Fund accounts on testnet
6. Restart the service

All configurations are pre-set for testnet use.

### 1. Install Dependencies

Install and build the LaunchTube plugin:

```bash
# From this directory (examples/launchtube-plugin-example)
cd launchtube
pnpm install
pnpm run build
cd ..
```

### 2. Create Keys and Configuration

LaunchTube requires two types of keys:

- **Fund account**: Pays transaction fees
- **Sequence accounts**: Manage sequence numbers (at least 2 recommended)

From this directory (`examples/launchtube-plugin-example`), run these commands:

#### Create LaunchTube accounts

```bash
# Replace each YOUR_PASSWORD with a unique strong password for each key
# You will need to add these passwords to your .env file
# Password must contain at least one uppercase letter, one lowercase letter,
# one number, and one special character (e.g., MyPass123!)

# Create fund account (pays fees)
cargo run --example create_key -- \
  --password YOUR_PASSWORD \
  --output-dir config/keys \
  --filename launchtube-fund.json

# Create first sequence account
cargo run --example create_key -- \
  --password YOUR_PASSWORD \
  --output-dir config/keys \
  --filename launchtube-seq-001.json

# Create second sequence account (recommended for better throughput)
cargo run --example create_key -- \
  --password YOUR_PASSWORD \
  --output-dir config/keys \
  --filename launchtube-seq-002.json
```

#### Generate API credentials

```bash
# Generate API key (save this output)
cargo run --example generate_uuid

# Generate webhook signing key (save this output)
cargo run --example generate_uuid
```

#### Create environment file

Create `.env` in this directory:

```env
REDIS_URL=redis://redis:6379
KEYSTORE_PASSPHRASE_FUND=YOUR_PASSWORD
KEYSTORE_PASSPHRASE_SEQ_001=YOUR_PASSWORD
KEYSTORE_PASSPHRASE_SEQ_002=YOUR_PASSWORD
WEBHOOK_SIGNING_KEY=<webhook_key_from_above>
API_KEY=<api_key_from_above>
```

### 3. Verify Configuration

The LaunchTube plugin and relayer configurations are already set up for testnet. The configurations include:

**`launchtube/config.json`** (pre-configured):

```json
{
  "fundRelayerId": "launchtube-fund",
  "sequenceRelayerIds": ["launchtube-seq-001", "launchtube-seq-002"],
  "maxFee": 1000000,
  "network": "testnet",
  "rpcUrl": "https://soroban-testnet.stellar.org"
}
```

**`config/config.json`** (pre-configured):

- Three relayers defined: `launchtube-fund`, `launchtube-seq-001`, `launchtube-seq-002`
- The Fund relayer has `concurrent_transactions: true` enabled in policies to allow parallel processing
- Corresponding signers pointing to the key files you'll create
- Plugin registered as `launchtube-plugin`

> **Note**: If you need mainnet, update `network` in both config files and use mainnet RPC URL

### 4. (Optional) Configure Webhooks

For transaction notifications, edit `config/config.json`:

```json
{
  "notifications": [
    {
      "url": "https://webhook.site/your-unique-id" // Get a test URL from webhook.site
    }
  ]
}
```

### 5. Start the Service and Get Account Addresses

```bash
docker compose up
```

The relayer will start and display the public addresses for your accounts in the logs:

```
relayer-1  | Syncing sequence for relayer: launchtube-fund (GCP7KWGZCDDVBFKANDJTA74H2HSORV34SMSQIPGZ3PK7V6OHKCFGRTF6)
relayer-1  | Syncing sequence for relayer: launchtube-seq-001 (GCWFXU6HZNHLTXMHWZRPXYBZFOODJYRDZXFOPMUQN4S2JJGEZA2ZHA4B)
relayer-1  | Syncing sequence for relayer: launchtube-seq-002 (GA7IXWK3VKF25JOXJZZ7XMFB3A3IPM5A66MW5DJ6FPOIWME4F66UK4HL)
```

### 6. Fund Your Accounts on Testnet

In a new terminal, copy the addresses from the logs above and fund them:

```bash
# Replace with your actual addresses from the logs
curl "https://friendbot.stellar.org?addr=YOUR_FUND_ADDRESS"     # fund account
curl "https://friendbot.stellar.org?addr=YOUR_SEQ_001_ADDRESS"  # seq-001
curl "https://friendbot.stellar.org?addr=YOUR_SEQ_002_ADDRESS"  # seq-002
```

After funding, restart the service for it to recognize the funded accounts:

```bash
# Stop the service with Ctrl+C, then restart
docker compose up
```

The relayer is now ready at `http://localhost:8080/api/v1` 🚀

## Usage

### Test Connection

```bash
curl -X GET http://localhost:8080/api/v1/plugins \
  -H "Authorization: Bearer YOUR_API_KEY"
```

### Submit Transactions

#### Option 1: Complete Transaction XDR

```bash
curl -X POST http://localhost:8080/api/v1/plugins/launchtube-plugin/call \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "xdr": "AAAAAgAAAAA...",
      "sim": false
    }
  }'
```

#### Option 2: Soroban Function + Auth

```bash
curl -X POST http://localhost:8080/api/v1/plugins/launchtube-plugin/call \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "func": "AAAABAAAAAEAAAAGc3ltYm9s...",
      "auth": ["AAAACAAAAAEAAAA..."],
      "sim": true
    }
  }'
```

**Parameters:**

- `xdr`: Complete transaction envelope XDR
- `func`: Soroban host function XDR
- `auth`: Array of authorization entry XDRs
- `sim`: Simulate before submission (true/false)

> Use either `xdr` OR `func`+`auth`, not both

**Response:**

```json
{
  "transactionId": "tx_123456",
  "status": "submitted",
  "hash": "1234567890abcdef..."
}
```

### Generating XDR for the Relayer

Use [@stellar/stellar-sdk](https://stellar.github.io/js-stellar-sdk/TransactionBuilder.html) to export either a full transaction envelope XDR, or Soroban `func` + `auth` XDRs.

#### Full transaction envelope XDR

```ts
import { Networks, TransactionBuilder, rpc } from '@stellar/stellar-sdk';

// ...build your tx with TransactionBuilder and Contract.call(...)
const tx = new TransactionBuilder(account, {
  fee: '100',
  networkPassphrase: Networks.TESTNET,
})
  .addOperation(/* Operation.invokeHostFunction from Contract.call(...) */)
  .setTimeout(30)
  .build();

// Optional: pre-simulate to set resources/fees before signing
const sim = await rpcServer.simulateTransaction(tx);
const prepared = rpc.assembleTransaction(tx, sim).build();
prepared.sign(keypair);

// Export base64 envelope XDR
const envelopeXdr = prepared.toXDR();
```

#### Soroban `func` + `auth` XDR

```ts
// Build and simulate first to obtain auth
const baseTx = /* TransactionBuilder(...).addOperation(...).build() */;
const sim = await rpcServer.simulateTransaction(baseTx);

// Apply simulation, then extract from the single InvokeHostFunction op
const assembled = rpc.assembleTransaction(baseTx, sim).build();
const op = assembled.operations[0]; // Operation.InvokeHostFunction

const funcXdr = op.func.toXDR("base64");
const authXdrs = (op.auth ?? []).map(a => a.toXDR("base64"));
```

## How It Works

1. **Request Validation**: Validates input parameters and extracts Soroban data
2. **Sequence Account Pool**: Acquires an available sequence account
3. **Auth Checking**: Validates authorization entries
4. **Simulation** (if enabled): Simulates transaction and rebuilds with proper resources
5. **Fee Bumping**: Fund account wraps transaction with fee bump
6. **Submission**: Sends to Stellar network

### Concurrent Transaction Processing

This example uses multiple Stellar accounts (fund account + sequence accounts) with `concurrent_transactions: true` enabled in the fund account relayer policy. This configuration:

- **Allows parallel processing**: Fund account leverages the sequence accounts sequence number, so transactions can be processed concurrently without blocking each other
- **Improves throughput**: Multiple transactions can be in-flight simultaneously

## Troubleshooting

### Common issues

- **Plugin not found**: Verify the plugin `id` and `path` in `examples/launchtube-plugin-example/config/config.json`.
- **Missing LaunchTube config**: Ensure `examples/launchtube-plugin-example/launchtube/config.json` exists and is correctly filled.
- **API authentication**: Ensure the `Authorization` header is present and the `API_KEY` is set in `.env`.
- **Webhook not received**: Ensure the `notifications[0].url` is set to a reachable URL.

### View logs

```bash
docker compose -f examples/launchtube-plugin-example/docker-compose.yaml logs -f relayer
```

## Docker notes

This compose file mounts:

```yaml
volumes:
  - ./config:/app/config/
  - ../../config/networks:/app/config/networks
  - ./launchtube:/app/plugins/launchtube
```

The container image already includes the relayer and plugin runtime. You only need to mount your config and the LaunchTube plugin wrapper.

## Learn more

- LaunchTube plugin GitHub: `https://github.com/OpenZeppelin/relayer-plugin-launchtube`
- LaunchTube on npm: `https://www.npmjs.com/package/@openzeppelin/relayer-plugin-launchtube`
- OpenZeppelin Relayer docs: `https://docs.openzeppelin.com/relayer`
