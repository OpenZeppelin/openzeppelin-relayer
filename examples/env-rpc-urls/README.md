# OpenZeppelin Relayer - Environment Variables for RPC URLs Example

This example demonstrates how to use environment variables for RPC URLs in the OpenZeppelin Relayer configuration, which is useful for keeping sensitive API keys secure.

## Overview

Instead of hardcoding RPC URLs with API keys directly in the configuration files, you can reference environment variables. This approach:
- Keeps sensitive API keys out of version control
- Makes it easier to manage different environments (dev, staging, production)
- Allows for runtime configuration without changing config files

## Configuration Format

RPC URLs can be specified in two ways:

### 1. Plain String (Traditional)
```json
"rpc_urls": ["https://eth.drpc.org"]
```

### 2. Environment Variable Reference
```json
"rpc_urls": [{"type": "env", "value": "ETH_MAINNET_RPC_URL"}]
```

### 3. Mixed (Recommended for fallbacks)
```json
"rpc_urls": [
  {"type": "env", "value": "PRIMARY_RPC_URL"},
  "https://public-fallback.example.com"
]
```

## Getting Started

### Step 1: Clone the Repository

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer/examples/env-rpc-urls
```

### Step 2: Set Up Environment Variables

Copy the example environment file and add your actual RPC URLs:

```bash
cp .env.example .env
```

Edit `.env` and add your RPC URLs with API keys:

```env
ETH_MAINNET_RPC_URL=https://mainnet.infura.io/v3/YOUR_ACTUAL_KEY
SEPOLIA_RPC_URL_PRIMARY=https://sepolia.infura.io/v3/YOUR_ACTUAL_KEY
# ... etc
```

### Step 3: Create a Signer

```bash
cargo run --example create_key -- \
  --password <YOUR_PASSWORD> \
  --output-dir examples/env-rpc-urls/config/keys \
  --filename local-signer.json
```

Update `KEYSTORE_PASSPHRASE` in your `.env` file with the password you used.

### Step 4: Configure the Relayer

Create `config/config.json`:

```json
{
  "relayers": [
    {
      "id": "mainnet-relayer",
      "name": "Mainnet Relayer",
      "network": "mainnet-private",
      "paused": false,
      "notification_id": "webhook-notification",
      "signer_id": "local-signer",
      "network_type": "evm"
    }
  ],
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
  "notifications": [
    {
      "id": "webhook-notification",
      "type": "webhook",
      "url": "https://webhook.site/YOUR_UNIQUE_URL",
      "signing_key": {
        "type": "env",
        "value": "WEBHOOK_SIGNING_KEY"
      }
    }
  ],
  "networks": "./config/networks",
  "plugins": []
}
```

### Step 5: Run the Service

Using Docker Compose:

```bash
docker-compose up
```

Or run locally:

```bash
cargo run
```

### Step 6: Test the Relayer

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

## Network Configuration Examples

### EVM Network with Private RPC
```json
{
  "type": "evm",
  "network": "mainnet-private",
  "chain_id": 1,
  "rpc_urls": [
    {"type": "env", "value": "ETH_MAINNET_RPC_URL"}
  ],
  "explorer_urls": ["https://etherscan.io"],
  "average_blocktime_ms": 12000,
  "required_confirmations": 12,
  "symbol": "ETH",
  "is_testnet": false
}
```

### Multiple RPC URLs with Fallback
```json
{
  "type": "evm",
  "network": "sepolia",
  "chain_id": 11155111,
  "rpc_urls": [
    {"type": "env", "value": "SEPOLIA_RPC_PRIMARY"},
    {"type": "env", "value": "SEPOLIA_RPC_SECONDARY"},
    "https://sepolia.drpc.org"
  ],
  "explorer_urls": ["https://sepolia.etherscan.io"],
  "average_blocktime_ms": 12000,
  "required_confirmations": 6,
  "symbol": "ETH",
  "is_testnet": true
}
```

### Environment Variables in Explorer URLs
```json
{
  "explorer_urls": [
    {"type": "env", "value": "ETHERSCAN_API_ENDPOINT"},
    "https://etherscan.io"
  ]
}
```

## Best Practices

1. **Never commit `.env` files** - Always add `.env` to `.gitignore`
2. **Use descriptive variable names** - e.g., `ETH_MAINNET_INFURA_URL` instead of `RPC_URL_1`
3. **Provide public fallbacks** - Mix environment variables with public RPC URLs for resilience
4. **Document required variables** - Maintain an up-to-date `.env.example` file
5. **Validate on startup** - The relayer will fail fast if environment variables are missing

## Troubleshooting

### Missing Environment Variable
If you see an error like:
```
Failed to resolve RPC URL: Environment variable 'ETH_MAINNET_RPC_URL' not found
```
Make sure the variable is set in your `.env` file or environment.

### Invalid URL Format
If you see:
```
Invalid RPC URL: <your-url>
```
Verify that the URL is properly formatted with protocol (https://) and valid structure.

### Testing Environment Variables
You can test if your environment variables are set correctly:

```bash
# Check if variable is set
echo $ETH_MAINNET_RPC_URL

# Set variable for current session
export ETH_MAINNET_RPC_URL="https://mainnet.infura.io/v3/YOUR_KEY"
```

## Security Considerations

- Store production `.env` files securely (e.g., using secret management tools)
- Use different API keys for different environments
- Rotate API keys regularly
- Monitor API key usage for suspicious activity
- Consider using IP allowlists where supported by RPC providers

## Additional Resources

- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer/)
- [Configuration Reference](https://docs.openzeppelin.com/relayer#configuration_references)
