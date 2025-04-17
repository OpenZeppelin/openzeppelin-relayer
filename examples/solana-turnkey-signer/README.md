# Using Turnkey for Secure Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use Turnkey Wallet Private Key to securely sign transactions in OpenZeppelin Relayer.

## Prerequisites

1. A Turnkey account - [Sign up here](https://app.turnkey.com)
2. Rust and Cargo installed
3. Git
4. [Docker](https://docs.docker.com/get-docker/)
5. [Docker Compose](https://docs.docker.com/compose/install/)

## Getting Started

### Step 1: Clone the Repository

Clone this repository to your local machine:

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Set Up Your Turnkey Organization

1. Log in to [Turnkey Console](https://app.turnkey.com)
2. Create a new organization if you haven't already
3. Note down your `Organization ID` - you'll need this later

### Step 3: Create API Credentials

1. Go to the User details -> API Credentials section in Turnkey Console
2. Create a new API credential pair
3. Save both the public and private keys - you'll need these for configuration
4. Note: The private key is only shown once, make sure to save it securely

### Step 4: Create a Wallet

1. In Turnkey Console, go to Wallets section
2. Click "Create private key"
3. Choose the following settings:
   - Curve type: ED25519
   - Asset Address type: Solana
4. Complete the api key creation process
5. Note down the following details:
   - Private Key ID
   - Public Key


### Step 5: Configure the Relayer Service

Create an environment file by copying the example:

```bash
cp examples/solana-turnkey-signer/.env.example examples/solana-turnkey-signer/.env
```

#### Populate Turnkey API Private Key

Edit the `.env` file and update the following variables:

```env
TURNKEY_API_PRIVATE_KEY=your_api_private_key
```

#### Populate Turnkey config

Edit the `config.json` file and update the following variables:

```json
{
  "signers": [
    {
      "id": "turnkey-signer-solana",
      "type": "turnkey",
      "config": {
        "api_public_key": "your_api_public_key",
        "organization_id": "your_organization_id",
        "private_key_id": "your_private_key_id",
        "public_key": "your_public_key"
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

Update the `examples/solana-turnkey-signer/config/config.json` file with your webhook configuration:

1. For testing, get a webhook URL from [Webhook.site](https://webhook.site)
2. Update the config file:

```json
{
  "notifications": [
    {
      "url": "your_webhook_url",
    }
  ]
}
```

### Step 7: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/solana-turnkey-signer/docker-compose.yaml up
```

### Step 8: Test the Service

1. The service exposes a REST API
2. You can test it using curl or any HTTP client:

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

### Troubleshooting

If you encounter issues:

1. Verify your Turnkey credentials are correct
2. Check the service logs for detailed error messages
3. Verify the transaction format matches the expected schema

### Additional Resources

- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
- [Turnkey Documentation](https://docs.turnkey.com/home)
