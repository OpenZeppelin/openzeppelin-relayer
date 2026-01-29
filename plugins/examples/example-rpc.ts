/**
 * Example plugin using rpc method
 */

import { JsonRpcResponseNetworkRpcResult, PluginAPI, PluginContext } from "@openzeppelin/relayer-sdk";
/**
 * Plugin handler function - this is the entry point
 * Export it as 'handler' and the relayer will automatically call it
 */
export async function handler(context: PluginContext): Promise<JsonRpcResponseNetworkRpcResult> {
  const { api } = context;

    /**
     * Instance the relayer with the given id.
     */
    const relayer = api.useRelayer("sepolia-example");

    /**
     * Makes an RPC call through the relayer to query the current block number.
     */
    const result = await relayer.rpc({
      method: 'eth_blockNumber',
      id: 1,
      jsonrpc: '2.0',
      params: ['latest'],
    });

    return result;
}
