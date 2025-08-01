# Basic Example with Plugin

This example demonstrates how to use OpenZeppelin Relayer with a **custom plugin** using the new simplified plugin pattern.

## 🎯 What This Example Shows

- **New Plugin Pattern**: Using the simplified `handler` export (no `runPlugin()` boilerplate)
- **Plugin Configuration**: How to configure plugins in `config.json`
- **Docker Integration**: Running plugins in a containerized environment
- **Transaction Handling**: Sending transactions through plugins with confirmation waiting
- **Error Handling**: Proper error handling and logging in plugins

## 📁 Structure

```
basic-example-plugin/
├── docker-compose.yaml       # Docker setup with plugin support
├── config/
│   ├── config.json          # Relayer config with plugin configuration
│   ├── keys/                # Signer keys (you need to create these)
│   └── networks/            # Network configurations (shared)
├── plugins/                 # Self-contained plugins directory
│   ├── package.json         # Plugin dependencies
│   ├── tsconfig.json        # TypeScript configuration
│   ├── lib/                 # Plugin library (wrapper, API, logger)
│   │   ├── wrapper.ts       # Plugin wrapper script
│   │   ├── plugin.ts        # Plugin API and execution logic
│   │   └── logger.ts        # Logging utilities
│   └── examples/
│       └── example-handler.ts  # The actual plugin implementation
└── README.md               # This file
```

## 🚀 Plugin Features

The `plugins/examples/example-handler.ts` plugin demonstrates:

- ✅ **Simple Pattern**: Just export a `handler` function - no boilerplate!
- ✅ **Parameter Validation**: Validates required parameters
- ✅ **Transaction Sending**: Sends ETH transactions via relayer
- ✅ **Confirmation Waiting**: Waits for transaction to be mined
- ✅ **Rich Logging**: Detailed console logging with emojis
- ✅ **Error Handling**: Graceful error handling and reporting
- ✅ **Typed Results**: Returns structured success/error results

### Plugin Handler Function

```typescript
// plugins/examples/example-handler.ts (within this example directory)
export async function handler(api: PluginAPI, params: HandlerParams): Promise<HandlerResult> {
    // Get relayer instance
    const relayer = api.useRelayer("sepolia-example");
    
    // Send transaction
    const result = await relayer.sendTransaction({
        to: params.destinationAddress,
        value: params.amount || 1,
        data: "0x",
        gas_limit: 21000,
        speed: Speed.FAST,
    });
    
    // Wait for confirmation
    await result.wait();
    
    return { success: true, transactionId: result.id, /* ... */ };
}
```

No `runPlugin()` call needed! 🎉

## 🔧 Setup

### 1. **Create Signer Keys**

You need to create signer keys for the relayers:

```bash
# Create a local signer key (you can use the helper script)
cd ../../ 
cargo run --bin generate_key > examples/basic-example-plugin/config/keys/local-signer.json
```

### 2. **Set Environment Variables**

Create a `.env` file in this directory:

```bash
# examples/basic-example-plugin/.env
API_KEY=your-api-key-here
WEBHOOK_SIGNING_KEY=your-webhook-signing-key
KEYSTORE_PASSPHRASE=your-keystore-passphrase
```

### 3. **Install Plugin Dependencies**

Install the TypeScript dependencies for the plugins:

```bash
cd plugins
npm install
cd ..
```

### 4. **Configure Networks**

The example uses shared network configurations from `../../config/networks`. Ensure you have:
- `sepolia.json` - For the Sepolia testnet relayer
- `testnet.json` - For the Stellar testnet relayer (if using stellar)

## 🐳 Running with Docker

This example is completely self-contained! The `plugins/` directory contains everything needed to run the plugin, and it's mounted directly into the Docker container at `/app/plugins`.

### Start the Services

```bash
cd examples/basic-example-plugin
docker-compose up -d
```

### Check Logs

