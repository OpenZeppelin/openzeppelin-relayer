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

import { NetworkTransactionRequest, TransactionResponse, TransactionStatus } from "@openzeppelin/relayer-sdk";

import { LogInterceptor } from "./logger";
import net from "node:net";
import { v4 as uuidv4 } from "uuid";

/**
 * Smart serialization for plugin return values
 * - Objects/Arrays: JSON.stringify (need serialization)
 * - Primitives: String conversion (clean, no extra quotes)
 * - null/undefined: String representation
 */
export function serializeResult(result: any): string {
  if (result === null) {
    return 'null';
  }
  
  if (result === undefined) {
    return 'undefined';
  }
  
  if (typeof result === 'object' || Array.isArray(result)) {
    return JSON.stringify(result); // Objects need JSON serialization
  }
  
  return String(result); // Primitives as clean strings
}

type TransactionWaitOptions = {
  interval?: number;
  timeout?: number;
}

/**
 * The result of a sendTransaction call.
 *
 * @property id - The transaction ID.
 * @property relayer_id - The relayer ID.
 * @property status - The transaction status. Can be `submitted`, `pending`, `sent`, `mined`, `cancelled`, `confirmed`, `failed` or `expired`.
 * @property confirmed_at - The date and time the transaction was confirmed.
 * @property created_at - The date and time the transaction was created.
 * @property from - The address of the sender.
 * @property gas_limit - The gas limit of the transaction.
 * @property gas_price - The gas price of the transaction.
 * @property hash - The hash of the transaction.
 * @property nonce - The nonce of the transaction.
 * @property sent_at - The date and time the transaction was sent.
 * @property status_reason - The reason for the transaction status.
 * @property to - The address of the recipient.
 * @property value - The value of the transaction.
 * @property wait - A method to wait for the transaction to be mined on chain.
 */
type SendTransactionResult = {
  id: string;
  relayer_id: string;
  status: string;
  confirmed_at: string | null;
  created_at: string;
  from: string;
  gas_limit: number;
  gas_price: string | null;
  hash: string | null;
  nonce: number | null;
  sent_at: string | null;
  status_reason: string | null;
  to: string;
  value: string;

  /**
   * Waits for the transaction to be mined on chain.
   * @param options - Allows to specify the polling interval and the timeout.
   *  - `interval` - The polling interval in milliseconds. Defaults to `5000`.
   *  - `timeout` - The timeout in milliseconds. Defaults to `60000`.
   * @returns The transaction response.
   */
  wait: (options?: TransactionWaitOptions) => Promise<TransactionResponse>;
}

type GetTransactionRequest = {
  transactionId: string;
}

/**
 * The relayer API.
 * We are defining this interface here and in SDK. When changes are made to the interface, we need to update both places.
 *
 * @property sendTransaction - Sends a transaction to the relayer.
 * @property getTransaction - Gets a transaction from the relayer.
 */
type Relayer = {
  /**
   * Sends a transaction to the relayer.
   * @param payload - The transaction request payload.
   * @returns The transaction result.
   */
  sendTransaction: (payload: NetworkTransactionRequest) => Promise<SendTransactionResult>;

  /**
   * Fetches a transaction from the relayer.
   * @param payload - including the transaction id.
   * @returns The transaction response.
   */
  getTransaction: (payload: GetTransactionRequest) => Promise<TransactionResponse>;
}

/**
 * Public interface for plugin API - only exposes methods that plugins should use.
 * We are defining this interface here and in SDK. When changes are made to the interface, we need to update both places.
 */
export interface PluginAPI {
  /**
   * Creates a relayer API for the given relayer ID.
   * @param relayerId - The relayer ID.
   * @returns The relayer API.
   */
  useRelayer(relayerId: string): Relayer;

  /**
   * Waits for a transaction to be mined on chain.
   * @param transaction - The transaction result from sendTransaction
   * @param options - Polling interval and timeout options
   * @returns The transaction response once mined/confirmed
   */
  transactionWait(transaction: SendTransactionResult, options?: TransactionWaitOptions): Promise<TransactionResponse>;
}

