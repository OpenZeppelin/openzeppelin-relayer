# OpenZeppelin Relayer — Channels Plugin Example

Run the Channels plugin with OpenZeppelin Relayer to enable parallel transaction submission on Stellar using channel accounts with fee bumping. The plugin handles fees, sequence numbers, simulation, and retries automatically.

## Quick Start

```bash
# Clone and navigate to this example:
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer/examples/channels-plugin-example

# Then follow the Setup steps below
```

## Prerequisites

- Docker and Docker Compose
- Rust (for generating keys and IDs)
- Node.js >= 18 and pnpm >= 10

## Setup

You only need to:

1. Install and build the Channels plugin
2. Create the keys for channel accounts
3. Set up environment variables
4. Start Docker to get account addresses
5. Fund accounts on testnet

All configurations are pre-set for testnet use.

### 1. Install Dependencies

Install and build the Channels plugin:

```bash
# From this directory (examples/channels-plugin-example)
cd channel
pnpm install
pnpm run build
cd ..
```

### 2. Create Keys and Configuration

The Channels plugin requires two types of keys:

- **Fund account**: Pays transaction fees and holds funds
- **Channel accounts**: Manage sequence numbers for parallel transactions (at least 2 recommended)

From this directory (`examples/channels-plugin-example`), run these commands:

#### Create Channel accounts

```bash
# Replace each YOUR_PASSWORD with a unique strong password for each key
# You will need to add these passwords to your .env file
# Password must contain at least one uppercase letter, one lowercase letter,
# one number, and one special character (e.g., MyPass123!)

# Create fund account (pays fees)
cargo run --example create_key -- \
  --password YOUR_PASSWORD \
  --output-dir config/keys \
  --filename channels-fund.json

# Create first channel account
cargo run --example create_key -- \
  --password YOUR_PASSWORD \
  --output-dir config/keys \
  --filename channel-001.json

# Create second channel account (recommended for better throughput)
cargo run --example create_key -- \
  --password YOUR_PASSWORD \
  --output-dir config/keys \
  --filename channel-002.json
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
KEYSTORE_PASSPHRASE_CHANNEL_001=YOUR_PASSWORD
KEYSTORE_PASSPHRASE_CHANNEL_002=YOUR_PASSWORD
WEBHOOK_SIGNING_KEY=<webhook_key_from_above>
API_KEY=<api_key_from_above>
# Channels Configuration
STELLAR_NETWORK=testnet
PLUGIN_ADMIN_SECRET=<admin_secret_for_channels_mgmt_api>
SOROBAN_RPC_URL=https://soroban-testnet.stellar.org
FUND_RELAYER_ID=channels-fund
LOCK_TTL_SECONDS=30
LOG_LEVEL=info
```

### 3. Verify Configuration

The Channels plugin and relayer configurations are already set up for testnet. The configurations include:

**`config/config.json`** (pre-configured):

- Three relayers defined: `channels-fund`, `channel-001`, `channel-002`
- The Fund relayer has `concurrent_transactions: true` enabled in policies to allow parallel processing
- Corresponding signers pointing to the key files you'll create
- Plugin registered as `channels`

**Channels Configuration** (via environment variables):

Channels is configured through environment variables in your `.env` file:

- `STELLAR_NETWORK=testnet` - Sets the Stellar network
- `SOROBAN_RPC_URL=https://soroban-testnet.stellar.org` - RPC endpoint
- `FUND_RELAYER_ID=channels-fund` - ID of the fund relayer
- `PLUGIN_ADMIN_SECRET` - Admin secret for Channels operations
- `LOCK_TTL_SECONDS=30` - Lock timeout for sequence management

### 4. (Optional) Configure Webhooks

For transaction notifications, edit `config/config.json` to add a webhook notification:

