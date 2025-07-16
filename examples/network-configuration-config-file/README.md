# OpenZeppelin Relayer - Network Configuration - Config File Example

This example demonstrates how to configure networks **directly within the main config.json file** instead of using separate network files. This approach is useful for simple setups or when you want to keep all configuration in a single file.

## Key Features Demonstrated

- **Config file Network Configuration**: Networks are defined directly in the `networks` array within `config.json`

## Configuration Structure

In this example, networks are defined directly in the main `config.json` file:

```json
{
  "relayers": [...],
  "signers": [...],
  "notifications": [...],
  "networks": [
    {
      "type": "evm",
      "network": "sepolia",
      "chain_id": 11155111,
      // ... other network settings
    }
  ]
}
```

### Network Configuration Details

#### EVM Network (Sepolia)

- **Network Type**: `evm`
- **Chain ID**: `11155111` (Sepolia testnet)
- **Confirmations**: `6` blocks for transaction finality
- **Features**: Supports EIP-1559 fee market
- **Multiple RPC URLs**: For redundancy and reliability
- **Explorer Integration**: Etherscan API and web interface

## Getting Started

### Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)
- Rust (for key generation tools)

### Step 1: Clone the Repository

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Create a Signer

Create a new signer keystore using the provided key generation tool:

```bash
cargo run --example create_key -- \
  --password <DEFINE_YOUR_PASSWORD> \
  --output-dir examples/network-direct-config/config/keys \
  --filename local-signer.json
```

**Note**: Replace `<DEFINE_YOUR_PASSWORD>` with a strong password for the keystore.

### Step 3: Environment Configuration

Create the environment file:

```bash
cp examples/basic-example/.env.example examples/network-direct-config/.env
```

Update the `.env` file with your configuration:

- `REDIS_URL`: Redis server url
- `KEYSTORE_PASSPHRASE`: The password you used for the keystore
- `WEBHOOK_SIGNING_KEY`: Generate using `cargo run --example generate_uuid`
- `API_KEY`: Generate using `cargo run --example generate_uuid`

### Step 4: Configure Webhook URL

Update the `url` field in the notifications section of `config/config.json`. For testing, you can use [Webhook.site](https://webhook.site) to get a test URL.

### Step 5: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/network-configuration-config-file/docker-compose.yaml up
```

The service will be available at `http://localhost:8080/api/v1`

## Testing the Configuration

### Check Available Relayers

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY"
```

## Advantages of Direct Network Configuration

1. **Simplicity**: All configuration in one file
2. **Transparency**: Easy to see all network settings at a glance
3. **Version Control**: Single file to track changes
4. **Deployment**: Simpler deployment with fewer files

## When to Use This Approach

- **Small Deployments**: When managing a few networks
- **Simple Setups**: When you don't need complex network hierarchies
- **Testing**: For development and testing environments
- **Single Team**: When one team manages all network configurations

## See Also

- [Network Configuration Inheritance Example](../network-configuration-inheritance/README.md) - Shows how to use network inheritance.
