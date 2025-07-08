/**
 * Plugins library.
 *
 * This library is used to create plugins for the relayer. Including a set of utilities to simplify
 * the interaction with the relayer.
 *
 * Most important components:
 * - `PluginAPI`: A class that provides a set of methods exposing the relayer API.
 * - `runPlugin`: A function that runs the plugin.
 *  - Handles the parameters passed to the plugin.
 *  - Creates a socket connection to the relayer server
 *  - Intercepts the logs, errors and return values.
 *
 * Example:
 * ```ts
 * import { runPlugin, PluginAPI } from "./lib/plugin";
 *
 * async function main(plugin: PluginAPI, args: {
 *  relayerId: string;
 *  method: string;
 *  params: any;
 * }) {
 *  const result = await plugin.useRelayer(args.relayerId).sendTransaction(args.params);
 *  return result;
 * }
 *
 * runPlugin(main);
 */

import net from "node:net";
import { v4 as uuidv4 } from "uuid";
import { LogInterceptor } from "./logger";
import { NetworkTransactionRequest } from "@openzeppelin/relayer-sdk";

type SendTransactionResult = {
  id: string;
  relayer_id: string;
  status: string;
}

type Result<T> = {
  request_id: string;
  result: T;
  error: string | null;
}

type Relayer = {
  sendTransaction: (payload: NetworkTransactionRequest) => Promise<Result<SendTransactionResult>>;
}

function getPluginParams(): unknown {
  const pluginParams = process.argv[3];

  if (pluginParams) {
    try {
      return JSON.parse(pluginParams);
    } catch (e) {
      throw new Error(`Failed to parse payload: ${e}`);
    }
  } else return {};
}

export async function runPlugin(main: (plugin: PluginAPI, pluginParams: unknown) => Promise<any>) {
  const logInterceptor = new LogInterceptor();

  try {
    // checks if socket path is provided
    let socketPath = process.argv[2];
    if (!socketPath) {
      throw new Error("Socket path is required");
    }

    // creates plugin instance
    let plugin = new PluginAPI(socketPath);

    // Start intercepting logs
    logInterceptor.start();

    const pluginParams = getPluginParams();

    // runs main function
    await main(plugin, pluginParams)
      .then((result) => {
        // adds return value to the stdout
        logInterceptor.addResult(JSON.stringify(result));
        plugin.close();
      })
      .catch((error) => {
        console.error(error);
        // closes socket signaling error
        plugin.closeErrored(error);
        })
      .finally(() => {
        plugin.close();
        process.exit(0);
      });

    // Stop intercepting logs
    logInterceptor.stop();
  } catch (error) {
      console.error(error);
      process.exit(1);
  }
}

export class PluginAPI {
  socket: net.Socket;
  pending: Map<string, { resolve: (value: any) => void, reject: (reason: any) => void }>;
  private _connectionPromise: Promise<void> | null = null;
  private _connected: boolean = false;

  constructor(socketPath: string) {
    this.socket = net.createConnection(socketPath);
    this.pending = new Map();

    this._connectionPromise = new Promise((resolve, reject) => {
      this.socket.on('connect', () => {
        this._connected = true;
        resolve();
      });

      this.socket.on('error', (error) => {
        console.error("Socket ERROR:", error);
        reject(error);
      });
    });

    this.socket.on('data', data => {
      data.toString().split('\n').filter(Boolean).forEach((msg: string) => {
        const parsed = JSON.parse(msg);
        const { requestId, result, error } = parsed;
        const resolver = this.pending.get(requestId);
        if (resolver) {
          error ? resolver.reject(error) : resolver.resolve(result);
          this.pending.delete(requestId);
        }
      });
    });
  }

  useRelayer(relayerId: string): Relayer {
    return {
      sendTransaction: (payload: NetworkTransactionRequest) => this._send<SendTransactionResult>(relayerId, "sendTransaction", payload),
    };
  }

  async _send<T>(relayerId: string, method: string, payload: any): Promise<Result<T>> {
    const requestId = uuidv4();
    const message = JSON.stringify({ requestId, relayerId, method, payload }) + "\n";

    if (!this._connected) {
      await this._connectionPromise;
    }

    const result = this.socket.write(message, (error) => {
      if (error) {
        console.error("Error sending message:", error);
      }
    });

    if (!result) {
      throw new Error(`Failed to send message to relayer: ${message}`);
    }

    return new Promise((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject });
    });
  }

  close() {
    this.socket.end();
  }

  closeErrored(error: any) {
    this.socket.destroy(error);
  }
}
