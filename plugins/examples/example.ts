/**
 * Example of a relayer plugin that is executed with params from HTTP /call request
 */
import { Speed } from "@openzeppelin/relayer-sdk/dist/src/models/speed";
import { PluginAPI, runPlugin } from "../lib/plugin";

type MyPluginParam = {
  relayerId: string;
};

async function example(api: PluginAPI, params: MyPluginParam) {
  console.log("Plugin started...");

  console.log("Plugin started with params:", params);

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

  return result;
}

/**
 * This is the entry point for the plugin
 */
runPlugin(example);