type Plugin<T, R> = (plugin: PluginAPI, pluginParams: T) => Promise<R>;

// Global variable to capture legacy plugin function
let _legacyPluginFunction: Plugin<any, any> | null = null;

function getPluginParams<T>(): T {
  const pluginParams = process.argv[3];

  if (!pluginParams) {
    throw new Error("Plugin parameters are required but not provided");
  }

    try {
      const parsed = JSON.parse(pluginParams);
      return parsed as T;
    } catch (e) {
      throw new Error(`Failed to parse plugin parameters: ${e}`);
    }
}

/**
 * Legacy runPlugin function - captures the plugin function for later execution
 * This provides backward compatibility while the new handler pattern is adopted
 */
export async function runPlugin<T, R>(main: Plugin<T, R>): Promise<void> {
  // In the new architecture, we just capture the function for later execution
  // instead of running it immediately
  if (typeof main === 'function') {
    _legacyPluginFunction = main as Plugin<any, any>;
    return;
  }
  
  // If we reach here, it means this is being called in the old direct execution mode
  // (not through the executor), so we fall back to the original behavior
  const logInterceptor = new LogInterceptor();

  try {
    // checks if socket path is provided
    let socketPath = process.argv[2];
    if (!socketPath) {
      throw new Error("Socket path is required");
    }

    // creates plugin instance
    let plugin = new DefaultPluginAPI(socketPath);

    // Start intercepting logs
    logInterceptor.start();

    const pluginParams = getPluginParams<T>();

    // runs main function
    const result = await (main as (api: PluginAPI, params: T) => Promise<R>)(plugin, pluginParams);
    
    // adds return value to the stdout
    logInterceptor.addResult(serializeResult(result));
    plugin.close();

    // Stop intercepting logs
    logInterceptor.stop();
  } catch (error) {
      console.error(error);
      process.exit(1);
  }
}

/**
 * Helper function that loads and executes a user plugin script
 * @param userScriptPath - Path to the user's plugin script
 * @param api - Plugin API instance
 * @param params - Plugin parameters
 */
export async function loadAndExecutePlugin<T, R>(
  userScriptPath: string, 
  api: PluginAPI, 
  params: T
): Promise<R> {
  try {
      // IMPORTANT: Path normalization required because executor is in plugins/lib/
      // but user scripts are in plugins/ (and config paths are relative to plugins/)
      // 
      // Examples:
      // - Config: "examples/example.ts" → Rust: "plugins/examples/example.ts" → Executor: "../examples/example.ts"
      // - Config: "my-plugin.ts" → Rust: "plugins/my-plugin.ts" → Executor: "../my-plugin.ts"
      let normalizedPath = userScriptPath;
      
      // Check if it's an absolute path (starts with / on Unix-like systems or C:\ on Windows)
      const isAbsolute = userScriptPath.startsWith('/') || /^[A-Za-z]:\\/.test(userScriptPath);
      
      if (isAbsolute) {
          // For absolute paths, use as-is (e.g., temporary test files)
          normalizedPath = userScriptPath;
      } else if (userScriptPath.startsWith('plugins/')) {
          // Remove 'plugins/' prefix and add '../' to go back from lib/ to plugins/
          normalizedPath = '../' + userScriptPath.substring('plugins/'.length);
      } else {
          // If path doesn't start with 'plugins/', assume it's relative to plugins/
          normalizedPath = '../' + userScriptPath;
      }
      
      // Clear any previous legacy plugin function
      _legacyPluginFunction = null;
      
      // Load user's script module
      const userModule = require(normalizedPath);
      
      // Try modern pattern first: look for 'handler' named export
      const handler = userModule.handler;
      
      if (handler && typeof handler === 'function') {
          // Modern pattern: call the exported handler
          const result = await handler(api, params);
          return result;
      }
      
      // Try legacy pattern: check if runPlugin was called during module loading
      if (_legacyPluginFunction && typeof _legacyPluginFunction === 'function') {
          console.warn(`[DEPRECATED] Plugin at ${userScriptPath} uses the deprecated runPlugin pattern. Please migrate to the handler export pattern.`);
          // Legacy pattern: call the captured plugin function
          const result = await (_legacyPluginFunction as (api: PluginAPI, params: T) => Promise<R>)(api, params);
          return result;
      }
      
      // If neither modern nor legacy pattern is found, assume it's a direct execution script
      // This handles simple scripts that just execute immediately (like test scripts)
      // For direct execution scripts, we don't call any function - the script already executed
      // when it was required. We just return an empty result.
      return undefined as any;
      
  } catch (error) {
      throw new Error(`Failed to execute user plugin ${userScriptPath}: ${(error as Error).message}`);
  }
}



