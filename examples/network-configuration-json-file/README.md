# OpenZeppelin Relayer - Network Configuration - JSON File Example

This example demonstrates how to configure networks using **JSON files with inheritance**, where testnets inherit common configuration from their corresponding mainnets. This approach reduces duplication while maintaining network-specific customizations.

## Key Features Demonstrated

- **JSON File Network Configuration**: Networks are defined in separate JSON files referenced from `config.json`
- **Network Inheritance**: Testnets inherit base configuration from mainnets using the `from` field
- **Selective Overrides**: Child networks can override specific fields like chain ID, RPC URLs, and confirmations

## Configuration Structure

In this example, networks are referenced from a separate JSON file in the main `config.json`:

```json
{
  "relayers": [...],
  "signers": [...],
  "notifications": [...],
  "networks": "./config/networks"
}
```

The actual network configurations are defined in `config/networks/evm.json`:

### Network Configuration Details

#### Network Inheritance Pattern

In this configuration:

1. **Mainnet as Base Network**: Ethereum mainnet serves as the base configuration with all standard settings
2. **Testnet Inheritance**: Both Sepolia and Holesky testnets inherit from mainnet using `"from": "mainnet"`
3. **Selective Overrides**: Testnets only specify the fields that differ from mainnet:
   - `chain_id`: Different chain identifiers for each network
   - `required_confirmations`: Reduced confirmations for testnets (6 vs 12)
   - `rpc_urls`: Network-specific RPC endpoints
   - `explorer_urls`: Network-specific explorer URLs
   - `is_testnet`: Flag to identify testnet networks

## How Network Inheritance Works

Network inheritance in this example allows you to:

1. **Define a base network** (mainnet) with comprehensive configuration
2. **Create child networks** (testnets) that inherit from the base using the `from` field
3. **Override specific fields** in child networks as needed for their requirements
4. **Maintain consistency** across related networks while allowing customization

### Inheritance Rules

- Child networks inherit **all fields** from the parent network specified in `from`
- Fields explicitly defined in child networks **override** inherited values
- The `from` field must reference another network within the same JSON file
- Inheritance works within the same network type (e.g., all EVM networks)

#### EVM Network Examples

- **Mainnet**: Full configuration with all properties defined
- **Sepolia**: Testnet inheriting from mainnet with specific overrides for testnet requirements
- **Holesky**: Another testnet with its own chain ID and endpoints but sharing mainnet's base configuration

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
  --output-dir examples/network-configuration-json-file/config/keys \
  --filename local-signer.json
```

**Note**: Replace `<DEFINE_YOUR_PASSWORD>` with a strong password for the keystore.

### Step 3: Environment Configuration

Create the environment file:

```bash
cp examples/network-configuration-json-file/.env.example examples/network-configuration-json-file/.env
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
docker compose -f examples/network-configuration-json-file/docker-compose.yaml up
```

The service will be available at `http://localhost:8080/api/v1`

## Testing the Configuration

### Check Available Relayers

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY"
```

## Advantages of JSON File Configuration with Inheritance

1. **Reduced Duplication**: Testnets inherit common settings from mainnets, eliminating repeated configuration
2. **Organized Structure**: Networks are separated into dedicated JSON files for better organization
3. **Easy Maintenance**: Changes to mainnet configuration automatically apply to inheriting testnets
4. **Clear Relationships**: The `from` field clearly shows parent-child relationships between networks
5. **Selective Overrides**: Only network-specific differences need to be specified in child networks
6. **Modularity**: Network configurations can be split across multiple files by type or purpose

## When to Use This Approach

- **Multi-Network Deployments**: When managing both mainnets and testnets that share common configuration
- **EVM Ecosystem**: Perfect for Ethereum mainnet with its multiple testnets (Sepolia, Holesky)
- **Configuration Consistency**: When you want to ensure testnets stay aligned with mainnet settings
- **Scalable Setups**: When you plan to add more networks that share base configurations
- **Production Environments**: When you need reliable inheritance patterns for network management

## See Also

- [Network Configuration Config File Example](../network-configuration-config-file/README.md) - Shows how to use network configuration via config file with inheritance.
