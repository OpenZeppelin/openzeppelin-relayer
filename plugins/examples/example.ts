import { PluginAPI, runPlugin } from "../lib/plugin";

import { Speed } from "@openzeppelin/relayer-sdk";

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
