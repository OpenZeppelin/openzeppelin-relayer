/**
 * Sandbox Executor Module
 *
 * Piscina worker that executes compiled JavaScript plugins in isolated node:vm contexts.
 * Each task gets a fresh vm.createContext() for complete isolation between executions.
 */

import * as vm from 'node:vm';
import * as net from 'node:net';
import { v4 as uuidv4 } from 'uuid';
import type { PluginKVStore } from './kv';
import { DefaultPluginKVStore } from './kv';
import { LogInterceptor } from './logger';
import { wrapForVm } from './compiler';
import type { PluginAPI, PluginContext, PluginHeaders, Relayer } from './plugin';
import {
  ApiResponseRelayerResponseData,
  ApiResponseRelayerStatusData,
  JsonRpcRequestNetworkRpcRequest,
  JsonRpcResponseNetworkRpcResult,
  NetworkTransactionRequest,
  SignTransactionResponse,
  SignTransactionRequest,
  TransactionResponse,
  TransactionStatus,
  pluginError,
} from '@openzeppelin/relayer-sdk';

/**
 * Task payload received from the worker pool
 */
export interface SandboxTask {
  /** Unique task ID for correlation */
  taskId: string;
  /** Plugin ID for KV namespacing */
  pluginId: string;
  /** Pre-compiled JavaScript code */
  compiledCode: string;
  /** Plugin parameters */
  params: any;
  /** HTTP headers from incoming request */
  headers?: PluginHeaders;
  /** Unix socket path for relayer communication */
  socketPath: string;
  /** HTTP request ID for tracing */
  httpRequestId?: string;
  /** Execution timeout in milliseconds */
  timeout: number;
}

/**
 * Result returned from sandbox execution
 */
export interface SandboxResult {
  /** Task ID for correlation */
  taskId: string;
  /** Whether execution succeeded */
  success: boolean;
  /** Return value (if success) */
  result?: any;
  /** Error information (if failed) */
  error?: {
    message: string;
    code?: string;
    status?: number;
    details?: any;
  };
  /** Captured console logs */
  logs: LogEntry[];
}

export interface LogEntry {
  level: 'error' | 'warn' | 'info' | 'log' | 'debug' | 'result';
  message: string;
}

/**
 * Sandboxed Plugin API that communicates with the relayer via Unix socket.
 * This is a minimal implementation for use within the vm context.
 * Connection is lazy - only established when first API call is made.
 */
class SandboxPluginAPI implements PluginAPI {
  private socket: net.Socket | null = null;
  private pending: Map<string, { resolve: (value: any) => void; reject: (reason: any) => void }>;
  private connectionPromise: Promise<void> | null = null;
  private connected: boolean = false;
  private httpRequestId?: string;
  private socketPath: string;

  constructor(socketPath: string, httpRequestId?: string) {
    this.socketPath = socketPath;
    this.pending = new Map();
    this.httpRequestId = httpRequestId;
  }

  private async ensureConnected(): Promise<void> {
    if (this.connected) return;

    if (!this.connectionPromise) {
      this.socket = net.createConnection(this.socketPath);

      this.connectionPromise = new Promise((resolve, reject) => {
        this.socket!.on('connect', () => {
          this.connected = true;
          resolve();
        });

        this.socket!.on('error', (error) => {
          reject(error);
        });
      });

      this.socket.on('data', (data) => {
        data
          .toString()
          .split('\n')
          .filter(Boolean)
          .forEach((msg: string) => {
            try {
              const parsed = JSON.parse(msg);
              const { requestId, result, error } = parsed;
              const resolver = this.pending.get(requestId);
              if (resolver) {
                error ? resolver.reject(error) : resolver.resolve(result);
                this.pending.delete(requestId);
              }
            } catch {
              // Ignore malformed messages
            }
          });
      });
    }

    await this.connectionPromise;
  }

