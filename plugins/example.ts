import { Plugin, runPlugin } from "./lib/plugin";

async function example(p: Plugin) {
    console.log("Plugin started...");
    /**
     * Instances the relayer with the given id.
     */
    const relayer = p.useRelayer("sepolia-example");

    /**
     * Sends an arbitrary transaction through the relayer.
     */
    const result = await relayer.sendTransaction({
        to: "0xab5801a7d398351b8be11c439e05c5b3259aec9b",
        value: "1",
        data: "0x",
        gas_limit: 21000,
        speed: "fast"
    });

    console.log("Result:", result);
}

// This is the entry point for the plugin
runPlugin(example)
