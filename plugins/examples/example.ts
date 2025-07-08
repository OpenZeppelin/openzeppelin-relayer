import { Speed } from "@openzeppelin/relayer-sdk";
import { PluginAPI, runPlugin } from "../lib/plugin";

async function example(api: PluginAPI) {
    console.info("Plugin started...");
    /**
     * Instances the relayer with the given id.
     */
    const relayer = api.useRelayer("sepolia-example");

    /**
     * Sends an arbitrary transaction through the relayer.
     */
    const result = await relayer.sendTransaction({
        to: "0xab5801a7d398351b8be11c439e05c5b3259aec9b",
        value: 1,
        data: "0x",
        gas_limit: 21000,
        speed: Speed.FAST,
    });

    /*
    * Waits for the transaction to be mined on chain.
    */
    const transaction = await result.wait();

    return transaction.hash;
}

/**
 * This is the entry point for the plugin
 */
runPlugin(example)
