import { Plugin, runPlugin } from "./lib/plugin";

type MyPluginParam = {
  relayerId: string;
};

async function example(p: Plugin, params: MyPluginParam) {
  console.log("Plugin started with params:", params);

  /**
   * Instances the relayer with the given id.
   */
  const relayer = p.useRelayer(params.relayerId || "sepolia-example");

  /**
   * Sends an arbitrary transaction through the relayer.
   */
  const result = await relayer.sendTransaction({
    to: "0xab5801a7d398351b8be11c439e05c5b3259aec9b",
    value: "1",
    data: "0x",
    gas_limit: 21000,
    speed: "fast",
  });

  console.log("Result:", result);
}

// This is the entry point for the plugin
runPlugin(example);
