/**
 * Example plugin using rpc method
 */

import { PluginAPI } from "@openzeppelin/relayer-sdk";

type Params = {};

/**
 * Plugin handler function - this is the entry point
 * Export it as 'handler' and the relayer will automatically call it
 */
export async function handler(api: PluginAPI, params: Params): Promise<string> {
    /**
     * Instance the relayer with the given id.
     */
    const relayer = api.useRelayer("sepolia-example");

    /**
     * Sends an arbitrary transaction through the relayer.
     */
    const result = await relayer.rpc({
      method: 'eth_blockNumber',
      id: 1,
      jsonrpc: '2.0',
      params: [''],
    });

    return result;
}
