/**
 * Example plugin using 'handler' export pattern
 */

import { PluginAPI, Speed } from "@openzeppelin/relayer-sdk";

type Params = {
    destinationAddress: string;
    amount?: number;
};

/**
 * Plugin handler function - this is the entry point
 * Export it as 'handler' and the relayer will automatically call it
 */
export async function handler(api: PluginAPI, params: Params): Promise<string> {
    console.info("Plugin started with new handler pattern...");
    
    /**
     * Instance the relayer with the given id.
     */
    const relayer = api.useRelayer("sepolia-example");

    /**
     * Sends an arbitrary transaction through the relayer.
     */
    const result = await relayer.sendTransaction({
        to: params.destinationAddress,
        value: params.amount || 1,
        data: "0x",
        gas_limit: 21000,
        speed: Speed.FAST,
    });

    console.info(`Transaction submitted: ${result.id}`);

    /*
    * Waits for the transaction to be mined on chain.
    */
    await result.wait();

    console.info("Transaction confirmed!");
    return `Transaction ${result.id} completed successfully!`;
}
