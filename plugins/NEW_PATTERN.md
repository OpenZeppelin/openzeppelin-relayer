# New Plugin Pattern with Named Export 'handler'

This document describes the new simplified plugin pattern that eliminates the need for manual `runPlugin()` calls.

## How It Works

The relayer now uses a centralized executor system:

1. **Executor Script**: `plugins/lib/executor.ts` - A single script that loads user plugins
2. **Centralized Runner**: `runUserPlugin()` function that handles plugin execution
3. **User Script Convention**: Export a function named `handler`

## User Plugin Structure

### ✅ New Pattern (Recommended)

```typescript
// plugins/examples/my-plugin.ts
import { Speed } from "@openzeppelin/relayer-sdk";
import { PluginAPI } from "../lib/plugin";

type Params = {
    destinationAddress: string;
    amount?: number;
};

// Just export a 'handler' function - that's it!
export async function handler(api: PluginAPI, params: Params): Promise<string> {
    console.info("Plugin started...");
    
    const relayer = api.useRelayer("sepolia-example");
    
    const result = await relayer.sendTransaction({
        to: params.destinationAddress,
        value: params.amount || 1,
        data: "0x",
        gas_limit: 21000,
        speed: Speed.FAST,
    });
    
    await result.wait();
    return `Transaction ${result.id} completed!`;
}

// No runPlugin() call needed! 🎉
```

### ❌ Old Pattern (Deprecated)

```typescript
// Old way - more boilerplate
import { PluginAPI, runPlugin } from "../lib/plugin";

async function example(api: PluginAPI, params: Params): Promise<string> {
    // ... plugin logic
    return "done!";
}

// Manual call required
runPlugin(example);
```

## Configuration

Plugin configuration remains the same:

```json
{
  "plugins": [
    {
      "id": "my-plugin",
      "path": "examples/my-plugin.ts",
      "timeout": 30
    }
  ]
}
```

## Benefits

1. **Cleaner Code**: No boilerplate `runPlugin()` calls
2. **Consistent Pattern**: All plugins follow the same `handler` export convention
3. **Better Error Messages**: Centralized error handling with clear messages
4. **Type Safety**: Full TypeScript support maintained
5. **Easier Testing**: Handler functions can be imported and tested directly

## Migration

Existing plugins with `runPlugin()` calls will continue to work during the transition period.

To migrate:

1. Remove the `runPlugin(yourFunction)` call
2. Rename your function to `handler` (or create a new `handler` export)
3. Export the `handler` function

## Testing Your Plugin

You can test handler functions directly:

```typescript
import { handler } from './my-plugin';
import { PluginAPI } from '../lib/plugin';

// Mock API for testing
const mockApi = new PluginAPI('/mock/socket');
const params = { destinationAddress: '0x123...' };

const result = await handler(mockApi, params);
console.log(result);
```

## Implementation Details

- **Executor Execution**: `ts-node plugins/lib/executor.ts <socket> <params> <user_script>`
- **Script Loading**: Uses `require()` to dynamically load user scripts
- **Path Resolution**: Automatically normalizes paths (executor is in `plugins/lib/`, scripts in `plugins/`)
- **Convention**: Must export `handler` function
- **Error Handling**: Clear error messages for missing or invalid handlers
- **Entry Point**: `runUserPlugin()` function handles the complete plugin lifecycle

### Path Resolution Examples

The executor automatically handles path normalization:

| Config Path | Rust Resolves To | Executor Uses |
|-------------|------------------|---------------|
| `"examples/example.ts"` | `"plugins/examples/example.ts"` | `"../examples/example.ts"` |
| `"my-plugin.ts"` | `"plugins/my-plugin.ts"` | `"../my-plugin.ts"` |
| `"sub/dir/plugin.ts"` | `"plugins/sub/dir/plugin.ts"` | `"../sub/dir/plugin.ts"` |

This ensures plugin scripts are loaded correctly regardless of the executor's location.

## Architecture

```
Relayer (Rust) 
    ↓
ScriptExecutor 
    ↓ 
ts-node executor.ts <socket> <params> <user_script>
    ↓
runUserPlugin() loads user script
    ↓
Calls user's exported handler(api, params)
    ↓
Returns result to relayer
```

The architecture is now simplified to just:
1. **`executor.ts`** - Static entry point
2. **`runUserPlugin()`** - Main execution function  
3. **`loadAndExecutePlugin()`** - Helper to load user scripts
4. **`runPlugin()`** - Legacy function (backward compatibility)