  useRelayer(relayerId: string): Relayer {
    return {
      sendTransaction: async (payload: NetworkTransactionRequest) => {
        const result = await this.send<{ id: string; relayer_id: string }>(relayerId, 'sendTransaction', payload);
        return {
          ...result,
          wait: (options?: { interval?: number; timeout?: number }) =>
            this.transactionWait(result, options),
        } as any;
      },
      getTransaction: (payload: { transactionId: string }) =>
        this.send(relayerId, 'getTransaction', payload),
      getRelayerStatus: () =>
        this.send<ApiResponseRelayerStatusData>(relayerId, 'getRelayerStatus', {}),
      signTransaction: (payload: SignTransactionRequest) =>
        this.send<SignTransactionResponse>(relayerId, 'signTransaction', payload),
      getRelayer: () =>
        this.send<ApiResponseRelayerResponseData>(relayerId, 'getRelayer', {}),
      rpc: (payload: JsonRpcRequestNetworkRpcRequest) =>
        this.send(relayerId, 'rpc', payload),
    };
  }

  async transactionWait(
    transaction: { id: string; relayer_id: string },
    options?: { interval?: number; timeout?: number }
  ): Promise<TransactionResponse> {
    const interval = options?.interval ?? 5000;
    const timeout = options?.timeout ?? 60000;
    const relayer = this.useRelayer(transaction.relayer_id);
    let shouldContinue = true;

    const poll = async (): Promise<TransactionResponse> => {
      let tx = await relayer.getTransaction({ transactionId: transaction.id });
      while (
        shouldContinue &&
        tx.status !== TransactionStatus.MINED &&
        tx.status !== TransactionStatus.CONFIRMED &&
        tx.status !== TransactionStatus.CANCELED &&
        tx.status !== TransactionStatus.EXPIRED &&
        tx.status !== TransactionStatus.FAILED
      ) {
        await new Promise((resolve) => setTimeout(resolve, interval));
        if (!shouldContinue) break;
        tx = await relayer.getTransaction({ transactionId: transaction.id });
      }
      return tx;
    };

    let timeoutId: NodeJS.Timeout | undefined;
    const timeoutPromise = new Promise<never>((_, reject) => {
      timeoutId = setTimeout(() => {
        shouldContinue = false;
        reject(pluginError(`Transaction ${transaction.id} timed out after ${timeout}ms`, { status: 504 }));
      }, timeout);
    });

    return Promise.race([poll(), timeoutPromise]).finally(() => {
      shouldContinue = false;
      if (timeoutId) clearTimeout(timeoutId);
    });
  }

  private async send<T>(relayerId: string, method: string, payload: any): Promise<T> {
    const requestId = uuidv4();
    const msg: any = { requestId, relayerId, method, payload };
    if (this.httpRequestId) {
      msg.httpRequestId = this.httpRequestId;
    }
    const message = JSON.stringify(msg) + '\n';

    // Ensure we're connected before sending
    await this.ensureConnected();

    return new Promise((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject });
      this.socket!.write(message, (error) => {
        if (error) {
          this.pending.delete(requestId);
          reject(error);
        }
      });
    });
  }

  close(): void {
    // Only close if we actually connected
    if (this.socket) {
      // Use destroy() for immediate close instead of end() which is graceful
      // This ensures the Rust socket reader sees EOF immediately
      this.socket.destroy();
    }
  }
}

/**
 * Creates a console-like object that captures logs.
 */
function createSandboxConsole(logs: LogEntry[]): Console {
  const log = (level: LogEntry['level']) => (...args: any[]) => {
    const message = args.map(arg =>
      typeof arg === 'object' ? JSON.stringify(arg) : String(arg)
    ).join(' ');
    logs.push({ level, message });
  };

  return {
    log: log('log'),
    info: log('info'),
    warn: log('warn'),
    error: log('error'),
    debug: log('debug'),
    trace: log('debug'),
    // Required console methods (no-ops for unused ones)
    assert: () => {},
    clear: () => {},
    count: () => {},
    countReset: () => {},
    dir: () => {},
    dirxml: () => {},
    group: () => {},
    groupCollapsed: () => {},
    groupEnd: () => {},
    table: () => {},
    time: () => {},
    timeEnd: () => {},
    timeLog: () => {},
    timeStamp: () => {},
    profile: () => {},
    profileEnd: () => {},
    Console: console.Console,
  } as Console;
}

/**
 * Executes a compiled plugin in an isolated vm context.
 * This is the Piscina worker function.
 */
