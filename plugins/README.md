## Relayer Plugins

Relayer plugins are TypeScript functions that can be invoked through the relayer HTTP API.

Under the hood, the relayer will execute the plugin code in a separate process using `ts-node` and communicate with it through a Unix socket.

## Setup

### 1. Writing your plugin

```typescript
import { Speed, PluginContext, pluginError } from '@openzeppelin/relayer-sdk';

type Params = {
  destinationAddress: string;
};

type Result = {
  transactionId: string;
};

export async function handler(context: PluginContext): Promise<Result> {
  const { api, params, kv } = context;
  console.info('Plugin started...');

  const relayer = api.useRelayer('sepolia-example');
  const result = await relayer.sendTransaction({
    to: params.destinationAddress,
    value: 1,
    data: '0x',
    gas_limit: 21000,
    speed: Speed.FAST,
  });

  // Optional: persist last transaction id
  await kv.set('last_tx_id', result.id);

  await result.wait();
  return { transactionId: result.id };
}
```

#### Legacy patterns (deprecated)

The following patterns are supported for backward compatibility but will be removed in a future version. They do not provide access to the KV store.

```typescript
// Legacy: runPlugin pattern (deprecated)
import { PluginAPI, runPlugin } from '../lib/plugin';

async function legacyMain(api: PluginAPI, params: any) {
  // logic here (no KV access)
  return 'done!';
}

runPlugin(legacyMain);
```

```typescript
// Legacy: two-parameter handler (deprecated, no KV)
import { PluginAPI } from '@openzeppelin/relayer-sdk';

export async function handler(api: PluginAPI, params: any): Promise<any> {
  // logic here (no KV access)
  return 'done!';
}
```

### 2. Adding extra dependencies

You can install any extra JS/TS dependencies in your plugins folder and access them upon execution.

```bash
pnpm add ethers
```

And then just import them in your plugin.

```typescript
import { ethers } from 'ethers';
```

### 3. Adding to config file

- id: The id of the plugin. This is used to call a specific plugin through the HTTP API.
- path: The path to the plugin file - relative to the `/plugins` folder.
- timeout (optional): The timeout for the script execution _in seconds_. If not provided, the default timeout of 300 seconds (5 minutes) will be used.

```yaml
{ 'plugins': [{ 'id': 'example', 'path': 'examples/example.ts', 'timeout': 30 }] }
```

## Usage

You can call your plugin through the HTTP API, passing your custom arguments as a JSON body.

```bash
curl -X POST "http://localhost:8080/api/v1/plugins/example/call" \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "params": {
      "destinationAddress": "0xab5801a7d398351b8be11c439e05c5b3259aec9b"
    }
  }'
```

Responses use the API envelope `{ success, data, error, metadata }`.

> **Visibility controls**
>
> Runtime logs and traces are only returned when the plugin entry in the relayer config enables `emit_logs`
> and/or `emit_traces`. The Rust service trims these fields before responding so callers never see data
> that a plugin has opted out of exposing.

> **Handler errors**
>
> Throwing `pluginError(...)` (or any `Error`) is normalized into a stable HTTP payload. The relayer derives a
> client-facing message, preserves `code`/`details`, and attaches metadata subject to the same visibility
> rules above.

- Success (HTTP 200):
  - `data`: your plugin return value
  - `metadata.logs?` and `metadata.traces?`: included if enabled for the plugin
  - `error: null`
- Plugin error (HTTP 4xx):
  - `error`: human-readable message
  - `data`: `{ code?: string, details?: any }`
  - `metadata.logs?` and `metadata.traces?`: included when available

Example success:

```json
{
  "success": true,
  "data": { "result": "done!" },
  "metadata": {
    "logs": [
      {
        "level": "info",
        "message": "Plugin started..."
      }
    ],
    "traces": [
      {
        "method": "sendTransaction",
        "payload": {
          "data": "0x",
          "gas_limit": 21000,
          "speed": "fast",
          "to": "0xab5801a7d398351b8be11c439e05c5b3259aec9b",
          "value": 1
        },
        "relayerId": "sepolia-example",
        "requestId": "6c1f336f-3030-4f90-bd99-ada190a1235b"
      }
    ]
  },
  "error": null
}
```

Example error (HTTP 422):

```json
{
  "success": false,
  "data": { "code": "VALIDATION_FAILED", "details": { "field": "email" } },
  "metadata": {
    "logs": [
      {
        "level": "error",
        "message": "Validation failed for field: email"
      }
    ]
  },
  "error": "Validation failed"
}
```

## Key-Value Store (KV)

Plugins have access to a built-in KV store via the `PluginContext.kv` property for persistent state and safe concurrency.

- Uses the same Redis URL as the Relayer (`REDIS_URL`)
- Keys are namespaced per plugin ID
- JSON values are supported

```typescript
import { PluginContext } from '@openzeppelin/relayer-sdk';

export async function handler(context: PluginContext) {
  const { kv } = context;

  // Set with optional TTL
  await kv.set('greeting', { text: 'hello' }, { ttlSec: 3600 });

  // Get
  const v = await kv.get<{ text: string }>('greeting');

  // Atomic update with lock
  const count = await kv.withLock(
    'counter',
    async () => {
      const cur = (await kv.get<number>('counter')) ?? 0;
      const next = cur + 1;
      await kv.set('counter', next);
      return next;
    },
    { ttlSec: 10 }
  );

  return { v, count };
}
```

Available methods:

- `get<T>(key: string): Promise<T | null>`
- `set(key: string, value: unknown, opts?: { ttlSec?: number }): Promise<boolean>`
- `del(key: string): Promise<boolean>`
- `exists(key: string): Promise<boolean>`
- `listKeys(pattern?: string, batch?: number): Promise<string[]>`
- `clear(): Promise<number>`
- `withLock<T>(key: string, fn: () => Promise<T>, opts?: { ttlSec?: number; onBusy?: 'throw' | 'skip' }): Promise<T | null>`
