/**
 * Example plugin using rpc method
 */

import { JsonRpcResponseNetworkRpcResult, PluginAPI } from "@openzeppelin/relayer-sdk";

type Params = {};

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));


/**
 * Plugin handler function - this is the entry point
 * Export it as 'handler' and the relayer will automatically call it
 */
export async function handler(api: PluginAPI, params: Params): Promise<JsonRpcResponseNetworkRpcResult> {
    /**
     * Instance the relayer with the given id.
     */
    const relayer = api.useRelayer("sepolia-example");

    /**
+     * Makes an RPC call through the relayer to query the current block number.
     */
    // const result = await relayer.rpc({
    //   method: 'eth_blockNumber',
    //   id: 1,
    //   jsonrpc: '2.0',
    //   params: [],
    // });

    // return result;
     await sleep(5000);

     return {
         jsonrpc: "2.0",
         id: 1,
         result: "0x0",
     };
}