/**
 * The plugin API.
 *
 * @property useRelayer - Creates a relayer API for the given relayer ID.
 * @property sendTransaction - Sends a transaction to the relayer.
 * @property getTransaction - Gets a transaction by id.
 */
export class DefaultPluginAPI implements PluginAPI {
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

  /**
   * Creates a relayer API for the given relayer ID.
   * @param relayerId - The relayer ID.
   * @returns The relayer API.
   */
  useRelayer(relayerId: string): Relayer {
    return {
      sendTransaction: async (payload: NetworkTransactionRequest) => {
        const result = await this._send<SendTransactionResult>(relayerId, "sendTransaction", payload);
        // Add the wait method to the result
        return {
          ...result,
          wait: (options?: TransactionWaitOptions) => this.transactionWait(result, options)
        };
      },
      getTransaction: (payload: GetTransactionRequest) => this._send<TransactionResponse>(relayerId, "getTransaction", payload),
    };
  }

  async transactionWait(transaction: SendTransactionResult, options?: TransactionWaitOptions): Promise<TransactionResponse> {
    const waitOptions: TransactionWaitOptions = {
      interval: options?.interval || 5000,
      timeout: options?.timeout || 60000,
    };

    const relayer = this.useRelayer(transaction.relayer_id);
    let transactionResult: TransactionResponse = await relayer.getTransaction({ transactionId: transaction.id });

    // timeout to avoid infinite waiting
    const timeout = setTimeout(() => {
      throw new Error(`Transaction ${transaction.id} timed out after ${waitOptions.timeout}ms`);
    }, waitOptions.timeout);

    // poll for transaction status until mined/confirmed, failed, cancelled or expired.
    while (transactionResult.status !== TransactionStatus.MINED &&
      transactionResult.status !== TransactionStatus.CONFIRMED &&
      transactionResult.status !== TransactionStatus.CANCELED &&
      transactionResult.status !== TransactionStatus.EXPIRED &&
      transactionResult.status !== TransactionStatus.FAILED) {
      transactionResult = await relayer.getTransaction({ transactionId: transaction.id });
      await new Promise(resolve => setTimeout(resolve, waitOptions.interval));
    }

    clearTimeout(timeout);
    return transactionResult;
  }

  async _send<T>(relayerId: string, method: string, payload: any): Promise<T> {
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

/**
 * Main entry point for plugin execution
 * 
 * This function handles the entire plugin lifecycle: loading, execution, and cleanup.
 * It receives validated parameters from the wrapper script and focuses purely on plugin execution logic.
 * 
 * @param socketPath - Unix socket path for communication with relayer
 * @param pluginParams - Parsed plugin parameters object
 * @param userScriptPath - Path to the user's plugin file to execute
 */
export async function runUserPlugin<T = any, R = any>(
  socketPath: string,
  pluginParams: T,
  userScriptPath: string
): Promise<R> {
  try {
    // Create plugin API instance
    const plugin = new DefaultPluginAPI(socketPath);
    
    // Use helper function to load and execute the plugin
    const result: R = await loadAndExecutePlugin<T, R>(userScriptPath, plugin, pluginParams);
    
    plugin.close();
    return result;
    
  } catch (error) {
    console.error(error);
    process.exit(1);
  }
}
