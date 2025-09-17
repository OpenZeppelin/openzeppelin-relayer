# Using CDP for Secure Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use CDP (Coinbase Developer Platform) Wallet to securely sign transactions in OpenZeppelin Relayer.

## Prerequisites

1. A CDP account - [Sign up here](https://portal.cdp.coinbase.com/)
1. Rust and Cargo installed
1. Git
1. [Docker](https://docs.docker.com/get-docker/)
1. [Docker Compose](https://docs.docker.com/compose/install/)

## Getting Started

### Step 1: Clone the Repository

Clone this repository to your local machine:

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Set Up Your CDP Account

1. Log in to [CDP Portal](https://portal.cdp.coinbase.com/)
1. Create a new project if you haven't already
1. Note down your project details - you'll need these later

### Step 3: Create API Credentials

1. Go to the API Keys section in your project dashboard.
1. Create a new Secret API Key.
1. Save both the API Key ID and Secret - you'll need these for configuration.
1. Note: The API Key Secret is only shown once, make sure to save it securely.

### Step 4: Create a Wallet

1. In CDP Portal, go to the Wallets tab, then Server Wallets.
1. Generate a new Wallet Secret if needed.
1. Create a new EVM EOA Wallet using either our [REST API](https://docs.cdp.coinbase.com/api-reference/v2/rest-api/evm-accounts/create-an-evm-account) or an official [CDP SDK](https://github.com/coinbase/cdp-sdk)
1. Note down the following details:
   - EVM Account Address

### Step 5: Configure the Relayer Service

Create an environment file by copying the example:

```bash
cp examples/evm-cdp-signer/.env.example examples/evm-cdp-signer/.env
```

#### Populate CDP API Credentials

Edit the `.env` file and update the following variables:

```env
CDP_API_KEY_SECRET=your_api_key_secret
CDP_WALLET_SECRET=your_wallet_secret
```

#### Populate CDP config

Edit the `config.json` file and update the following variables:

```json
{
  "signers": [
    {
      "id": "cdp-signer-evm",
      "type": "cdp",
      "config": {
        "api_key_id": "YOUR_API_KEY_ID",
        "api_key_secret": {
          "type": "env",
          "value": "CDP_API_KEY_SECRET"
        },
        "wallet_secret": {
          "type": "env",
          "value": "CDP_WALLET_SECRET"
        },
        "account_address": "0xYOUR_EVM_ACCOUNT_ADDRESS"
      }
    }
  ]
}
```

#### Generate Security Keys

Generate random keys for API authentication and webhook signing:

```bash
# Generate API key
cargo run --example generate_uuid

# Generate webhook signing key
cargo run --example generate_uuid
```

Add these to your `.env` file:

```env
WEBHOOK_SIGNING_KEY=generated_webhook_key
API_KEY=generated_api_key
```

#### Configure Webhook URL

Update the `examples/evm-cdp-signer/config/config.json` file with your webhook configuration:

1. For testing, get a webhook URL from [Webhook.site](https://webhook.site)
2. Update the config file:

```json
{
  "notifications": [
    {
      "url": "your_webhook_url"
    }
  ]
}
```

### Step 6: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/evm-cdp-signer/docker-compose.yaml up
```

### Step 7: Test the Service

1. The service exposes a REST API
2. You can test it using curl or any HTTP client:

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer $API_KEY"
```

### Troubleshooting

If you encounter issues:

1. Verify your CDP credentials are correct
2. Check the service logs for detailed error messages
3. Verify the transaction format matches the expected schema
4. Ensure your CDP wallet has sufficient permissions for signing

### Additional Resources

- [CDP Documentation](https://docs.cdp.coinbase.com/)
- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
