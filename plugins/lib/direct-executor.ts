/**
 * Plugin Executor Module
 *
 * Piscina worker that executes compiled JavaScript plugins.
 * Plugins run in worker threads for parallelism.
 */

import * as v8 from 'node:v8';
import * as net from 'node:net';
import { v4 as uuidv4 } from 'uuid';
import { DefaultPluginKVStore } from './kv';
import type { PluginAPI, PluginContext, PluginHeaders, Relayer } from './plugin';
import {
  ApiResponseRelayerResponseData,
  ApiResponseRelayerStatusData,
  JsonRpcRequestNetworkRpcRequest,
  NetworkTransactionRequest,
  SignTransactionResponse,
  SignTransactionRequest,
  TransactionResponse,
  TransactionStatus,
  pluginError,
} from '@openzeppelin/relayer-sdk';
import { DEFAULT_SOCKET_REQUEST_TIMEOUT_MS } from './constants';

/**
 * Function Cache - Caches compiled plugin factory functions.
 * Compiling with Function constructor takes 1-5ms. Caching eliminates this for repeated code.
 * Memory-aware: monitors heap usage and proactively evicts under pressure.
 */
class FunctionCache {
  private cache = new Map<string, { factory: Function; timestamp: number }>();
  private readonly maxSize = 100; // Max functions to cache per worker
  private lastMemoryCheck = 0;
  private readonly memoryCheckInterval = 5000; // Check every 5s

  get(code: string): Function | undefined {
    // Periodic memory check on get operations
    this.maybeCheckMemory();

    const entry = this.cache.get(code);
    if (entry) {
      // Update access timestamp for LRU
      entry.timestamp = Date.now();
      return entry.factory;
    }
    return undefined;
  }

  set(code: string, factory: Function): void {
    // Check memory before adding
    this.maybeCheckMemory();

    // Evict oldest entry if at capacity (FIFO)
    if (this.cache.size >= this.maxSize) {
      this.evictOldest(1);
    }
    this.cache.set(code, { factory, timestamp: Date.now() });
  }

  /**
   * Periodic memory check - evict cache if heap is under pressure.
   */
  private maybeCheckMemory(): void {
    const now = Date.now();
    if (now - this.lastMemoryCheck < this.memoryCheckInterval) {
      return;
    }
    this.lastMemoryCheck = now;

    const usage = process.memoryUsage();
    const heapStats = v8.getHeapStatistics();
    // Use heap_size_limit (the actual max heap) instead of heapTotal (current allocated)
    // heapTotal grows dynamically and can be much smaller than the configured max,
    // causing false positive pressure detection (e.g., 28MB/30MB = 93% when max is 26GB)
    const heapUsedRatio = usage.heapUsed / heapStats.heap_size_limit;

    // At 85% heap usage, evict 50% of cache
    if (heapUsedRatio >= 0.85 && this.cache.size > 0) {
      const evictCount = Math.max(1, Math.ceil(this.cache.size * 0.5));
      console.warn(
        `[worker-cache] HIGH memory pressure (${Math.round(heapUsedRatio * 100)}% of heap limit), ` +
        `evicting ${evictCount} functions`
      );
      this.evictOldest(evictCount);
    } else if (heapUsedRatio >= 0.70 && this.cache.size > 0) {
      // At 70% heap usage, evict 25% of cache
      const evictCount = Math.max(1, Math.ceil(this.cache.size * 0.25));
      console.warn(
        `[worker-cache] Memory pressure (${Math.round(heapUsedRatio * 100)}% of heap limit), ` +
        `evicting ${evictCount} functions`
      );
      this.evictOldest(evictCount);
    }
  }

  evictOldest(count: number): void {
    // Sort by timestamp (oldest first) and evict
    const entries = [...this.cache.entries()].sort(
      (a, b) => a[1].timestamp - b[1].timestamp
    );

    let evicted = 0;
    for (const [key] of entries) {
      if (evicted >= count) break;
      this.cache.delete(key);
      evicted++;
    }
    if (evicted > 0) {
      console.log(`[worker-cache] Evicted ${evicted} oldest functions`);
    }
  }

  clear(): void {
    // Emergency cleanup - drop all cached functions
    const size = this.cache.size;
    this.cache.clear();
    if (size > 0) {
      console.log(`[worker-cache] Emergency eviction: cleared ${size} cached functions`);
    }
  }

  size(): number {
    return this.cache.size;
  }
}

/**
 * Custom error for socket closure - enables retry logic to detect and handle
 * server-side connection termination (e.g., Rust's 60-second connection lifetime).
 */
class SocketClosedError extends Error {
  code: string;