export default async function executeInSandbox(task: SandboxTask): Promise<SandboxResult> {
  const logs: LogEntry[] = [];
  const api = new SandboxPluginAPI(task.socketPath, task.httpRequestId);
  const kv = new DefaultPluginKVStore(task.pluginId);

  try {
    // Create isolated context with limited globals
    const sandboxConsole = createSandboxConsole(logs);

    // Prepare the module wrapper
    const wrappedCode = wrapForVm(task.compiledCode);

    // Create a mock require function for the sandbox
    // Only allow specific safe modules
    const sandboxRequire = (id: string): any => {
      // Allow the relayer SDK (it's already used in the compiled code)
      if (id === '@openzeppelin/relayer-sdk') {
        return require('@openzeppelin/relayer-sdk');
      }
      // Block all other requires for security
      throw new Error(`Module '${id}' is not available in plugin sandbox`);
    };

    // Create module-like objects for CommonJS compatibility
    const moduleExports: any = {};
    const moduleObject = { exports: moduleExports };

    // Create the sandbox context
    const context = vm.createContext({
      // Console for logging
      console: sandboxConsole,
      // CommonJS module system
      exports: moduleExports,
      require: sandboxRequire,
      module: moduleObject,
      __filename: `plugin-${task.pluginId}.js`,
      __dirname: '/plugins',
      // Timer functions
      setTimeout,
      setInterval,
      setImmediate,
      clearTimeout,
      clearInterval,
      clearImmediate,
      // Promise (needed for async)
      Promise,
      // Buffer for binary data handling
      Buffer,
      // JSON utilities
      JSON,
      // Error types
      Error,
      TypeError,
      RangeError,
      SyntaxError,
      // URL handling
      URL,
      URLSearchParams,
      // TextEncoder/Decoder
      TextEncoder,
      TextDecoder,
    });

    // Compile and run the wrapped code to get the factory function
    const script = new vm.Script(wrappedCode, {
      filename: `plugin-${task.pluginId}.js`,
    });

    const factory = script.runInContext(context, { timeout: task.timeout });

    // Execute the factory to populate module.exports
    factory(moduleExports, sandboxRequire, moduleObject, `plugin-${task.pluginId}.js`, '/plugins');

    // Get the handler from exports
    const handler = moduleObject.exports.handler || moduleExports.handler;

    if (!handler || typeof handler !== 'function') {
      throw new Error('Plugin must export a handler function');
    }

    // Execute the handler
    let result: any;

    if (handler.length === 1) {
      // Modern context-based handler
      const pluginContext: PluginContext = {
        api,
        params: task.params,
        kv,
        headers: task.headers ?? {},
      };
      result = await handler(pluginContext);
    } else {
      // Legacy 2-param handler (no KV/headers access)
      result = await handler(api, task.params);
    }

    return {
      taskId: task.taskId,
      success: true,
      result,
      logs,
    };
  } catch (error) {
    const err = error as any;
    
    // Extract detailed error information
    let errorCode = 'PLUGIN_ERROR';
    let errorMessage = String(error);
    let errorStack: string | undefined;
    let errorDetails: any = undefined;
    let errorStatus = 500;
    
    if (err && typeof err === 'object') {
      errorMessage = err.message || String(error);
      
      // Determine error code from error type
      if (err.name === 'SyntaxError') {
        errorCode = 'SYNTAX_ERROR';
      } else if (err.name === 'TypeError') {
        errorCode = 'TYPE_ERROR';
      } else if (err.name === 'ReferenceError') {
        errorCode = 'REFERENCE_ERROR';
      } else if (err.code === 'ERR_SCRIPT_EXECUTION_TIMEOUT') {
        errorCode = 'TIMEOUT';
        errorStatus = 504;
        errorMessage = `Plugin execution timed out after ${task.timeout}ms`;
      } else if (err.code) {
        errorCode = err.code;
      }
      
      // Capture stack trace (sanitize paths)
      if (err.stack) {
        errorStack = err.stack
          .split('\n')
          .slice(0, 10)  // Limit stack trace length
          .join('\n');
      }
      
      // Capture any additional details
      if (err.details) {
        errorDetails = err.details;
      }
      
      // Use status if provided
      if (typeof err.status === 'number') {
        errorStatus = err.status;
      }
    }
    
    return {
      taskId: task.taskId,
      success: false,
      error: {
        message: errorMessage,
        code: errorCode,
        status: errorStatus,
        details: errorDetails ? {
          ...errorDetails,
          stack: errorStack,
        } : (errorStack ? { stack: errorStack } : undefined),
      },
      logs,
    };
  } finally {
    api.close();
  }
}
