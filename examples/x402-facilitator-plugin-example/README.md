# OpenZeppelin Relayer — x402 Facilitator Plugin Example

Run the x402 Facilitator plugin with OpenZeppelin Relayer to enable payment verification and settlement for x402 payments on Stellar. The plugin verifies payment payloads, validates authorization, and settles transactions via the relayer or an optional channel service.

## Quick Start

```bash
# Clone and navigate to this example:
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer/examples/x402-facilitator-plugin-example

# Then follow the Setup steps below.
```

## Prerequisites

- Docker and Docker Compose
- Rust (for generating keys and IDs)
- Node.js >= 18 and pnpm >= 10

## Overview

The x402 Facilitator plugin implements the X402 payment facilitation protocol for Stellar. In this Relayer example, it exposes three plugin routes (invoked via `/api/v1/plugins/x402-facilitator/call/{route}`):

- **`verify`**: Verifies a payment payload against payment requirements
- **`settle`**: Settles a verified payment by submitting the transaction on-chain
- **`supported`**: Returns supported payment kinds with current max ledger information

The plugin supports:
- Payment verification with auth entry validation
- Transaction settlement via relayer API or channel service API
- Multiple Stellar network configurations

## Setup

To get started:

1. Install and build the x402 Facilitator plugin
2. Create relayer keys and generate API credentials
3. Configure the plugin in `config/config.json`
4. Start Docker to get account addresses
5. Fund accounts on testnet

All configurations are pre-set for testnet use.

### 1. Install Dependencies

Install and build the x402 Facilitator plugin:

```bash
# From this directory (examples/x402-facilitator-plugin-example)
cd x402-facilitator
pnpm install
pnpm run build
cd ..
```

### 2. Create Keys and Configuration

The x402 Facilitator plugin requires relayer keys for submitting transactions:

- **Relayer account**: Pays transaction fees and submits transactions

From this directory (`examples/x402-facilitator-plugin-example`), run these commands.

#### Create relayer account

```bash
# Replace YOUR_PASSWORD with a unique strong password
# You will need to add this password to your .env file
# Password must contain at least one uppercase letter, one lowercase letter,
# one number, and one special character (e.g., MyPass123!)

# Create relayer account (pays fees and submits transactions)
cargo run --example create_key -- \
  --password YOUR_PASSWORD \
  --output-dir config/keys \
  --filename local-signer.json
```

#### Generate API credentials

```bash
# Generate API key (save this output)
cargo run --example generate_uuid
```

#### Create environment file

Create `.env` in this directory (or start from the provided `.env.example`):

```bash
cp .env.example .env
```

```env
REDIS_URL=redis://redis:6379
KEYSTORE_PASSPHRASE=YOUR_PASSWORD
API_KEY=<api_key_from_above>
```

### 3. Configure Plugin

The `config/config.json` file already contains the relayer and plugin definitions.
The x402 facilitator plugin is configured for the `stellar-example` relayer on the Stellar testnet with USDC support.

```json
{
  "relayers": [
    {
      "id": "stellar-example",
      "name": "Stellar Example",
      "network": "testnet",
      "paused": false,
      "network_type": "stellar",
      "signer_id": "local-signer",
      "policies": {
        "fee_payment_strategy": "relayer",
        "min_balance": 0
      }
    }
  ],
  "notifications": [],
  "signers": [
    {
      "id": "local-signer",
      "type": "local",
      "config": {
        "path": "config/keys/local-signer.json",
        "passphrase": {
          "type": "env",
          "value": "KEYSTORE_PASSPHRASE"
        }
      }
    }
  ],
  "networks": "./config/networks",
  "plugins": [
    {
      "id": "x402-facilitator",
      "path": "x402-facilitator/index.ts",
      "timeout": 30,
      "emit_logs": false,
      "emit_traces": false,
      "forward_logs": true,
      "raw_response": true,
      "allow_get_invocation": true,
      "config": {
        "networks": [
          {
            "network": "stellar:testnet",
            "type": "stellar",
            "relayer_id": "stellar-example",
            "assets": [
              "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA"
            ]
          }
        ]
      }
    }
  ]
}
```

**Plugin Configuration Options:**

- `networks`: Array of network configurations
  - `type`: Network type (e.g., "stellar")
  - `network`: Network identifier (e.g., "stellar:testnet", "stellar:pubnet")
  - `relayer_id`: ID of the relayer to use for this network
  - `assets`: Array of supported asset contract addresses
  - `channel_service_api_url` (optional): Channel service API URL for settlement
  - `channel_service_api_key` (optional): Channel service API key


### 4. Start the Service and Get Account Addresses

```bash
docker compose up
```

Once the relayer is running, retrieve the relayer address via:

