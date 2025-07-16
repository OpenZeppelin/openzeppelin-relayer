# OpenZeppelin Relayer - Network Configuration - Config File Example

This example demonstrates how to configure networks **directly within the main config.json file** with **network inheritance support**. This approach is useful for simple setups or when you want to keep all configuration in a single file while still leveraging inheritance to reduce duplication.

## Key Features Demonstrated

- **Config file Network Configuration**: Networks are defined directly in the `networks` array within `config.json`
- **Network Inheritance**: Child networks inherit configuration from parent networks using the `from` field
- **Selective Overrides**: Child networks can override specific fields like confirmations, RPC URLs, and tags

## Configuration Structure

In this example, networks are defined directly in the main `config.json` file with inheritance support:

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
      "required_confirmations": 6,
      "symbol": "ETH",
      "features": ["eip1559"],
      "rpc_urls": [
        "https://sepolia.drpc.org",
        "https://1rpc.io/sepolia"
      ],
      "explorer_urls": [
        "https://api-sepolia.etherscan.io/api",
        "https://sepolia.etherscan.io"
      ],
      "average_blocktime_ms": 12000,
      "is_testnet": true,
      "tags": ["ethereum", "testnet"]
    },
    {
      "from": "sepolia",
      "type": "evm",
      "network": "sepolia-custom",
      "required_confirmations": 3,
      "rpc_urls": [
        "https://ethereum-sepolia-rpc.publicnode.com",
        "https://ethereum-sepolia-public.nodies.app"
      ],
      "chain_id": 343434324,
      "tags": ["ethereum", "testnet", "custom"]
    }
  ]
}
```

## How Network Inheritance Works

Network inheritance in this example allows you to:

1. **Define a base network** (sepolia) with comprehensive configuration
2. **Create child networks** (sepolia-custom) that inherit from the base using the `from` field
3. **Override specific fields** in child networks as needed for their requirements
4. **Maintain consistency** across related networks while allowing customization

### Inheritance Rules

- Child networks inherit **all fields** from the parent network specified in `from`
- Fields explicitly defined in child networks **override** inherited values
- The `from` field must reference another network within the same `networks` array
- Inheritance works within the same network type (e.g., all EVM networks)

### Network Configuration Details

#### Network Inheritance Pattern

In this configuration:

1. **Base Network (Sepolia)**: Serves as the foundation with comprehensive configuration including all EVM settings
2. **Child Network (Sepolia-Custom)**: Inherits from sepolia using `"from": "sepolia"` and overrides specific fields:
   - `required_confirmations`: Reduced from 6 to 3 for faster testing
   - `rpc_urls`: Different RPC endpoints for custom requirements
   - `chain_id`: Custom chain identifier (343434324)
   - `tags`: Extended tags including "custom" identifier


#### EVM Network Examples

- **Sepolia**: Base testnet configuration with standard settings
- **Sepolia-Custom**: Child network inheriting from sepolia with customized confirmations, RPC URLs, and chain ID for specific testing requirements

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
  --output-dir examples/network-configuration-config-file/config/keys \
  --filename local-signer.json
```

**Note**: Replace `<DEFINE_YOUR_PASSWORD>` with a strong password for the keystore.

### Step 3: Environment Configuration

Create the environment file:

```bash
cp examples/network-configuration-config-file/.env.example examples/network-configuration-config-file/.env
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

## Advantages of Direct Network Configuration with Inheritance

1. **Simplicity**: All configuration in one file with inheritance support
2. **Reduced Duplication**: Child networks inherit common settings, eliminating repeated configuration
3. **Transparency**: Easy to see all network settings and relationships at a glance
4. **Version Control**: Single file to track changes with clear inheritance patterns
5. **Deployment**: Simpler deployment with fewer files while maintaining configuration flexibility
6. **Quick Prototyping**: Perfect for testing network variations without complex file structures

## When to Use This Approach

- **Small to Medium Deployments**: When managing a moderate number of related networks
- **Development and Testing**: For creating network variations during development
- **Single File Preference**: When you want all configuration centralized but still need inheritance
- **Rapid Prototyping**: When you need to quickly test different network configurations
- **Team Simplicity**: When teams prefer a single configuration file with clear inheritance patterns
- **Custom Network Testing**: When creating multiple variations of a base network for testing

## See Also

- [Network Configuration JSON File Example](../network-configuration-json-file/README.md) - Shows how to use separate JSON files with inheritance for better organization.
