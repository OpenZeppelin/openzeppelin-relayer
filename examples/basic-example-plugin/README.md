# OpenZeppelin Relayer - Basic Plugin Example

This example demonstrates how to create, configure, and run custom plugins with OpenZeppelin Relayer. It showcases the modern plugin pattern using the `handler` export convention for clean, testable plugin development.

## Key Features Demonstrated

- **Modern Plugin Pattern**: Uses the new `handler` export convention (no manual `runPlugin()` calls required)
- **Plugin API Usage**: Shows how to interact with relayers through the plugin API
- **Transaction Management**: Demonstrates sending transactions and waiting for confirmation
- **Docker Integration**: Complete Docker setup for plugin development and testing
- **TypeScript Support**: Full TypeScript support with proper type definitions
- **Error Handling**: Comprehensive error handling and logging patterns

## Plugin Functionality

The example plugin (`test-plugin/index.ts`) performs the following operations:

1. **Parameter Validation**: Validates required parameters like destination address
2. **Transaction Submission**: Sends an ETH transfer transaction through the relayer
3. **Status Monitoring**: Waits for transaction confirmation with configurable timeout
4. **Result Reporting**: Returns structured results with transaction details and status

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)
- Rust (for key generation tools)
- Node.js and pnpm (for plugin development)

## Getting Started

### Step 1: Clone the Repository

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Create a Signer

Create a new signer keystore for the relayer:

```bash
cargo run --example create_key -- \
  --password <DEFINE_YOUR_PASSWORD> \
  --output-dir examples/basic-example-plugin/config/keys \
  --filename local-signer.json
```

**Note**: Replace `<DEFINE_YOUR_PASSWORD>` with a strong password for the keystore.

### Step 3: Environment Configuration

Create the environment file:

```bash
cp examples/basic-example-plugin/.env.example examples/basic-example-plugin/.env
```

Generate required security keys:

```bash
# Generate API key
cargo run --example generate_uuid

# Generate webhook signing key
cargo run --example generate_uuid
```

Update the `.env` file with your configuration:

```env
REDIS_URL=redis://redis:6379
KEYSTORE_PASSPHRASE=<DEFINE_YOUR_PASSWORD>
WEBHOOK_SIGNING_KEY=<generated_webhook_key>
API_KEY=<generated_api_key>
```

### Step 4: Configure Webhook URL

Update the `url` field in the notifications section of `config/config.json`. For testing, you can use [Webhook.site](https://webhook.site) to get a test URL:

```json
{
  "notifications": [
    {
      "url": "https://webhook.site/your-unique-id"
    }
  ]
}
```

### Step 5: Plugin Development

The example plugin is located in `test-plugin/index.ts`. You can modify it or create new plugins following the same pattern:

```typescript
import { Speed, PluginAPI } from "@openzeppelin/relayer-sdk";

type HandlerParams = {
    destinationAddress: string;
    amount?: number;
    message?: string;
    relayerId?: string;
};

export async function handler(api: PluginAPI, params: HandlerParams): Promise<any> {
    // Your plugin logic here
    const relayer = api.useRelayer(params.relayerId || "sepolia-example");
    
    const result = await relayer.sendTransaction({
        to: params.destinationAddress,
        value: params.amount || 1,
        data: "0x",
        gas_limit: 21000,
        speed: Speed.FAST,
    });
    
    await result.wait();
    return { success: true, transactionId: result.id };
}
```

### Step 6: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/basic-example-plugin/docker-compose.yaml up
```

The service will be available at `http://localhost:8080/api/v1`

## Testing the Plugin

### Step 1: Check Available Plugins

```bash
curl -X GET http://localhost:8080/api/v1/plugins \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY"
```

### Step 2: Call the Plugin

```bash
curl -X POST http://localhost:8080/api/v1/plugins/test-plugin/call \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -d '{
    "destinationAddress": "0x742d35Cc6640C21a1c7656d2c9C8F6bF5e7c3F8A",
    "amount": 1000000000000000,
    "message": "Hello from OpenZeppelin Relayer Plugin!"
  }'
```

### Step 3: Monitor Transaction

The plugin will return a transaction ID. You can monitor its status:

```bash
curl -X GET http://localhost:8080/api/v1/relayers/sepolia-example/transactions/TRANSACTION_ID \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY"
```

## Plugin Configuration

The plugin is configured in `config/config.json`:

```json
{
  "plugins": [
    {
      "id": "test-plugin",
      "path": "test-plugin/index.ts",
      "timeout": 30
    }
  ]
}
```

### Configuration Options

- **id**: Unique identifier for the plugin (used in API calls)
- **path**: Path to the plugin file relative to the plugins directory
- **timeout**: Maximum execution time in seconds (optional, defaults to 300)

## Plugin Development Guidelines

### Modern Plugin Pattern

Use the new `handler` export pattern:

```typescript
// ✅ Recommended: New pattern
export async function handler(api: PluginAPI, params: any): Promise<any> {
    // Plugin logic
}

// ❌ Deprecated: Old pattern
import { runPlugin } from "../lib/plugin";
runPlugin(myFunction);
```

### TypeScript Support

Define proper types for your parameters and return values:

```typescript
type MyParams = {
    destinationAddress: string;
    amount?: number;
};

type MyResult = {
    success: boolean;
    transactionId: string;
    message: string;
};

export async function handler(api: PluginAPI, params: MyParams): Promise<MyResult> {
    // Implementation
}
```

### Error Handling

Implement comprehensive error handling:

```typescript
export async function handler(api: PluginAPI, params: MyParams): Promise<MyResult> {
    try {
        // Plugin logic
        return { success: true, transactionId: result.id, message: "Success" };
    } catch (error) {
        console.error("Plugin execution failed:", error);
        return { 
            success: false, 
            transactionId: "", 
            message: `Failed: ${error.message}` 
        };
    }
}
```

## Troubleshooting

### Common Issues

1. **Plugin Not Found**: Verify the plugin path in `config.json` is correct
2. **Permission Errors**: Ensure Docker has access to mount the plugin directory
3. **TypeScript Errors**: Check that all dependencies are installed in the plugin directory
4. **API Authentication**: Verify your API key is correct and properly formatted

### Debug Mode

Enable debug logging by checking the Docker logs:

```bash
docker compose -f examples/basic-example-plugin/docker-compose.yaml logs -f relayer
```

### Plugin Testing

Test your plugin locally before deployment:

```bash
cd examples/basic-example-plugin/test-plugin
pnpm install
pnpm test
```

## Docker Configuration

The example includes optimized Docker volume mounting:

```yaml
volumes:
  - ./config:/app/config/                    # Configuration files
  - ./test-plugin:/app/plugins/test-plugin  # Your specific plugin
```

The plugins infrastructure (`lib/executor.ts`, `lib/plugin.ts`) is already included in the Docker image, so only your specific plugin needs to be mounted.

## See Also

- [Basic Example](../basic-example/README.md) - Simple relayer setup without plugins
- [Network Configuration Examples](../network-configuration-config-file/README.md) - Advanced network configuration
- [Signer Examples](../evm-aws-kms-signer/README.md) - Different signing methods