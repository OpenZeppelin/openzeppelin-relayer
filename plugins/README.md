## Relayer Plugins

Relayer plugins are TypeScript functions that can be invoked through the relayer HTTP API.

Under the hood, the relayer will execute the plugin code in a separate process using `ts-node` and communicate with it through a Unix socket.

## Setup

#### 1. Writing your plugin

```typescript
import { Plugin, runPlugin } from "./lib/plugin";

type Args = {
  foo: string;
  bar: number;
}

async function myPlugin(plugin: Plugin, args: Args) {
    console.log(args.foo);
    console.log(args.bar);

    const relayer = plugin.useRelayer("my-relayer");
    await relayer.sendTransaction({
        to: "0x1234567890123456789012345678901234567890",
        value: 1000000000000000000,
    });

    return "done!";
}

runPlugin(myPlugin);
```


#### 2. Adding extra dependencies

You can install any extra JS/TS dependencies in your plugins folder and access them upon execution.

```bash
npm install ethers
```

And then just import them in your plugin.

```typescript
import { ethers } from "ethers";
```

#### 3. Adding to config file

- id: The id of the plugin. This is used to call a specific plugin through the HTTP API.
- path: The path to the plugin file - relative to the `/plugins` folder.

```yaml
{
  "plugins": [
    {
      "id": "my-plugin",
      "path": "my-plugin.ts"
    }
  ]
}
```

## Usage

You can call your plugin through the HTTP API, passing your custom arguments as a JSON body.

```bash
curl -X POST http://localhost:8080/plugins/my-plugin/call -d '{ "params": { "foo": "bar", "bar": 123 } }'
```

Then the response will include:

- `logs`: The logs from the plugin execution.
- `return_value`: The returned value of the plugin execution.
- `error`: An error message if the plugin execution failed.
- `traces`: A list of payloads that were sent between the plugin and the relayer. e.g. the `sendTransaction` payloads.

Example response:

```json
{
  "logs": [
    {
      "level": "log",
      "message": "bar"
    },
    {
      "level": "log",
      "message": "123"
    }
  ],
  "return_value": "done!",
  "error": null,
  "traces": [
    {
      "relayer_id": "my-relayer",
      "method": "sendTransaction",
      "params": {
        "to": "0x1234567890123456789012345678901234567890",
        "value": "0x1234567890123456789012345678901234567890"
      }
    }
  ]
}
```
