/**
 * Example plugin using the new 'handler' export pattern
 * 
 * This plugin demonstrates the new simplified approach where users
 * just export a 'handler' function - no manual runPlugin() call needed!
 */

import { PluginAPI } from "../lib/plugin";
import { Speed } from "@openzeppelin/relayer-sdk";

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
        to: params.destinationAddress || "0x5e87fD270D40C47266B7E3c822f4a9d21043012D",
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

// That's it! No runPlugin() call needed - much cleaner! 🎉