  constructor(message: string) {
    super(message);
    this.name = 'SocketClosedError';
    this.code = 'ESOCKETCLOSED';
  }
}

/**
 * Pooled socket with creation timestamp for age-based eviction.
 */
interface PooledSocket {
  socket: net.Socket;
  createdAt: number;
}

/**
 * Result of acquiring a socket from the pool.
 */
interface AcquiredSocket {
  socket: net.Socket;
  createdAt: number;
}

/**
 * Socket Pool - Reuses socket connections across tasks in the same worker.
 * Creating a socket takes 0.1-1ms. Pooling reduces this overhead.
 *
 * CRITICAL: The Rust server has a 60-second TOTAL CONNECTION LIFETIME (not idle).
 * This means sockets created at T=0 will be closed at T=60, regardless of activity.
 * We track per-socket creation time and discard sockets older than 50 seconds.
 */
class SocketPool {
  private available: PooledSocket[] = [];
  private readonly maxSize = 5; // Max sockets to cache per worker
  // Rust server connection lifetime is 60s. Discard sockets older than 50s.
  private readonly maxSocketAgeMs = 50_000;

  acquire(): AcquiredSocket | null {
    const now = Date.now();

    // Pop sockets until we find a valid one or pool is empty
    while (this.available.length > 0) {
      const pooled = this.available.pop()!;
      const age = now - pooled.createdAt;

      // Discard sockets older than max age (Rust will close them soon anyway)
      if (age > this.maxSocketAgeMs) {
        pooled.socket.destroy();
        continue;
      }

      // Check socket health
      if (pooled.socket.writable && !pooled.socket.destroyed) {
        return { socket: pooled.socket, createdAt: pooled.createdAt };
      }

      // Socket unhealthy, destroy and try next
      pooled.socket.destroy();
    }

    // No valid socket found
    return null;
  }

  /**
   * Release a socket back to the pool.
   * @param socket The socket to release
   * @param createdAt When the socket was originally created (for age tracking)
   */
  release(socket: net.Socket, createdAt: number): void {
    const now = Date.now();
    const age = now - createdAt;

    // Don't pool sockets that are already old or unhealthy
    if (
      age > this.maxSocketAgeMs ||
      !socket.writable ||
      socket.destroyed ||
      this.available.length >= this.maxSize
    ) {
      socket.destroy();
      return;
    }

    this.available.push({ socket, createdAt });
  }

  clear(): void {
    // Clean up all pooled sockets
    for (const pooled of this.available) {
      pooled.socket.destroy();
    }
    this.available = [];
  }
}

// Worker-level caches (thread-safe since Piscina workers are single-threaded)
const functionCache = new FunctionCache();
const socketPool = new SocketPool();

/**
 * Task payload received from the worker pool
 */
export interface ExecutorTask {
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
  /** Wildcard route path (e.g., "/verify" from "/api/v1/plugins/:id/*") */
  route?: string;
  /** Plugin configuration object */
  config?: any;
  /** HTTP method (GET, POST, etc.) */
  method?: string;
  /** Query parameters */
  query?: any;
}

/**
 * Result returned from plugin execution
 */
export interface ExecutorResult {
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
 * Plugin API that communicates with the relayer via Unix socket.
 * Connection is lazy - only established when first API call is made.
 * Uses socket pooling to reduce connection overhead.
 */
class PluginAPIImpl implements PluginAPI {
  private socket: net.Socket | null = null;
  private pending: Map<string, { resolve: (value: any) => void; reject: (reason: any) => void }>;
  private connectionPromise: Promise<void> | null = null;
  private connected: boolean = false;
  private httpRequestId?: string;
  private socketPath: string;
  private readonly maxPendingRequests = 100; // Prevent memory leak from unbounded pending
  private socketCreatedAt: number = 0; // Track socket creation time for pool age-based eviction

  // Store handler references for proper cleanup (prevents listener accumulation)
  private boundErrorHandler: ((error: Error) => void) | null = null;
  private boundCloseHandler: (() => void) | null = null;
  private boundDataHandler: ((data: Buffer) => void) | null = null;

  constructor(socketPath: string, httpRequestId?: string) {
    this.socketPath = socketPath;
    this.pending = new Map();
    this.httpRequestId = httpRequestId;
  }

