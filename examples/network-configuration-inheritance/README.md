# OpenZeppelin Relayer - Network Configuration - Inheritance Example

This example demonstrates how to use **network inheritance** to create network configurations that inherit from base networks. This approach reduces duplication and makes it easier to manage multiple related networks.

## Key Features Demonstrated

- **Network Inheritance**: Child networks inherit configuration from parent networks using the `from` field
- **Configuration Override**: Child networks can override specific fields from parent networks

## How Network Inheritance Works

Network inheritance allows you to:

1. **Define a base network** with common configuration
2. **Create child networks** that inherit from the base using `from` field
3. **Override specific fields** in child networks as needed
4. **Maintain consistency** across related networks

### Inheritance Rules

- Child networks inherit **all fields** from the parent network
- Fields specified in child networks **override** parent values
- The `from` field must reference a network of the **same type** (evm, solana, stellar)

## Configuration Structure

```json
{
  "networks": [
    {
      "type": "evm",
      "network": "sepolia-base",
      "chain_id": 11155111,
      "required_confirmations": 6,
      "rpc_urls": ["https://sepolia.drpc.org"],
      // ... base configuration
    },
    {
      "from": "sepolia-base",           // Inherits from sepolia-base
      "type": "evm",
      "network": "sepolia-custom",
      "required_confirmations": 3,     // Overrides parent value
      "chain_id": 34343443,      // Overrides parent value
      "rpc_urls": ["https://custom-rpc.example.com"],  // Overrides parent value
      "tags": ["ethereum", "testnet", "custom"]        // Overrides parent value
    }
  ]
}
```

## Example Networks in This Configuration

### EVM Networks (Sepolia-based)

#### Base Network: `sepolia-base`

- **Chain ID**: 11155111 (Sepolia)
- **Confirmations**: 6 blocks
- **RPC URLs**: Standard Sepolia endpoints
- **Features**: EIP-1559 support
- **Tags**: Basic testnet tags

#### Inherited Network: `sepolia-custom`

- **Inherits from**: `sepolia-base`
- **Overrides**:
  - Confirmations: 3 blocks (faster finality)
  - RPC URLs: Custom endpoints
  - Tags: Adds "custom" tag

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

Create a new signer keystore:

```bash
cargo run --example create_key -- \
  --password <DEFINE_YOUR_PASSWORD> \
  --output-dir examples/network-configuration-inheritance/config/keys \
  --filename local-signer.json
```

### Step 3: Environment Configuration

Create the environment file:

```bash
cp examples/basic-example/.env.example examples/network-configuration-inheritance/.env
```

Update the `.env` file with your configuration:

- `REDIS_URL`: Redis server url
- `KEYSTORE_PASSPHRASE`: The password you used for the keystore
- `WEBHOOK_SIGNING_KEY`: Generate using `cargo run --example generate_uuid`
- `API_KEY`: Generate using `cargo run --example generate_uuid`

### Step 4: Configure Webhook URL

Update the `url` field in the notifications section of `config/config.json`. For testing, use [Webhook.site](https://webhook.site).

### Step 5: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/network-configuration-inheritance/docker-compose.yaml up
```

The service will be available at `http://localhost:8081/api/v1` (note the different port to avoid conflicts).

## Testing the Configuration

### Check Available Relayers

```bash
curl -X GET http://localhost:8081/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY"
```

## Configuration Analysis

### What Gets Inherited

When `sepolia-custom` inherits from `sepolia-base`, it receives:

```json
{
  "type": "evm",
  "network": "sepolia-custom",        // Own value
  "chain_id": 11155111,               // From parent
  "required_confirmations": 3,        // Overridden value
  "symbol": "ETH",                    // From parent
  "features": ["eip1559"],            // From parent
  "rpc_urls": [                       // Overridden value
    "https://ethereum-sepolia-rpc.publicnode.com",
    "https://ethereum-sepolia-public.nodies.app"
  ],
  "explorer_urls": [                  // From parent
    "https://api-sepolia.etherscan.io/api",
    "https://sepolia.etherscan.io"
  ],
  "average_blocktime_ms": 12000,      // From parent
  "is_testnet": true,                 // From parent
  "tags": ["ethereum", "testnet", "custom"]  // Overridden value
}
```

## Advantages of Network Inheritance

1. **Reduced Duplication**: Share common configuration across related networks
2. **Consistency**: Ensure base settings are consistent across variants
3. **Maintainability**: Update base configuration in one place
4. **Flexibility**: Override specific settings for different environments
5. **Scalability**: Easy to add new network variants

## See Also

- [Network Configuration Example](../network-configuration-config-file/README.md) - Shows usage of network configuration via config file.