```json
{
  "notifications": [
    {
      "id": "webhook-notification",
      "type": "webhook",
      "url": "https://webhook.site/your-unique-id",
      "signing_key": {
        "type": "env",
        "value": "WEBHOOK_SIGNING_KEY"
      }
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
relayer-1  | Syncing sequence for relayer: channels-fund (GCP7KWGZCDDVBFKANDJTA74H2HSORV34SMSQIPGZ3PK7V6OHKCFGRTF6)
relayer-1  | Syncing sequence for relayer: channel-001 (GCWFXU6HZNHLTXMHWZRPXYBZFOODJYRDZXFOPMUQN4S2JJGEZA2ZHA4B)
relayer-1  | Syncing sequence for relayer: channel-002 (GA7IXWK3VKF25JOXJZZ7XMFB3A3IPM5A66MW5DJ6FPOIWME4F66UK4HL)
```

### 6. Fund Your Accounts on Testnet

In a new terminal, copy the addresses from the logs above and fund them:

```bash
# Replace with your actual addresses from the logs
curl "https://friendbot.stellar.org?addr=YOUR_FUND_ADDRESS"        # fund account
curl "https://friendbot.stellar.org?addr=YOUR_CHANNEL_001_ADDRESS"  # channel-001
curl "https://friendbot.stellar.org?addr=YOUR_CHANNEL_002_ADDRESS"  # channel-002
```

After funding, restart the service for it to recognize the funded accounts:

```bash
# Stop the service with Ctrl+C, then restart
docker compose up
```

### 7. Initialize Channel Accounts

**Critical Step**: Before Channels can process transactions, you must configure the channel accounts using the Management API:

```bash
# Replace YOUR_API_KEY with your actual API key from the .env file
# Replace YOUR_ADMIN_SECRET with your PLUGIN_ADMIN_SECRET from the .env file
curl -X POST http://localhost:8080/api/v1/plugins/channels/call \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "management": {
        "action": "setChannelAccounts",
        "adminSecret": "YOUR_ADMIN_SECRET",
        "relayerIds": ["channel-0001", "channel-0002", "channel-0003"]
      }
    }
  }'
```

**Expected Response (HTTP 200):**

```json
{
  "success": true,
  "data": {
    "result": {
      "ok": true,
      "appliedRelayerIds": ["channel-0001", "channel-0002", "channel-0003"]
    }
  },
  "error": null
}
```

## Usage

### Test Connection

```bash
curl -X GET http://localhost:8080/api/v1/plugins \
  -H "Authorization: Bearer YOUR_API_KEY"
```

### Submit Transactions

#### Option 1: Signed Transaction XDR (no channel)

```bash
curl -X POST http://localhost:8080/api/v1/plugins/channels/call \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "xdr": "AAAAAgAAAAA..."
    }
  }'
```

#### Option 2: Soroban Function + Auth (uses channel accounts)

```bash
curl -X POST http://localhost:8080/api/v1/plugins/channels/call \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "func": "AAAABAAAAAEAAAAGc3ltYm9s...",
      "auth": ["AAAACAAAAAEAAAA..."]
    }
  }'
```

**Parameters:**

- `xdr`: Complete signed transaction envelope XDR (not fee-bump)
- `func`: Soroban host function XDR
- `auth`: Array of authorization entry XDRs

> Use either `xdr` OR `func`+`auth`, not both

**Response (HTTP 200):**

```json
{
  "success": true,
  "data": {
    "result": {
      "transactionId": "tx_123456",
      "status": "confirmed",
      "hash": "1234567890abcdef..."
    }
  },
  "error": null
}
```

## Local Plugin Development (Swap Built Output Only)

If you're actively developing `@openzeppelin/relayer-plugin-channels`, you can replace only the installed package's built output (`dist/`) with your local build — no code or package.json changes required.

### One-time setup

1. Set an environment variable pointing to your local plugin directory:

```bash
export CHANNELS_PLUGIN_DIR=/path/to/your/relayer-plugin-channels
```

Replace `/path/to/your/relayer-plugin-channels` with the actual path to your local plugin repository.

2. Build the plugin so `dist/` exists (or run a watch build):

```bash
pnpm -C $CHANNELS_PLUGIN_DIR install
pnpm -C $CHANNELS_PLUGIN_DIR build
```

### Start with the dev override

`docker-compose.plugin-dev.yaml` mounts your local plugin's `dist/` over the installed package's `dist/` in the container.