```bash
# View relayer logs
docker-compose logs -f relayer

# View Redis logs
docker-compose logs -f redis
```

### Test the Relayer

```bash
# Health check
curl http://localhost:8080/health

# List available plugins
curl -H "X-API-Key: your-api-key-here" http://localhost:8080/api/v1/plugins
```

## 🔌 Using the Plugin

### Call the Plugin via HTTP API

```bash
# Call the plugin
curl -X POST http://localhost:8080/api/v1/plugins/example-handler/call \
  -H "Content-Type: application/json" \
  -H "X-API-Key: your-api-key-here" \
  -d '{
    "destinationAddress": "0x742d35Cc6463C59A7b8E3f0b0E5c4ea7f6A2f51a",
    "amount": 1000000000000000,
    "message": "Hello from plugin!",
    "relayerId": "sepolia-example"
  }'
```

### Expected Response

```json
{
  "success": true,
  "return_value": "{\"success\":true,\"transactionId\":\"abc123\",\"transactionHash\":\"0x...\",\"message\":\"Successfully sent 1000000000000000 wei to 0x742d35Cc6463C59A7b8E3f0b0E5c4ea7f6A2f51a. Hello from plugin!\",\"timestamp\":\"2024-01-15T10:30:00.000Z\"}",
  "message": "Plugin called successfully",
  "logs": [
    {"level": "info", "message": "🚀 Starting example handler plugin..."},
    {"level": "info", "message": "💰 Sending 1000000000000000 wei to 0x742d35Cc6463C59A7b8E3f0b0E5c4ea7f6A2f51a"},
    {"level": "info", "message": "📤 Submitting transaction..."},
    {"level": "info", "message": "✅ Transaction submitted!"},
    {"level": "info", "message": "⏳ Waiting for transaction confirmation..."},
    {"level": "info", "message": "🎉 Transaction confirmed!"}
  ],
  "error": "",
  "traces": []
}
```

## 🎛️ Plugin Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `destinationAddress` | string | ✅ Yes | - | Ethereum address to send to |
| `amount` | number | ❌ No | `1` | Amount in wei to send |
| `message` | string | ❌ No | `"Hello from OpenZeppelin Relayer Plugin!"` | Custom message |
| `relayerId` | string | ❌ No | `"sepolia-example"` | Which relayer to use |

## 🔍 Debugging

### View Plugin Logs

The plugin logs are included in the HTTP response and also shown in the relayer logs:

```bash
docker-compose logs -f relayer | grep "🚀\|💰\|📤\|✅\|⏳\|🎉\|❌"
```

### Common Issues

1. **Plugin not found**: Check that the plugin path in `config.json` matches the actual file
2. **Handler not found**: Ensure your plugin exports a function named `handler`
3. **Transaction fails**: Check that your relayer has sufficient balance and gas
4. **Permission denied**: Ensure the relayer has access to the plugins directory

## 🔄 Cleanup

```bash
docker-compose down -v
```

## 📚 Learn More

- [Plugin Documentation](../../plugins/NEW_PATTERN.md) - Complete plugin development guide
- [Relayer Configuration](../../docs/modules/ROOT/pages/configuration.adoc) - Configuration reference
- [API Documentation](../../docs/openapi.json) - Full API reference

## 🎯 Next Steps

Try modifying the plugin in `plugins/examples/example-handler.ts` to:
- Send to multiple addresses
- Use different transaction speeds
- Interact with smart contracts
- Add more complex business logic
- Handle different token types

## ✨ Self-Contained Design

This example is completely self-contained:
- ✅ **All plugin code** is within the example directory
- ✅ **Plugin libraries** are copied locally (no external dependencies)
- ✅ **Docker mounts** only the example's plugins directory
- ✅ **Easy to modify** - just edit files in this directory
- ✅ **Easy to share** - copy the entire example directory

The new plugin pattern makes it easy to build and deploy custom relayer logic! 🚀