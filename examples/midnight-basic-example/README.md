# OpenZeppelin Relayer Midnight Example

This comprehensive guide demonstrates how to configure and use the OpenZeppelin Relayer service with Midnight blockchain, a privacy-preserving network that uses zero-knowledge proofs to protect transaction data.

## Table of Contents

- [Overview](#overview)
- [Prerequisites](#prerequisites)
- [Getting Started](#getting-started)
  - [Step 1: Clone the Repository](#step-1-clone-the-repository)
  - [Step 2: Create a Signer](#step-2-create-a-signer)
  - [Step 3: Configure Environment](#step-3-configure-environment)
  - [Step 4: Configure Notifications](#step-4-configure-notifications)
  - [Step 5: Configure API Key](#step-5-configure-api-key)
  - [Step 6: Run the Service](#step-6-run-the-service)
  - [Step 7: Verify the Setup](#step-7-verify-the-setup)
- [Testing the Relayer](#testing-the-relayer)
- [Architecture](#architecture)
- [Troubleshooting](#troubleshooting)
- [Advanced Configuration](#advanced-configuration)

## Overview

Midnight is a data protection blockchain that enables developers to build applications that safeguard personal and commercial data while ensuring regulatory compliance. This example demonstrates:

- Setting up a Midnight testnet relayer
- Configuring the prover service for zero-knowledge proof generation
- Submitting privacy-preserving transactions
- Monitoring transaction status

### Key Features

- **Privacy-Preserving Transactions**: Use zero-knowledge proofs to keep transaction details private
- **Prover Service Integration**: Automatic proof generation for transactions
- **Docker-based Setup**: Easy deployment with Docker Compose
- **Complete API Examples**: Ready-to-use curl commands for testing

## Prerequisites

Before you begin, ensure you have the following installed:

- [Docker](https://docs.docker.com/get-docker/) (version 20.10 or later)
- [Docker Compose](https://docs.docker.com/compose/install/) (version 2.0 or later)
- [Rust](https://www.rust-lang.org/tools/install) (version 1.86 or later, for key generation tools)
- [jq](https://stedolan.github.io/jq/) (optional, for JSON formatting)
- Access to Midnight testnet (wallet seeds for testing)
- **[TEMPORARY]** GitHub credentials with access to private Midnight repositories

> **⚠️ TEMPORARY REQUIREMENT - WILL BE REMOVED**
>
> Currently, the Midnight blockchain repositories are private. To build the relayer, you need:
>
> - A GitHub username with access to the Midnight repositories
> - A GitHub Personal Access Token (PAT) or password
>
> **This requirement is temporary.** Once the Midnight repositories are open-sourced, all GitHub authentication code will be removed from this example.

## Getting Started

### Step 1: Clone the Repository

Clone the OpenZeppelin Relayer repository and navigate to the project directory:

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Create a Signer

The relayer needs a signer to authorize transactions. Create a new signer keystore using the provided key generation tool:

```bash
cargo run --example create_key -- \
  --password <YOUR_SECURE_PASSWORD> \
  --output-dir examples/midnight-basic-example/config/keys \
  --filename local-signer.json
```

**Important Notes:**

- Replace `<YOUR_SECURE_PASSWORD>` with a strong password (minimum 12 characters recommended)
- Keep this password secure - you'll need it in the next step
- The generated keystore file contains your private key encrypted with this password

### Step 3: Configure Environment

Create the environment configuration file from the template:

```bash
cp examples/midnight-basic-example/.env.example examples/midnight-basic-example/.env
```

Edit the `.env` file and populate the following values:

```bash
# The password you used in Step 2
KEYSTORE_PASSPHRASE=your_keystore_password_here

# Will be generated in Step 4
WEBHOOK_SIGNING_KEY=

# Will be generated in Step 5
API_KEY=
```

### Step 4: Configure Notifications

The relayer can send webhook notifications for transaction status updates.

#### Option A: Using Webhook.site (Development)

For quick testing:

1. Visit [Webhook.site](https://webhook.site)
2. Copy your unique URL (e.g., `https://webhook.site/your-unique-id`)
3. Edit `examples/midnight-basic-example/config/config.json`
4. Update the `notifications[0].url` field with your webhook URL

#### Option B: Using Your Own Webhook Endpoint

If you have your own webhook endpoint, update the URL accordingly.

#### Generate Webhook Signing Key

Generate a signing key to secure webhook payloads:

```bash
cargo run --example generate_uuid
```

Copy the generated UUID and update the `WEBHOOK_SIGNING_KEY` in your `.env` file.

### Step 5: Configure API Key

Generate an API key for authenticating requests to the relayer:

```bash
cargo run --example generate_uuid
```

Copy the generated UUID and update the `API_KEY` in your `.env` file.

Your `.env` file should now look similar to:

```bash
API_KEY=a1b2c3d4-e5f6-7890-abcd-ef1234567890
WEBHOOK_SIGNING_KEY=f9e8d7c6-b5a4-3210-fedc-ba0987654321
KEYSTORE_PASSPHRASE=YourSecurePassword123!
```

### Step 6: Run the Service

Start all services with Docker Compose:

```bash
docker compose -f examples/midnight-basic-example/docker-compose.yaml up
```

This command starts:

- **Midnight Prover Server**: Generates zero-knowledge proofs (port 6300)
- **OpenZeppelin Relayer**: Manages transactions (port 8080)
- **Redis**: Stores relayer state (port 6379)

**Note**: The `MIDNIGHT_LEDGER_TEST_STATIC_DIR` environment variable is automatically set to `/tmp/midnight-test-static` as required by the Midnight test environment.

Wait for the services to fully initialize. You should see logs indicating:

- Prover server is running on port 6300
- Relayer is listening on port 8080
- Redis is ready to accept connections

### Step 7: Verify the Setup

Check that all services are running correctly:

```bash
# Check relayer health
curl -X GET http://localhost:8080/health

# List configured relayers
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY" | jq
```

Expected response:

```json
{
  "success": true,
  "data": [
    {
      "id": "midnight-testnet-example",
      "name": "Midnight Testnet Example",
      "network": "testnet",
      "status": "active",
      ...
    }
  ]
}
```

## Testing the Relayer

### 1. Get Relayer Balance

Check the balance of your relayer account:

```bash
curl -X GET http://localhost:8080/api/v1/relayers/midnight-testnet-example/balance \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY" | jq
```

### 2. Submit a Test Transaction

Submit a simple value transfer transaction:

```bash
curl -X POST http://localhost:8080/api/v1/relayers/midnight-testnet-example/transactions \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY" \
  -d '{
    "guaranteed_offer": {
      "inputs": [
        {
          "origin": "{RELAYER_WALLET_SEED}",
          "token_type": "02000000000000000000000000000000000000000000000000000000000000000000",
          "value": "1000000"
        }
      ],
      "outputs": [
        {
          "destination": "{DESTINATION_WALLET_SEED}",
          "token_type": "02000000000000000000000000000000000000000000000000000000000000000000",
          "value": "1000000"
        }
      ]
    },
    "intents": [],
    "fallible_offers": [],
    "ttl": "2025-12-31T23:59:59Z"
  }' | jq
```

### 3. Check Transaction Status

Replace `{transaction_id}` with the ID from the submission response:

```bash
curl -X GET http://localhost:8080/api/v1/relayers/midnight-testnet-example/transactions/{transaction_id} \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY" | jq
```

### 4. List Recent Transactions

```bash
curl -X GET "http://localhost:8080/api/v1/relayers/midnight-testnet-example/transactions?per_page=5" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY" | jq
```

## Architecture

The example setup consists of three main components:

```
┌─────────────────────────────────────────────────────────┐
│                    Docker Network                        │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐ │
│  │   Prover     │  │   Relayer    │  │    Redis     │ │
│  │   Server     │◄─┤   Service    ├─►│   Storage    │ │
│  │  Port: 6300  │  │  Port: 8080  │  │  Port: 6379  │ │
│  └──────────────┘  └──────────────┘  └──────────────┘ │
│         ▲                 │                             │
│         │                 ▼                             │
│  ┌──────────────────────────────────────┐              │
│  │     Midnight Testnet Network         │              │
│  └──────────────────────────────────────┘              │
└─────────────────────────────────────────────────────────┘
```

### Component Responsibilities

1. **Prover Server**: Generates zero-knowledge proofs for transactions
2. **Relayer Service**: Manages transaction lifecycle and API
3. **Redis**: Stores transaction state and relayer configuration

## Troubleshooting

### Common Issues and Solutions

#### 1. Prover Server Connection Failed

**Error**: "Failed to connect to prover service"

**Solution**:

```bash
# Check if prover is running
docker compose -f examples/midnight-basic-example/docker-compose.yaml ps

# Check prover logs
docker compose -f examples/midnight-basic-example/docker-compose.yaml logs prover

# Restart prover service
docker compose -f examples/midnight-basic-example/docker-compose.yaml restart prover
```

#### 2. Invalid Wallet Seed

**Error**: "Invalid origin/destination format"

**Solution**:

- Ensure wallet seeds are 32-byte hex-encoded strings (64 characters)
- Check that there are no spaces or special characters

#### 3. Insufficient Balance

**Error**: "Insufficient balance for transaction"

**Solution**:

- Fund your relayer account with testnet tokens
- Check balance using the balance endpoint
- Ensure `min_balance` policy is configured appropriately

#### 4. Transaction Timeout

**Error**: "Transaction expired"

**Solution**:

- Increase the TTL value in your transaction request
- Check network connectivity
- Verify the indexer service is accessible

### Checking Logs

View logs for all services:

```bash
docker compose -f examples/midnight-basic-example/docker-compose.yaml logs -f
```

View logs for specific service:

```bash
docker compose -f examples/midnight-basic-example/docker-compose.yaml logs -f relayer
```

## Advanced Configuration

### Custom Network Configuration

To connect to a different Midnight network, edit the `networks` section in `config/config.json`:

```json
{
  "networks": [
    {
      "type": "midnight",
      "network": "your-network-name",
      "rpc_urls": ["http://your-rpc-url:9944"],
      "indexer_urls": {
        "http": "https://your-indexer-url",
        "ws": "wss://your-indexer-ws-url"
      },
      "prover_url": "http://prover:6300",
      "commitment_tree_ttl": 3600
    }
  ]
}
```

### Relayer Policies

Configure relayer behavior by editing the `policies` section:

```json
{
  "policies": {
    "min_balance": 1000000000 // Minimum balance in speck
  }
}
```

### Performance Tuning

Optimize performance by adjusting:

1. **Commitment Tree TTL**: Cache duration for Merkle roots
2. **Rate Limiting**: Adjust `RATE_LIMIT_REQUESTS_PER_SECOND` in docker-compose.yaml
3. **Redis Persistence**: Configure save intervals in docker-compose.yaml

## Security Considerations

1. **Production Deployment**:

   - Use hosted signers (AWS KMS, Google Cloud KMS) instead of local keystores
   - Deploy behind a secure reverse proxy
   - Enable TLS/HTTPS for all communications
   - Implement proper access controls

2. **Key Management**:

   - Never commit `.env` files or keystores to version control
   - Rotate API keys and signing keys regularly
   - Use strong passwords for keystores

3. **Network Security**:
   - Restrict network access to necessary ports only
   - Use private networks for inter-service communication
   - Monitor for unusual transaction patterns

## Next Steps

- Explore the [Midnight documentation](https://midnight.network/docs)
- Learn about [zero-knowledge proofs](https://docs.midnight.network/develop/tutorial/introduction/)
- Join the [OpenZeppelin Telegram community](https://t.me/openzeppelin_tg/2)
- Review the [API documentation](https://release-v1-0-0--openzeppelin-relayer.netlify.app/api_docs.html)

## Support

If you encounter any issues:

1. Check the [troubleshooting section](#troubleshooting)
2. Review logs for error messages
3. Open an issue on [GitHub](https://github.com/OpenZeppelin/openzeppelin-relayer/issues)
4. Join our [Telegram community](https://t.me/openzeppelin_tg/2) for help

## License

This project is licensed under the GNU Affero General Public License v3.0.