```bash
export CHANNELS_PLUGIN_LOCAL_DIST=$CHANNELS_PLUGIN_DIR/dist
docker compose -f docker-compose.yaml -f docker-compose.plugin-dev.yaml up -d --build
```

Under the hood:

- Your local `dist/` is mounted at `/app/plugins/channel/node_modules/@openzeppelin/relayer-plugin-channels/dist` inside the container.
- Dependencies remain intact from the installed npm package; only the runtime JS is swapped.

### Iterating on code

1. Edit code in your plugin repo.
2. Rebuild the plugin outputs (e.g., `pnpm -C $CHANNELS_PLUGIN_DIR build`).
3. Restart the relayer to reload the module: `docker compose restart relayer`.

### Management API

Channels provides a management API to dynamically configure channel accounts. This requires the `PLUGIN_ADMIN_SECRET` from your `.env` file.

#### List Current Channel Accounts

```bash
curl -X POST http://localhost:8080/api/v1/plugins/channels/call \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "management": {
        "action": "listChannelAccounts",
        "adminSecret": "YOUR_ADMIN_SECRET"
      }
    }
  }'
```

#### Add or Update Channel Accounts

```bash
curl -X POST http://localhost:8080/api/v1/plugins/channels/call \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "management": {
        "action": "setChannelAccounts",
        "adminSecret": "YOUR_ADMIN_SECRET",
        "relayerIds": ["channel-0001", "channel-0002", "channel-0003"]
      }
    }
  }'
```

**Important Notes:**

- You must configure at least one channel account before Channels can process `func`+`auth` transactions
- The management API will prevent removing accounts that are currently locked (in use)
- All relayer IDs must exist in your OpenZeppelin Relayer configuration

### Generating XDR for the Relayer

Use [@stellar/stellar-sdk](https://stellar.github.io/js-stellar-sdk/TransactionBuilder.html) to export either a signed transaction envelope XDR, or Soroban `func` + `auth` XDRs.

#### Signed transaction envelope XDR

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

// Sign with your account
tx.sign(keypair);

// Export base64 envelope XDR
const envelopeXdr = tx.toXDR();
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
2. **Channel Account Pool** (for func+auth): Acquires an available channel account
3. **Auth Checking**: Validates authorization entries
4. **Simulation** (for func+auth): Simulates transaction and rebuilds with proper resources
5. **Fee Bumping**: Fund account wraps transaction with fee bump
6. **Submission**: Sends to Stellar network

### Concurrent Transaction Processing

This example uses multiple Stellar accounts (fund account + channel accounts) with `concurrent_transactions: true` enabled in the fund account relayer policy. This configuration:

- **Allows parallel processing**: Fund account leverages the channel accounts' sequence numbers, so transactions can be processed concurrently without blocking each other
- **Improves throughput**: Multiple transactions can be in-flight simultaneously

## Troubleshooting

### Common issues

- **Plugin not found**: Verify the plugin `id` and `path` in `examples/channels-plugin-example/config/config.json`.
- **Missing Channels config**: Ensure required environment variables are set in `.env` and that channel accounts are configured via the Management API.
- **API authentication**: Ensure the `Authorization` header is present and the `API_KEY` is set in `.env`.
- **Webhook not received**: Ensure the `notifications[0].url` is set to a reachable URL.

### View logs

```bash
docker compose -f examples/channels-plugin-example/docker-compose.yaml logs -f relayer
```

## Docker notes

This compose file mounts:

```yaml
volumes:
  - ./config:/app/config/
  - ../../config/networks:/app/config/networks
  - ./channel:/app/plugins/channel
```

The container image already includes the relayer and plugin runtime. You only need to mount your config and the Channels plugin wrapper.

## Learn more

- Channels plugin GitHub: `https://github.com/OpenZeppelin/relayer-plugin-channels`
- Channels on npm: `https://www.npmjs.com/package/@openzeppelin/relayer-plugin-channels`
- OpenZeppelin Relayer docs: `https://docs.openzeppelin.com/relayer`
