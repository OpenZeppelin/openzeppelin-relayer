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
import { DEFAULT_SOCKET_REQUEST_TIMEOUT_MS } from './constants';

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
          this.rejectAllPending(error);
          reject(error);
        });

        this.socket!.on('close', () => {
          this.connected = false;
          this.rejectAllPending(new Error('Socket closed unexpectedly'));
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

  /**
   * Reject all pending requests with the given error.
   * Called on socket error or close to prevent hanging promises.
   */
  private rejectAllPending(error: Error): void {
    for (const [requestId, resolver] of this.pending.entries()) {
      resolver.reject(error);
      this.pending.delete(requestId);
    }
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
      let timeoutId: NodeJS.Timeout | undefined;

      // Set up timeout to prevent hanging forever
      timeoutId = setTimeout(() => {
        this.pending.delete(requestId);
        reject(new Error(`Socket request '${method}' timed out after ${DEFAULT_SOCKET_REQUEST_TIMEOUT_MS}ms`));
      }, DEFAULT_SOCKET_REQUEST_TIMEOUT_MS);

      // Wrap resolvers to clear timeout on completion
      this.pending.set(requestId, {
        resolve: (value) => {
          if (timeoutId) clearTimeout(timeoutId);
          resolve(value);
        },
        reject: (reason) => {
          if (timeoutId) clearTimeout(timeoutId);
          reject(reason);
        },
      });

      this.socket!.write(message, (error) => {
        if (error) {
          if (timeoutId) clearTimeout(timeoutId);
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
 * Safely stringify a value, handling circular references and BigInt.
 * Falls back to String() if JSON.stringify fails.
 */
function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value, (_, v) => {
      if (typeof v === 'bigint') {
        return v.toString() + 'n';
      }
      return v;
    });
  } catch {
    try {
      return String(value);
    } catch {
      return '[Unstringifiable value]';
    }
  }
}

/**
 * Creates a console-like object that captures logs.
 */
function createSandboxConsole(logs: LogEntry[]): Console {
  const log = (level: LogEntry['level']) => (...args: any[]) => {
    const message = args.map(arg =>
      typeof arg === 'object' ? safeStringify(arg) : String(arg)
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

    // Create a require function for the sandbox
    // Block dangerous Node.js built-ins, allow npm packages
    const BLOCKED_MODULES = new Set([
      // Filesystem access
      'fs', 'fs/promises', 'node:fs', 'node:fs/promises',
      // Process/system control
      'child_process', 'node:child_process',
      'cluster', 'node:cluster',
      'worker_threads', 'node:worker_threads',
      'process', 'node:process',
      // Low-level system
      'os', 'node:os',
      'v8', 'node:v8',
      'vm', 'node:vm',
      // Direct network (use PluginAPI instead)
      'net', 'node:net',
      'dgram', 'node:dgram',
      'tls', 'node:tls',
      'http', 'node:http',
      'https', 'node:https',
      'http2', 'node:http2',
      // Dangerous utilities
      'repl', 'node:repl',
      'inspector', 'node:inspector',
      'perf_hooks', 'node:perf_hooks',
      'async_hooks', 'node:async_hooks',
      'trace_events', 'node:trace_events',
      // Native modules (potential escape)
      'module', 'node:module',
    ]);

    const sandboxRequire = (id: string): any => {
      // Block dangerous built-in modules
      if (BLOCKED_MODULES.has(id)) {
        throw new Error(
          `Module '${id}' is blocked for security. ` +
          `Use the PluginAPI for network operations.`
        );
      }

      // Allow everything else (SDK, npm packages like uuid, ethers, etc.)
      try {
        return require(id);
      } catch (err) {
        throw new Error(
          `Module '${id}' not found. Ensure it's installed in plugins/node_modules.`
        );
      }
    };

    // Create module-like objects for CommonJS compatibility
    const moduleExports: any = {};
    const moduleObject = { exports: moduleExports };

    // Minimal crypto exposure - only safe ID generation
    const safeCrypto = {
      randomUUID: () => require('crypto').randomUUID(),
      getRandomValues: (arr: Uint8Array) => require('crypto').getRandomValues(arr),
    };

    // Create the sandbox context
    // SECURITY: Only expose safe globals. Never expose:
    // - process (env vars, exit, argv)
    // - fs, child_process, net, http (I/O)
    // - eval, Function constructor (code execution)
    // - require for arbitrary modules
    const context = vm.createContext({
      // === Console for logging ===
      console: sandboxConsole,

      // === CommonJS module system ===
      exports: moduleExports,
      require: sandboxRequire,
      module: moduleObject,
      __filename: `plugin-${task.pluginId}.js`,
      __dirname: '/plugins',

      // === Async primitives ===
      setTimeout,
      setInterval,
      setImmediate,
      clearTimeout,
      clearInterval,
      clearImmediate,
      Promise,
      queueMicrotask,

      // === Core JS types (often needed explicitly) ===
      Object,
      Array,
      String,
      Number,
      Boolean,
      Symbol,
      BigInt,
      Map,
      Set,
      WeakMap,
      WeakSet,
      Date,
      RegExp,
      Math,
      JSON,

      // === Error types ===
      Error,
      TypeError,
      RangeError,
      SyntaxError,
      ReferenceError,
      URIError,
      EvalError,

      // === Number utilities ===
      parseInt,
      parseFloat,
      isNaN,
      isFinite,
      Infinity,
      NaN,

      // === String/URL encoding ===
      encodeURI,
      decodeURI,
      encodeURIComponent,
      decodeURIComponent,
      atob,
      btoa,
      URL,
      URLSearchParams,

      // === Binary data ===
      Buffer,
      ArrayBuffer,
      SharedArrayBuffer,
      Uint8Array,
      Uint16Array,
      Uint32Array,
      Int8Array,
      Int16Array,
      Int32Array,
      Float32Array,
      Float64Array,
      BigInt64Array,
      BigUint64Array,
      DataView,
      TextEncoder,
      TextDecoder,

      // === Cancellation (modern async patterns) ===
      AbortController,
      AbortSignal,

      // === Safe crypto subset (no signing/hashing secrets) ===
      crypto: safeCrypto,

      // === Reflection (needed by some libraries) ===
      Reflect,
      Proxy,
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

    // Execute the handler with timeout protection
    // This prevents hung async handlers from blocking workers indefinitely
    let result: any;

    const handlerPromise = (async () => {
      if (handler.length === 1) {
        // Modern context-based handler
        const pluginContext: PluginContext = {
          api,
          params: task.params,
          kv,
          headers: task.headers ?? {},
        };
        return await handler(pluginContext);
      } else {
        // Legacy 2-param handler (no KV/headers access)
        return await handler(api, task.params);
      }
    })();

    // Race handler against timeout to prevent worker starvation
    const timeoutPromise = new Promise<never>((_, reject) => {
      setTimeout(() => {
        const error = new Error(`Plugin handler timed out after ${task.timeout}ms`);
        (error as any).code = 'ERR_HANDLER_TIMEOUT';
        reject(error);
      }, task.timeout);
    });

    result = await Promise.race([handlerPromise, timeoutPromise]);

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
        errorMessage = `Plugin module compilation timed out after ${task.timeout}ms`;
      } else if (err.code === 'ERR_HANDLER_TIMEOUT') {
        errorCode = 'TIMEOUT';
        errorStatus = 504;
        // Message already set by the timeout error
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
    // Disconnect Redis to prevent connection leak
    await kv.disconnect().catch(() => {
      // Ignore disconnect errors - connection may not have been established
    });
  }
}
