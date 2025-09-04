# OpenZeppelin Relayer - Gas Price Caching Example

This example demonstrates how to configure **gas price caching** for EVM networks using OpenZeppelin Relayer. Gas price caching improves performance by reducing RPC calls and implementing a **stale-while-revalidate (SWR)** strategy that serves cached data immediately while refreshing in the background.

This simple example uses **Sepolia testnet** to showcase gas price caching functionality.

## Key Features Demonstrated

- **Gas Price Caching Configuration**: Configure caching behavior with custom timings
- **Performance Optimization**: Reduce RPC calls and improve response times
- **Direct Network Configuration**: Network defined directly in `config.json` for simplicity

## How Gas Price Caching Works

The gas price cache uses a **stale-while-revalidate** strategy:

1. **Fresh Data**: When data is fresh (within `stale_after_ms`), it's served directly from cache
2. **Stale Data**: When data is stale but not expired, it's served immediately while a background refresh is triggered
3. **Expired Data**: When data is expired (after `expire_after_ms`), the cache returns no data and the service makes fresh RPC calls

### Cache Configuration Parameters

```json
{
  "gas_price_cache": {
    "enabled": true,
    "stale_after_ms": 20000,   // 20 seconds - when to trigger background refresh
    "expire_after_ms": 45000   // 45 seconds - when to force synchronous refresh
  }
}
```

- **`enabled`**: Enable/disable caching for the network
- **`stale_after_ms`**: Milliseconds after which data is considered stale (triggers background refresh)
- **`expire_after_ms`**: Milliseconds after which data expires (cache returns no data, forces direct RPC calls)

## Configuration Structure

In this example, networks are defined directly in the main `config.json` file with different cache configurations:

### Network Configuration with Gas Price Caching

```json
{
  "relayers": [
    {
      "id": "sepolia-example",
      "name": "Sepolia Gas Cache Example",
      "network": "sepolia",
      "network_type": "evm"
    }
  ],
  "signers": [...],
  "notifications": [...],
  "networks": [
    {
      "network": "sepolia",
      "chain_id": 11155111,
      "type": "evm",
      "is_testnet": true,
      "rpc_urls": [
        "https://sepolia.drpc.org",
        "https://1rpc.io/sepolia"
      ],
      "gas_price_cache": {
        "enabled": true,
        "stale_after_ms": 20000,
        "expire_after_ms": 45000
      }
    }
  ]
}
```

### Cache Configuration for Sepolia

The example uses balanced cache timings suitable for testnet development:

```json
{
  "gas_price_cache": {
    "enabled": true,
    "stale_after_ms": 20000,   // 20 seconds - fresh enough for testing
    "expire_after_ms": 45000   // 45 seconds - reasonable expiry
  }
}
```

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
  --output-dir examples/gas-price-caching/config/keys \
  --filename local-signer.json
```

**Note**: Replace `<DEFINE_YOUR_PASSWORD>` with a strong password for the keystore.

### Step 3: Environment Configuration

Create the environment file:

```bash
cp examples/gas-price-caching/.env.example examples/gas-price-caching/.env
```

Update the `.env` file with your configuration:

- `REDIS_URL`: Redis server URL
- `KEYSTORE_PASSPHRASE`: The password you used for the keystore
- `WEBHOOK_SIGNING_KEY`: Generate using `cargo run --example generate_uuid`
- `API_KEY`: Generate using `cargo run --example generate_uuid`

### Step 4: Configure Webhook URL

Update the `url` field in the notifications section of `config/config.json`. For testing, you can use [Webhook.site](https://webhook.site) to get a test URL.

### Step 5: Set Environment Variables

Before running any commands to interact with the API, export your API key as an environment variable:

```bash
export API_KEY="your-api-key-here"
```

### Step 6: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/gas-price-caching/docker-compose.yaml up
```

The service will be available at `http://localhost:8080/api/v1`

## Testing Gas Price Caching

### Check Available Relayers

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $API_KEY"
```

### Test Transaction with Cached Gas Prices

```bash
curl -X POST http://localhost:8080/api/v1/relayers/sepolia-example/transactions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $API_KEY" \
  -d '{
    "value": 1000000000000000,
    "data": "0x",
    "to": "0x742d35Cc6640C21a1c7656d2c9C8F6bF5e7c3F8A",
    "gas_limit": 21000,
    "speed": "average"
  }'
```

### Monitor Cache Performance

Check the logs to see cache behavior:

```bash
docker compose -f examples/gas-price-caching/docker-compose.yaml logs -f relayer
```

Look for log messages like:
- `"Updated gas price snapshot for chain_id 11155111 in background"` - Background refresh

## Cache Configuration Best Practices

### RPC Provider Limits

- **Rate-Limited Providers**: Longer cache times to reduce calls
- **Premium Providers**: Shorter cache times for fresher data
- **Public RPCs**: Balanced approach to avoid hitting limits


### Network Congestion Patterns

- **High Congestion Networks**: Shorter cache for rapid price changes
- **Stable Networks**: Longer cache for consistent pricing
- **Predictable Patterns**: Adjust cache based on known traffic patterns

## Performance Benefits

### Reduced RPC Calls

- **Before**: Every gas price request hits RPC
- **After**: Most requests served from cache

### Better User Experience

- **Reduced Failures**: Less dependency on RPC availability
- **Smoother Operations**: No delays during high traffic

## Troubleshooting

### Cache Not Working

1. Check that `gas_price_cache.enabled` is `true` in network config
2. Verify network configuration is loaded correctly
3. Check logs for cache-related errors

### High RPC Usage Despite Caching

1. Verify cache timings are appropriate for your use case
2. Check if cache timings are too short (causing frequent refreshes)
3. Monitor cache hit rates in logs

### Stale Gas Prices

1. Reduce `stale_after_ms` for more frequent background refreshes
2. Check if RPC provider is returning outdated data
3. Verify background refresh is working (check logs)

## When to Use Gas Price Caching

### ✅ **Ideal Use Cases**

- **High-Volume Applications**: Many transactions per minute
- **User-Facing Applications**: Need fast response times
- **Rate-Limited RPCs**: Need to reduce API calls

### ⚠️ **Use with Caution**

- **MEV Applications**: May need very fresh gas prices
- **Arbitrage Trading**: Timing-critical applications
- **Emergency Transactions**: When gas price accuracy is critical

### ❌ **Not Recommended**

- **Single-Use Scripts**: Overhead not worth it
- **Very Low Transactions**: Cache rarely used
- **Real-Time Trading**: Need absolute latest prices

## See Also

- [Network Configuration JSON File Example](../network-configuration-json-file/README.md) - Shows how to use separate JSON files with inheritance
- [Network Configuration Config File Example](../network-configuration-config-file/README.md) - Direct config file approach (similar to this example)
- [Basic Example](../basic-example/README.md) - Simple relayer setup without caching