  private async ensureConnected(): Promise<void> {
    if (this.connected) return;

    if (!this.connectionPromise) {
      // Try to get socket from pool first
      const acquired = socketPool.acquire();

      if (acquired) {
        this.socket = acquired.socket;
        this.socketCreatedAt = acquired.createdAt;
        this.connected = true;
        this.connectionPromise = Promise.resolve();

        // Set up error/close handlers for pooled socket to enable reconnection
        this.setupSocketHandlers(this.socket);
      } else {
        // Create new socket if pool is empty
        this.socket = net.createConnection(this.socketPath);
        this.socketCreatedAt = Date.now();

        // Set up tracked handlers (can be removed later)
        this.setupSocketHandlers(this.socket);

        this.connectionPromise = new Promise((resolve, reject) => {
          // 'connect' is one-time event, use once() so it auto-removes
          this.socket!.once('connect', () => {
            this.connected = true;
            resolve();
          });

          // Additional one-time error handler for connection phase only
          this.socket!.once('error', reject);
        });
      }
    }

    await this.connectionPromise;
  }

  /**
   * Set up error/close/data handlers for a socket (pooled or new).
   * This ensures sockets can trigger reconnection on failure.
   * Stores references for proper cleanup to prevent listener accumulation.
   */
  private setupSocketHandlers(socket: net.Socket): void {
    // Create bound handlers so they can be removed later
    this.boundErrorHandler = (error: Error) => this.handleSocketError(error);
    this.boundCloseHandler = () => this.handleSocketClose();
    this.boundDataHandler = (data: Buffer) => this.handleSocketData(data);

    socket.on('error', this.boundErrorHandler);
    socket.on('close', this.boundCloseHandler);
    socket.on('data', this.boundDataHandler);
  }

  /**
   * Remove socket handlers before returning to pool.
   * Prevents listener accumulation (MaxListenersExceededWarning).
   */
  private removeSocketHandlers(socket: net.Socket): void {
    if (this.boundErrorHandler) {
      socket.removeListener('error', this.boundErrorHandler);
      this.boundErrorHandler = null;
    }
    if (this.boundCloseHandler) {
      socket.removeListener('close', this.boundCloseHandler);
      this.boundCloseHandler = null;
    }
    if (this.boundDataHandler) {
      socket.removeListener('data', this.boundDataHandler);
      this.boundDataHandler = null;
    }
  }

  /**
   * Handle socket error - reject pending requests and reset connection state
   */
  private handleSocketError(error: Error): void {
    this.rejectAllPending(error);
    // Reset connection state to force reconnection on next call
    this.connected = false;
    this.connectionPromise = null;
    this.socket = null;
  }

  /**
   * Handle socket close - reset connection state to enable reconnection.
   * Uses SocketClosedError to enable automatic retry in sendWithRetry().
   */
  private handleSocketClose(): void {
    this.connected = false;
    // Use SocketClosedError so retry logic can detect and handle this
    this.rejectAllPending(new SocketClosedError('Socket closed by server (connection lifetime exceeded)'));
    // Reset connection state to force reconnection on next call
    this.connectionPromise = null;
    this.socket = null;
  }

  /**
   * Handle incoming data from socket
   */
  private handleSocketData(data: Buffer): void {
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
    return this.sendWithRetry(relayerId, method, payload, false);
  }

  /**
   * Send request with EPIPE retry logic.
   * EPIPE occurs when the pooled socket was closed by the server but client doesn't know yet.
   * On EPIPE, we destroy the stale socket and retry with a fresh connection.
   */
  private async sendWithRetry<T>(
    relayerId: string,
    method: string,
    payload: any,
    isRetry: boolean
  ): Promise<T> {
    const requestId = uuidv4();
    const msg: any = { requestId, relayerId, method, payload };
    if (this.httpRequestId) {
      msg.httpRequestId = this.httpRequestId;
    }
    const message = JSON.stringify(msg) + '\n';

    // Check pending request limit to prevent memory leaks
    if (this.pending.size >= this.maxPendingRequests) {
      throw new Error(
        `Too many concurrent API requests (max ${this.maxPendingRequests}). ` +
        `Await previous requests before making new ones.`
      );
    }

    // Ensure we're connected before sending
    await this.ensureConnected();

    try {
      // Capture socket reference locally to guard against TOCTOU race condition.
      // Socket can become null between ensureConnected() and write() if an error/close
      // event fires asynchronously. This is especially likely after stress testing when
      // pooled connections may be in a degraded state.
      const socket = this.socket;
      if (!socket) {
        // Socket was nullified by error/close handler between ensureConnected and now.
        // Treat as a stale connection error that can be retried.
        const staleError = new Error('Socket became unavailable after connection') as NodeJS.ErrnoException;
        staleError.code = 'ECONNRESET';
        throw staleError;
      }

      return await new Promise<T>((resolve, reject) => {
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

        socket.write(message, (error) => {
          if (error) {
            if (timeoutId) clearTimeout(timeoutId);
            this.pending.delete(requestId);
            reject(error);
          }
        });
      });
    } catch (error: any) {
      // Handle connection errors - stale socket from pool or server-side closure
      // EPIPE: write to closed socket (client doesn't know server closed)
      // ECONNRESET: connection reset by peer (server forcefully closed)
      // ESOCKETCLOSED: server closed connection (e.g., 60-second lifetime expired)
      const isRetryableError =
        error.code === 'EPIPE' ||
        error.code === 'ECONNRESET' ||
        error.code === 'ESOCKETCLOSED';

      if (!isRetry && isRetryableError) {
        // Destroy the stale socket (don't return to pool)
        if (this.socket) {
          this.removeSocketHandlers(this.socket);
          this.socket.destroy();
          this.socket = null;
        }
        this.connected = false;
        this.connectionPromise = null;

        // Retry with fresh connection (bypass pool by clearing state)
        return this.sendWithRetry(relayerId, method, payload, true);
      }
      throw error;
    }
  }

