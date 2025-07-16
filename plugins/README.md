
## Relayer Plugins

Relayer plugins are TypeScript functions that can be invoked through the relayer HTTP API.

Under the hood, the relayer will execute the plugin code in a separate process using `ts-node` and communicate with it through a Unix socket.

## Setup

### 1. Writing your plugin

```typescript
import { Speed } from "@openzeppelin/relayer-sdk";
import { PluginAPI, runPlugin } from "../lib/plugin";

type Params = {
    destinationAddress: string;
};

async function example(api: PluginAPI, params: Params): Promise<string> {
    console.info("Plugin started...");
    /**
     * Instances the relayer with the given id.
     */
    const relayer = api.useRelayer("sepolia-example");

    /**
     * Sends an arbitrary transaction through the relayer.
     */
    const result = await relayer.sendTransaction({
        to: params.destinationAddress,
        value: 1,
        data: "0x",
        gas_limit: 21000,
        speed: Speed.FAST,
    });

    /*
    * Waits for the transaction to be mined on chain.
    */
    await result.wait();

    return "done!";
}

/**
 * This is the entry point for the plugin
 */
runPlugin(example);
```


### 2. Adding extra dependencies

You can install any extra JS/TS dependencies in your plugins folder and access them upon execution.

```bash
pnpm add ethers
```

And then just import them in your plugin.

```typescript
import { ethers } from "ethers";
```

### 3. Adding to config file

- id: The id of the plugin. This is used to call a specific plugin through the HTTP API.
- path: The path to the plugin file - relative to the `/plugins` folder.
- timeout (optional): The timeout for the script execution *in seconds*. If not provided, the default timeout of 300 seconds (5 minutes) will be used.

```yaml
{
  "plugins": [
    {
      "id": "example",
      "path": "examples/example.ts",
      "timeout": 30
    }
  ]
}
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

Then the response will include:

- `logs`: The logs from the plugin execution.
- `return_value`: The returned value of the plugin execution.
- `error`: An error message if the plugin execution failed.
- `traces`: A list of payloads that were sent between the plugin and the relayer. e.g. the `sendTransaction` payloads.

Example response:

```json
{
  "success": true,
  "data": {
    "success": true,
    "return_value": "\"done!\"",
    "message": "Plugin called successfully",
    "logs": [
      {
        "level": "info",
        "message": "Plugin started..."
      }
    ],
    "error": "",
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