```bash
curl -X GET http://localhost:8080/api/v1/relayers/stellar-example \
  -H "Authorization: Bearer YOUR_API_KEY"
```

### 5. Fund Your Account on Testnet

In a new terminal, copy the address from the logs above and fund it:

```bash
# Replace with your actual address from the logs
curl "https://friendbot.stellar.org?addr=YOUR_RELAYER_ADDRESS"
```

## Usage

The plugin implements the API defined by the x402 v2 spec, so it works with any packages in that ecosystem. For more information on available packages, see `https://github.com/coinbase/x402`.


### x402-express example

To use OpenZeppelin Relayer and its x402-facilitator plugin with x402-express (and similar packages), point the facilitator to your Relayer plugin URL and pass the Relayer API key via `createAuthHeaders`.

```.env
STELLAR_ADDRESS=
FACILITATOR_URL=http://localhost:8080/api/v1/plugins/x402-facilitator/call
```

```typescript
import { config } from "dotenv";
import express from "express";
import { paymentMiddleware, x402ResourceServer } from "@x402/express";
import { ExactStellarScheme } from "@x402/stellar/exact/server";
import { HTTPFacilitatorClient } from "@x402/core/server";
config();

const stellarAddress = process.env.STELLAR_ADDRESS as string | undefined;

// Validate stellar address is provided
if (!stellarAddress) {
  console.error("❌ STELLAR_ADDRESS is required");
  process.exit(1);
}

const facilitatorUrl = process.env.FACILITATOR_URL;
if (!facilitatorUrl) {
  console.error("❌ FACILITATOR_URL environment variable is required");
  process.exit(1);
}
const facilitatorClient = new HTTPFacilitatorClient({ url: facilitatorUrl, createAuthHeaders: async () => ({
  // Use your Relayer API key for the plugin
  verify: { Authorization: "Bearer RELAYER_API_KEY" },
  settle: { Authorization: "Bearer RELAYER_API_KEY" },
  supported: { Authorization: "Bearer RELAYER_API_KEY" },
})});

const app = express();

app.use(
  paymentMiddleware(
    {
      "GET /weather": {
        accepts: [
          {
            scheme: "exact",
            price: "$0.001",
            network: "stellar:testnet",
            payTo: stellarAddress,
          },
        ],
        description: "Weather data",
        mimeType: "application/json",
      },
    },
    new x402ResourceServer(facilitatorClient)
      .register("stellar:testnet", new ExactStellarScheme())

  ),
);

app.get("/weather", (req, res) => {
  res.send({
    report: {
      weather: "sunny",
      temperature: 70,
    },
  });
});

app.listen(4021, () => {
  console.log(`Server listening at http://localhost:${4021}`);
});
```

### Server Setup (Express)

Set up an Express server with x402 payment middleware. For detailed instructions, see the [x402 Express server guide](https://github.com/coinbase/x402/tree/main/examples/typescript/servers/advanced/README.md).

### Client Setup (Fetch)

Make requests to x402-protected endpoints using the fetch client. For detailed instructions, see the [x402 Fetch client guide](https://github.com/coinbase/x402/blob/main/examples/typescript/clients/advanced/README.md).

## How It Works

### Verification Flow

1. **Protocol Validation**: Validates X402 version, scheme, and network
2. **Transaction Parsing**: Decodes transaction XDR and extracts operation details
3. **Operation Validation**: Ensures it's an `invokeHostFunction` calling `transfer`
4. **Amount & Recipient Validation**: Validates transfer amount and recipient match requirements
5. **Auth Entry Validation**: Verifies auth entries are present and signed by the payer
6. **Envelope Signature Check**: Ensures transaction envelope has no signatures (for relayer rebuild)
7. **Expiration**: Validate auth entry expiration is within allowed window
8. **Simulation**: Simulates transaction to ensure it will succeed
9. **Security Checks**: Validates transaction source is not the relayer

### Settlement Flow

1. **Verification**: Verifies payment before settlement
2. **Operation Extraction**: Extracts operation details and signed auth entries from transaction
3. **Submission**:
   - **If channel service configured**: Submits via channel service API with `func` and `auth` XDRs
   - **Otherwise**: Submits via relayer API with operations format
4. **Confirmation**: Waits for transaction confirmation

### Channel Service vs Relayer API

The plugin supports two settlement methods:

- **Relayer API** (default): Uses the relayer's `sendTransaction` with operations format
- **Channel Service API** (optional): Uses external channel service when `channel_service_api_url` and `channel_service_api_key` are configured

## Learn more

- OpenZeppelin Relayer docs: `https://docs.openzeppelin.com/relayer`
- Stellar SDK: `https://stellar.github.io/js-stellar-sdk/`
- Coinbase x402 GitHub repo: `https://github.com/coinbase/x402`