  close(): void {
    // Return socket to pool if healthy, otherwise destroy
    if (this.socket) {
      // Remove handlers BEFORE returning to pool to prevent listener accumulation
      this.removeSocketHandlers(this.socket);
      socketPool.release(this.socket, this.socketCreatedAt);
      this.socket = null;
    }
    this.connected = false;
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
 * Creates a console-like object that captures logs with lazy stringification.
 * Stringification is deferred until logs are accessed to reduce overhead.
 */
function createPluginConsole(logs: LogEntry[]): Console {
  const log = (level: LogEntry['level']) => (...args: any[]) => {
    // Store raw args, stringify lazily when accessed
    const entry: any = {
      level,
      _args: args, // Store raw args
      _stringified: false,
      _message: '',
    };

    // Lazy getter for message
    Object.defineProperty(entry, 'message', {
      get() {
        if (!this._stringified) {
          this._message = this._args.map((arg: any) =>
            typeof arg === 'object' ? safeStringify(arg) : String(arg)
          ).join(' ');
          this._stringified = true;
          // Clear raw args to free memory
          delete this._args;
        }
        return this._message;
      },
      enumerable: true,
      configurable: true,
    });

    logs.push(entry);
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
 * Executes a compiled plugin.
 * This is the Piscina worker function.
 * Uses function caching for performance.
 */
export default async function executePlugin(task: ExecutorTask): Promise<ExecutorResult> {
  const logs: LogEntry[] = [];
  const api = new PluginAPIImpl(task.socketPath, task.httpRequestId);
  const kv = new DefaultPluginKVStore(task.pluginId);

  try {
    // Create console that captures logs
    const pluginConsole = createPluginConsole(logs);

    // Create module-like objects for CommonJS compatibility
    const moduleExports: any = {};
    const moduleObject = { exports: moduleExports };

    // Try to get cached factory function, otherwise compile and cache
    let factory = functionCache.get(task.compiledCode);
    if (!factory) {
      // eslint-disable-next-line no-new-func
      factory = new Function(
        'exports',
        'require',
        'module',
        '__filename',
        '__dirname',
        'console',
        task.compiledCode
      );
      functionCache.set(task.compiledCode, factory);
    }

    // Execute the factory to populate module.exports
    // Pass our custom console to capture logs
    factory(moduleExports, require, moduleObject, `plugin-${task.pluginId}.js`, '/plugins', pluginConsole);

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
          route: task.route ?? '',
          config: task.config,
          method: task.method ?? 'POST',
          query: task.query ?? {},
        };
        return await handler(pluginContext);
      } else {
        // Legacy 2-param handler (no KV/headers access)
        return await handler(api, task.params);
      }
    })();

    // Race handler against timeout to prevent worker starvation
    let timeoutId: NodeJS.Timeout | undefined;
    const timeoutPromise = new Promise<never>((_, reject) => {
      timeoutId = setTimeout(() => {
        const error = new Error(`Plugin handler timed out after ${task.timeout}ms`);
        (error as any).code = 'ERR_HANDLER_TIMEOUT';
        reject(error);
      }, task.timeout);
    });

    try {
      result = await Promise.race([handlerPromise, timeoutPromise]);
    } finally {
      // Clear timeout if handler completed before timeout
      if (timeoutId) clearTimeout(timeoutId);
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
    // Close API socket (non-blocking, don't throw)
    try {
      api.close();
    } catch {
      // Log but don't fail - cleanup is best-effort
    }

    // Disconnect KV (async, don't throw)
    try {
      await kv.disconnect();
    } catch {
      // Ignore disconnect errors - connection may not have been established
    }
  }
}
