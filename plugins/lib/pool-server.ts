#!/usr/bin/env node

/**
 * Pool Server
 *
 * Long-running Node.js process that manages the Piscina worker pool.
 * Communicates with Rust via Unix socket using JSON-line protocol.
 *
 * Protocol:
 * - Each message is a single JSON line terminated by \n
 * - Request: { type: "execute", taskId, pluginId, compiledCode?, pluginPath?, params, headers?, socketPath, httpRequestId?, timeout? }
 * - Request: { type: "precompile", taskId, pluginId, pluginPath?, sourceCode? }
 * - Request: { type: "cache", taskId, pluginId, compiledCode }
 * - Request: { type: "invalidate", taskId, pluginId }
 * - Request: { type: "stats", taskId }
 * - Request: { type: "shutdown", taskId }
 * - Response: { taskId, success, result?, error?, logs? }
 *
 * Configuration:
 * 
 * All configuration is derived from PLUGIN_MAX_CONCURRENCY in Rust's config.rs
 * and passed via environment variables when spawning this process.
 * See: src/services/plugins/config.rs
 *
 * Environment Variables (passed from Rust):
 * - PLUGIN_MAX_CONCURRENCY: Primary scaling knob
 * - PLUGIN_POOL_MAX_THREADS: Worker threads (derived from concurrency)
 * - PLUGIN_POOL_CONCURRENT_TASKS: Tasks per worker (derived from concurrency)
 * - PLUGIN_POOL_IDLE_TIMEOUT: Idle timeout in ms
 * - PLUGIN_POOL_DEBUG: Set to 'true' to enable debug logging
 */

import * as net from 'node:net';
import * as fs from 'node:fs';
import * as v8 from 'node:v8';
import { WorkerPoolManager, type PluginExecutionRequest } from './worker-pool';
import {
  DEFAULT_POOL_MIN_THREADS,
  DEFAULT_POOL_MAX_THREADS_FLOOR,
  DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER,
  DEFAULT_POOL_IDLE_TIMEOUT_MS,
  DEFAULT_POOL_SOCKET_BACKLOG,
} from './constants';

// Debug logging helper
const DEBUG = process.env.PLUGIN_POOL_DEBUG === 'true';
function debug(...args: any[]): void {
  if (DEBUG) {
    console.error('[pool-server]', ...args);
  }
}

/**
 * Memory Pressure Monitor - Pool Server Edition
 * Monitors main process heap and logs warnings when approaching limits.
 * Pool server manages workers, code cache, and socket communication.
 *
 * SELF-HEALING FEATURES:
 * - Proactive GC triggering at 80% heap usage
 * - Cache eviction at 85% heap usage
 * - Emergency restart signal at 95% (tells Rust to restart)
 *
 * ASYNC SAFETY:
 * - All callbacks are queued to event loop via setImmediate to avoid blocking the timer
 * - Emergency shutdown is tracked to prevent duplicate invocations
 */
class PoolServerMemoryMonitor {
  private checkInterval: NodeJS.Timeout | null = null;
  private readonly warningThreshold = 0.75; // 75% of heap - start monitoring
  private readonly gcThreshold = 0.80; // 80% of heap - force GC
  private readonly criticalThreshold = 0.85; // 85% of heap - evict caches
  private readonly emergencyThreshold = 0.92; // 92% of heap - signal restart
  private lastWarningTime = 0;
  private readonly warningCooldown = 10000; // 10 seconds between warnings (more frequent)
  private consecutiveHighPressure = 0;
  private onCacheEvictionRequest: (() => void) | null = null;
  private onEmergencyRestart: (() => void) | null = null;
  /** Tracks if emergency shutdown has already been triggered (prevents duplicate calls) */
  private emergencyTriggered = false;

  start(): void {
    if (this.checkInterval) return;
    
    // Check memory pressure every 5 seconds (more frequent for faster response)
    this.checkInterval = setInterval(() => {
      this.checkMemoryPressure();
    }, 5000);
    
    // Don't prevent process exit
    this.checkInterval.unref();
    
    console.error('[pool-server] Memory monitoring started (thresholds: 75%/80%/85%/92%)');
  }

  stop(): void {
    if (this.checkInterval) {
      clearInterval(this.checkInterval);
      this.checkInterval = null;
    }
  }

  /**
   * Register callback for cache eviction requests.
   */
  onCacheEviction(callback: () => void): void {
    this.onCacheEvictionRequest = callback;
  }

  /**
   * Register callback for emergency restart signal.
   */
  onEmergency(callback: () => void): void {
    this.onEmergencyRestart = callback;
  }

  private checkMemoryPressure(): void {
    const usage = process.memoryUsage();
    const heapStats = v8.getHeapStatistics();
    const heapUsed = usage.heapUsed;
    // Use heap_size_limit (V8's configured maximum heap) instead of heapTotal
    // heapTotal is the currently allocated heap which grows dynamically,
    // heap_size_limit is the actual maximum (e.g., from --max-old-space-size)
    const heapLimit = heapStats.heap_size_limit;
    const heapUsedRatio = heapUsed / heapLimit;

    // Track consecutive high pressure for emergency detection
    if (heapUsedRatio >= this.criticalThreshold) {
      this.consecutiveHighPressure++;
    } else if (heapUsedRatio < this.warningThreshold) {
      this.consecutiveHighPressure = 0;
    }

    // Emergency: persistent critical pressure = GC can't keep up
    if (heapUsedRatio >= this.emergencyThreshold || this.consecutiveHighPressure >= 6) {
      this.handleEmergency(heapUsed, heapLimit, usage.external, usage.rss);
    } else if (heapUsedRatio >= this.criticalThreshold) {
      this.handleCriticalPressure(heapUsed, heapLimit, usage.external, usage.rss);
    } else if (heapUsedRatio >= this.gcThreshold) {
      this.handleGCPressure(heapUsed, heapLimit);
    } else if (heapUsedRatio >= this.warningThreshold) {
      this.handleWarningPressure(heapUsed, heapLimit, usage.external, usage.rss);
    }
  }

  private handleEmergency(heapUsed: number, heapLimit: number, external: number, rss: number): void {
    // Prevent duplicate emergency shutdowns
    if (this.emergencyTriggered) {
      return;
    }
    this.emergencyTriggered = true;

    const heapUsedMB = Math.round(heapUsed / 1024 / 1024);
    const heapLimitMB = Math.round(heapLimit / 1024 / 1024);
    const percent = Math.round((heapUsed / heapLimit) * 100);

    console.error(
      `[pool-server] EMERGENCY: Heap at ${heapUsedMB}MB / ${heapLimitMB}MB limit (${percent}%). ` +
      `Consecutive high pressure: ${this.consecutiveHighPressure}. ` +
      `GC cannot keep up - signaling Rust for graceful restart.`
    );

    // Try last-ditch GC
    if (global.gc) {
      try {
        global.gc();
      } catch {
        // Ignore
      }
    }

    // Queue emergency callback to event loop to avoid blocking the timer
    // This ensures the setInterval can continue and the async shutdown
    // is handled properly without blocking other operations
    if (this.onEmergencyRestart) {
      setImmediate(() => {
        this.onEmergencyRestart!();
      });
    }

    // Reset counter after emergency signal
    this.consecutiveHighPressure = 0;
  }

  private handleCriticalPressure(heapUsed: number, heapLimit: number, external: number, rss: number): void {
    const now = Date.now();
    if (now - this.lastWarningTime < this.warningCooldown) return;
    this.lastWarningTime = now;

    const heapUsedMB = Math.round(heapUsed / 1024 / 1024);
    const heapLimitMB = Math.round(heapLimit / 1024 / 1024);
    const externalMB = Math.round(external / 1024 / 1024);
    const rssMB = Math.round(rss / 1024 / 1024);
    const percent = Math.round((heapUsed / heapLimit) * 100);

    console.error(
      `[pool-server] CRITICAL: Heap at ${heapUsedMB}MB / ${heapLimitMB}MB limit (${percent}%). ` +
      `External: ${externalMB}MB, RSS: ${rssMB}MB. ` +
      `Requesting cache eviction and forcing GC.`
    );

    // Request cache eviction - queue to event loop to avoid blocking timer
    if (this.onCacheEvictionRequest) {
      setImmediate(() => {
        this.onCacheEvictionRequest!();
      });
    }

    // Force GC
    if (global.gc) {
      try {
        global.gc();
        console.error(`[pool-server] Forced GC triggered`);
      } catch (err) {
        console.error(`[pool-server] Failed to trigger GC:`, err);
      }
    }
  }

  private handleGCPressure(heapUsed: number, heapLimit: number): void {
    // Silently trigger GC at 80% threshold
    if (global.gc) {
      try {
        global.gc();
      } catch {
        // Ignore
      }
    }
  }

  private handleWarningPressure(heapUsed: number, heapLimit: number, external: number, rss: number): void {
    const now = Date.now();
    if (now - this.lastWarningTime < this.warningCooldown * 3) return; // Less frequent warnings
    this.lastWarningTime = now;

    const heapUsedMB = Math.round(heapUsed / 1024 / 1024);
    const heapLimitMB = Math.round(heapLimit / 1024 / 1024);
    const percent = Math.round((heapUsed / heapLimit) * 100);
    
    console.error(
      `[pool-server] WARNING: Heap at ${heapUsedMB}MB / ${heapLimitMB}MB limit (${percent}%).`
    );
  }
}

// Message types
interface BaseMessage {
  type: string;
  taskId: string;
}

interface ExecuteMessage extends BaseMessage {
  type: 'execute';
  pluginId: string;
  compiledCode?: string;
  pluginPath?: string;
  params: any;
  headers?: Record<string, string[]>;
  socketPath: string;
  httpRequestId?: string;
  timeout?: number;
  route?: string;
  config?: any;
  method?: string;
  query?: any;
}

interface PrecompileMessage extends BaseMessage {
  type: 'precompile';
  pluginId: string;
  pluginPath?: string;
  sourceCode?: string;
}

interface CacheMessage extends BaseMessage {
  type: 'cache';
  pluginId: string;
  compiledCode: string;
}

interface InvalidateMessage extends BaseMessage {
  type: 'invalidate';
  pluginId: string;
}

interface StatsMessage extends BaseMessage {
  type: 'stats';
}

interface HealthMessage extends BaseMessage {
  type: 'health';
}

interface ShutdownMessage extends BaseMessage {
  type: 'shutdown';
}

type Message =
  | ExecuteMessage
  | PrecompileMessage
  | CacheMessage
  | InvalidateMessage
  | StatsMessage
  | HealthMessage
  | ShutdownMessage;

interface Response {
  taskId: string;
  success: boolean;
  result?: any;
  error?: {
    message: string;
    code?: string;
    status?: number;
    details?: any;
  };
  logs?: Array<{ level: string; message: string }>;
}

/**
 * Pool Server - manages worker pool and handles requests from Rust
 */
class PoolServer {
  private pool: WorkerPoolManager;
  private server: net.Server | null = null;
  private socketPath: string;
  private running: boolean = false;
  private shuttingDown: boolean = false;
  private activeRequests: number = 0;
  private readonly shutdownTimeoutMs: number = 30000; // 30 seconds max drain time

  constructor(socketPath: string) {
    this.socketPath = socketPath;
    
    const cpuCount = require('os').cpus().length;
    
    // =========================================================================
    // Configuration - ALL values are passed from Rust (single source of truth)
    // =========================================================================
    // Rust's config.rs derives all values from PLUGIN_MAX_CONCURRENCY and
    // passes them via environment variables when spawning this process.
    //
    // Fallbacks are provided only for manual testing without Rust.
    //
    const maxConcurrency = parseInt(process.env.PLUGIN_MAX_CONCURRENCY || '2048', 10);
    const minThreads = parseInt(
      process.env.PLUGIN_POOL_MIN_THREADS || String(Math.max(DEFAULT_POOL_MIN_THREADS, Math.floor(cpuCount / 2))),
      10
    );
    const maxThreads = parseInt(
      process.env.PLUGIN_POOL_MAX_THREADS || String(Math.max(cpuCount, DEFAULT_POOL_MAX_THREADS_FLOOR)),
      10
    );
    const concurrentTasksPerWorker = parseInt(
      process.env.PLUGIN_POOL_CONCURRENT_TASKS || String(DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER),
      10
    );
    const idleTimeout = parseInt(
      process.env.PLUGIN_POOL_IDLE_TIMEOUT || String(DEFAULT_POOL_IDLE_TIMEOUT_MS),
      10
    );
    
    // Log effective configuration (received from Rust)
    console.error(`[pool-server] Configuration (from Rust): maxConcurrency=${maxConcurrency}, minThreads=${minThreads}, maxThreads=${maxThreads}, concurrentTasksPerWorker=${concurrentTasksPerWorker}, idleTimeout=${idleTimeout}ms`);
    
    // WorkerPoolManager handles cache lifecycle with active eviction (runs every 5 mins)
    // Cache is properly cleaned up on shutdown via pool.destroy()
    this.pool = new WorkerPoolManager({
      minThreads,
      maxThreads,
      concurrentTasksPerWorker,
      idleTimeout,
    });
  }

  /**
   * Start the pool server
   */
  async start(): Promise<void> {
    debug('Initializing worker pool...');
    // Initialize the worker pool
    await this.pool.initialize();
    debug('Worker pool initialized');

    // Clean up any existing socket file
    try {
      fs.unlinkSync(this.socketPath);
      debug('Removed existing socket file');
    } catch {
      // Ignore if doesn't exist
    }

    // Create Unix socket server with high connection backlog for bursts
    this.server = net.createServer((socket) => {
      debug('net.createServer callback - new connection');
      this.handleConnection(socket);
    });
    
    
    // Log any server-level errors
    this.server.on('error', (err) => {
      console.error('[pool-server] Server error:', err);
    });
    
    this.server.on('connection', (socket) => {
      debug('Server connection event from:', socket.remoteAddress);
    });

    // Backlog for pending connections (default 511, increase for high concurrency)
    // Note: Rust exports PLUGIN_POOL_SOCKET_BACKLOG with derived headroom
    const backlog = parseInt(process.env.PLUGIN_POOL_SOCKET_BACKLOG || String(DEFAULT_POOL_SOCKET_BACKLOG), 10);

    return new Promise((resolve, reject) => {
      this.server!.on('error', (err) => {
        console.error('[pool-server] Listen error:', err);
        reject(err);
      });

      this.server!.listen(this.socketPath, backlog, () => {
        this.running = true;
        debug(`Listening on ${this.socketPath} (backlog=${backlog})`);
        
        // Verify the socket file exists
        try {
          const stats = fs.statSync(this.socketPath);
          debug(`Socket file verified: mode=${stats.mode.toString(8)}, isSocket=${stats.isSocket()}`);
        } catch (e: any) {
          console.warn(`[pool-server] Socket file check failed:`, e.message);
        }
        
        // Verify server state
        const addr = this.server!.address();
        debug(`Server address:`, addr);
        debug(`Server listening:`, this.server!.listening);
        
        resolve();
      });
    });
  }

  /**
   * Handle a client connection
   */
  private handleConnection(socket: net.Socket): void {
    const clientId = Math.random().toString(36).substring(7);
    debug(`[${clientId}] Client connected`);
    
    // Enable keep-alive to prevent connection drops
    socket.setKeepAlive(true, 30000); // 30 second keep-alive probe
    socket.setNoDelay(true); // Disable Nagle's algorithm for lower latency
    
    let buffer = '';
    let processingPromise: Promise<void> | null = null;
    const pendingLines: string[] = [];

    // Max buffer size to prevent OOM from malicious clients (10MB)
    const MAX_BUFFER_SIZE = 10 * 1024 * 1024;

    const processQueue = async (): Promise<void> => {
      // Wait for any in-progress processing to complete (proper mutual exclusion)
      if (processingPromise) {
        await processingPromise;
      }
      
      if (pendingLines.length === 0) return;
      
      processingPromise = (async () => {
        while (pendingLines.length > 0) {
          const line = pendingLines.shift()!;
          if (!line.trim()) continue;
          
          try {
            const message = JSON.parse(line) as Message;
            debug('Processing message type:', message.type);
            const response = await this.handleMessage(message);
            debug('Sending response for task:', response.taskId);
            // Check if socket is still writable before writing
            if (socket.writable) {
              socket.write(JSON.stringify(response) + '\n');
            } else {
              debug('Socket no longer writable, discarding response');
            }
          } catch (err) {
            const error = err as Error;
            debug('Error handling message:', error);
            const response: Response = {
              taskId: 'unknown',
              success: false,
              error: {
                message: error.message || 'Failed to parse message',
                code: 'PARSE_ERROR',
              },
            };
            if (socket.writable) {
              socket.write(JSON.stringify(response) + '\n');
            }
          }
        }
      })();
      
      await processingPromise;
      processingPromise = null;
    };

    // Define handlers for proper cleanup on close
    const dataHandler = (data: Buffer): void => {
      buffer += data.toString();
      debug(`[${clientId}] Received data: ${data.length} bytes, buffer: ${buffer.length}`);

      // Buffer overflow protection - disconnect malicious clients
      if (buffer.length > MAX_BUFFER_SIZE) {
        console.error(`[pool-server] [${clientId}] Buffer overflow (${buffer.length} bytes), disconnecting`);
        socket.destroy();
        return;
      }

      debug(`[${clientId}] Buffer content: ${buffer.substring(0, 200)}...`);

      // Extract complete messages (newline-delimited)
      let newlineIndex;
      while ((newlineIndex = buffer.indexOf('\n')) !== -1) {
        const line = buffer.slice(0, newlineIndex);
        buffer = buffer.slice(newlineIndex + 1);
        pendingLines.push(line);
        debug(`[${clientId}] Queued message: ${line.substring(0, 100)}...`);
      }
      
      // Process queue (async but don't await here)
      processQueue().catch((err) => {
        console.error(`[pool-server] [${clientId}] Error in processQueue:`, err);
        // Send error response so client doesn't hang
        const response: Response = {
          taskId: 'unknown',
          success: false,
          error: { message: 'Internal queue processing error', code: 'QUEUE_ERROR' },
        };
        if (socket.writable) {
          socket.write(JSON.stringify(response) + '\n');
        }
      });
    };

    const errorHandler = (err: Error): void => {
      // Connection resets are normal during shutdown, don't log as errors
      if ((err as any).code === 'ECONNRESET') {
        debug(`[${clientId}] Connection reset`);
      } else {
        console.error(`[pool-server] [${clientId}] Socket error:`, err.message);
      }
    };
    
    const closeHandler = (): void => {
      // Explicit cleanup of listeners (good practice for long-running servers)
      socket.removeListener('data', dataHandler);
      socket.removeListener('error', errorHandler);
      debug(`[${clientId}] Client disconnected, listeners cleaned up`);
    };

    socket.on('data', dataHandler);
    socket.on('error', errorHandler);
    socket.once('close', closeHandler);
  }

  /**
   * Handle a message from Rust
   */
  private async handleMessage(message: Message): Promise<Response> {
    const { taskId } = message;

    // Allow shutdown and health messages during shutdown, reject others
    if (this.shuttingDown && message.type !== 'shutdown' && message.type !== 'health') {
      return {
        taskId,
        success: false,
        error: {
          message: 'Server is shutting down, not accepting new requests',
          code: 'SHUTTING_DOWN',
        },
      };
    }

    // Track active requests for graceful shutdown draining
    // Work requests get try/finally to ensure counter accuracy
    const isWorkRequest = message.type === 'execute' || message.type === 'precompile';

    if (isWorkRequest) {
      this.activeRequests++;
      try {
        return await this.handleWorkRequest(message);
      } finally {
        this.activeRequests--;
      }
    }

    // Non-work requests don't need tracking
    return this.handleNonWorkRequest(message);
  }

  /**
   * Handle work requests (execute, precompile) - tracked for graceful shutdown
   */
  private async handleWorkRequest(message: ExecuteMessage | PrecompileMessage): Promise<Response> {
    const { taskId } = message;
    try {
      switch (message.type) {
        case 'execute':
          return await this.handleExecute(message);
        case 'precompile':
          return await this.handlePrecompile(message);
        default:
          // TypeScript exhaustiveness check
          const _exhaustive: never = message;
          return {
            taskId,
            success: false,
            error: { message: 'Unknown work request type', code: 'UNKNOWN_TYPE' },
          };
      }
    } catch (err) {
      const error = err as Error;
      return {
        taskId,
        success: false,
        error: {
          message: error.message || String(err),
          code: 'HANDLER_ERROR',
        },
      };
    }
  }

  /**
   * Handle non-work requests (cache, invalidate, stats, health, shutdown)
   */
  private async handleNonWorkRequest(message: Message): Promise<Response> {
    const { taskId } = message;
    try {
      switch (message.type) {
        case 'cache':
          return this.handleCache(message as CacheMessage);
        case 'invalidate':
          return this.handleInvalidate(message as InvalidateMessage);
        case 'stats':
          return this.handleStats(message as StatsMessage);
        case 'health':
          return this.handleHealth(message as HealthMessage);
        case 'shutdown':
          return await this.handleShutdown(message as ShutdownMessage);
        default:
          return {
            taskId,
            success: false,
            error: {
              message: `Unknown message type: ${(message as any).type}`,
              code: 'UNKNOWN_MESSAGE_TYPE',
            },
          };
      }
    } catch (err) {
      const error = err as Error;
      return {
        taskId,
        success: false,
        error: {
          message: error.message || String(err),
          code: 'HANDLER_ERROR',
        },
      };
    }
  }

  /**
   * Execute a plugin
   */
  private async handleExecute(message: ExecuteMessage): Promise<Response> {
    debug('handleExecute called with:', JSON.stringify({
      pluginId: message.pluginId,
      pluginPath: message.pluginPath,
      hasCompiledCode: !!message.compiledCode,
      socketPath: message.socketPath,
      timeout: message.timeout,
    }));

    const request: PluginExecutionRequest = {
      pluginId: message.pluginId,
      pluginPath: message.pluginPath,
      compiledCode: message.compiledCode,
      params: message.params,
      headers: message.headers,
      socketPath: message.socketPath,
      httpRequestId: message.httpRequestId,
      timeout: message.timeout,
      route: message.route,
      config: message.config,
      method: message.method,
      query: message.query,
    };

    try {
      debug('Calling pool.runPlugin...');
      const result = await this.pool.runPlugin(request);
      debug('runPlugin returned:', JSON.stringify({
        success: result.success,
        hasResult: !!result.result,
        hasError: !!result.error,
      }));

      return {
        taskId: message.taskId,
        success: result.success,
        result: result.result,
        error: result.error,
        logs: result.logs,
      };
    } catch (err) {
      debug('runPlugin threw error:', err);
      const error = err as any;
      
      // Provide detailed error context
      return {
        taskId: message.taskId,
        success: false,
        error: {
          message: error?.message || String(err),
          code: error?.code || 'POOL_EXECUTION_ERROR',
          status: typeof error?.status === 'number' ? error.status : 500,
          details: {
            pluginId: message.pluginId,
            errorType: error?.name || 'Error',
            stack: error?.stack?.split('\n').slice(0, 5).join('\n'),
          },
        },
        logs: [],
      };
    }
  }

  /**
   * Precompile a plugin
   */
  private async handlePrecompile(message: PrecompileMessage): Promise<Response> {
    let compilationResult;

    if (message.sourceCode) {
      compilationResult = await this.pool.precompilePluginSource(
        message.pluginId,
        message.sourceCode
      );
    } else if (message.pluginPath) {
      compilationResult = await this.pool.precompilePlugin(message.pluginPath);
    } else {
      return {
        taskId: message.taskId,
        success: false,
        error: {
          message: 'Either sourceCode or pluginPath is required for precompilation',
          code: 'MISSING_SOURCE',
        },
      };
    }

    return {
      taskId: message.taskId,
      success: true,
      result: {
        code: compilationResult.code,
        warnings: compilationResult.warnings,
      },
    };
  }

  /**
   * Cache compiled code
   */
  private handleCache(message: CacheMessage): Response {
    this.pool.cacheCompiledCode(message.pluginId, message.compiledCode);
    return {
      taskId: message.taskId,
      success: true,
    };
  }

  /**
   * Invalidate cached plugin
   */
  private handleInvalidate(message: InvalidateMessage): Response {
    this.pool.invalidatePlugin(message.pluginId);
    return {
      taskId: message.taskId,
      success: true,
    };
  }

  /**
   * Get pool statistics
   */
  private handleStats(message: StatsMessage): Response {
    const stats = this.pool.getStats();
    return {
      taskId: message.taskId,
      success: true,
      result: stats,
    };
  }

  /**
   * Health check - returns basic health status
   */
  private handleHealth(message: HealthMessage): Response {
    const stats = this.pool.getStats();
    const memUsage = process.memoryUsage();
    
    return {
      taskId: message.taskId,
      success: true,
      result: {
        status: 'healthy',
        uptime: stats.uptime,
        memory: {
          heapUsed: memUsage.heapUsed,
          heapTotal: memUsage.heapTotal,
          rss: memUsage.rss,
        },
        pool: stats.pool ? {
          completed: stats.pool.completed,
          queued: stats.pool.queued,
        } : null,
        execution: {
          total: stats.execution.total,
          successRate: stats.execution.successRate,
        },
      },
    };
  }

  /**
   * Shutdown the server gracefully
   * 1. Stop accepting new requests
   * 2. Wait for in-flight requests to complete (with timeout)
   * 3. Clean up resources and exit
   */
  private async handleShutdown(message: ShutdownMessage): Promise<Response> {
    if (this.shuttingDown) {
      return {
        taskId: message.taskId,
        success: true,
        result: { status: 'already_shutting_down', activeRequests: this.activeRequests },
      };
    }

    console.error(`[pool-server] Graceful shutdown initiated, ${this.activeRequests} active requests`);
    this.shuttingDown = true;

    // Wait for in-flight requests to drain, then shutdown
    // This runs asynchronously - we send the response first
    this.drainAndExit().catch((err) => {
      console.error('[pool-server] Error during graceful shutdown:', err);
      process.exit(1);
    });

    return {
      taskId: message.taskId,
      success: true,
      result: { 
        status: 'draining', 
        activeRequests: this.activeRequests,
        timeoutMs: this.shutdownTimeoutMs,
      },
    };
  }

  /**
   * Wait for active requests to complete, then exit
   */
  private async drainAndExit(): Promise<void> {
    const startTime = Date.now();
    const checkInterval = 100; // Check every 100ms

    // Wait for requests to drain
    while (this.activeRequests > 0) {
      const elapsed = Date.now() - startTime;
      if (elapsed >= this.shutdownTimeoutMs) {
        console.error(
          `[pool-server] Shutdown timeout (${this.shutdownTimeoutMs}ms) reached, ` +
          `${this.activeRequests} requests still active - forcing shutdown`
        );
        break;
      }

      // Log progress every 5 seconds
      if (elapsed > 0 && elapsed % 5000 < checkInterval) {
        console.error(
          `[pool-server] Draining: ${this.activeRequests} requests remaining ` +
          `(${Math.round(elapsed / 1000)}s / ${this.shutdownTimeoutMs / 1000}s)`
        );
      }

      await new Promise((resolve) => setTimeout(resolve, checkInterval));
    }

    const drainTime = Date.now() - startTime;
    console.error(`[pool-server] Drain complete in ${drainTime}ms, proceeding with shutdown`);

    await this.stop();
    process.exit(0);
  }

  /**
   * Evict code cache under memory pressure.
   * Called by memory monitor when heap usage is critical.
   */
  evictCache(): void {
    try {
      this.pool.clearCache();
      console.error('[pool-server] Code cache evicted due to memory pressure');
    } catch (err) {
      console.error('[pool-server] Failed to evict cache:', err);
    }
  }

  /**
   * Emergency shutdown due to memory pressure.
   * Uses shorter timeout but same drain logic for consistency.
   * Exit code 1 indicates abnormal termination (not 137 which is SIGKILL).
   */
  async emergencyShutdown(): Promise<void> {
    if (this.shuttingDown) {
      // Already shutting down, just wait
      return;
    }

    console.error(`[pool-server] Emergency shutdown initiated, ${this.activeRequests} active requests`);
    this.shuttingDown = true;

    // Shorter timeout for emergency (10 seconds vs normal 30)
    const emergencyTimeoutMs = 10000;
    const startTime = Date.now();
    const checkInterval = 100;

    // Wait for requests to drain with shorter timeout
    while (this.activeRequests > 0) {
      const elapsed = Date.now() - startTime;
      if (elapsed >= emergencyTimeoutMs) {
        console.error(
          `[pool-server] Emergency shutdown timeout (${emergencyTimeoutMs}ms) reached, ` +
          `${this.activeRequests} requests still active - forcing exit`
        );
        break;
      }

      if (elapsed > 0 && elapsed % 2000 < checkInterval) {
        console.error(
          `[pool-server] Emergency draining: ${this.activeRequests} requests remaining`
        );
      }

      await new Promise((resolve) => setTimeout(resolve, checkInterval));
    }

    const drainTime = Date.now() - startTime;
    console.error(`[pool-server] Emergency drain complete in ${drainTime}ms`);

    await this.stop();
    // Exit code 1 = abnormal termination (OOM-like condition)
    // Note: 137 is SIGKILL which is incorrect for voluntary exit
    process.exit(1);
  }

  /**
   * Stop the server
   */
  async stop(): Promise<void> {
    this.running = false;

    if (this.server) {
      await new Promise<void>((resolve) => {
        this.server!.close(() => resolve());
      });
      this.server = null;
    }

    await this.pool.shutdown();

    // Clean up socket file
    try {
      fs.unlinkSync(this.socketPath);
    } catch {
      // Ignore
    }

    console.log('Pool server stopped');
  }
}

// Main entry point
async function main(): Promise<void> {
  const socketPath = process.argv[2] || process.env.PLUGIN_POOL_SOCKET || '/tmp/plugin-pool.sock';

  debug('Starting with socket path:', socketPath);

  // Log heap configuration from NODE_OPTIONS
  const nodeOptions = process.env.NODE_OPTIONS || '';
  const maxOldSpaceMatch = nodeOptions.match(/--max-old-space-size=(\d+)/);
  const configuredHeapMB = maxOldSpaceMatch ? parseInt(maxOldSpaceMatch[1], 10) : 'default';
  const currentHeapMB = Math.round(process.memoryUsage().heapTotal / 1024 / 1024);
  
  console.error(
    `[pool-server] Heap configuration: ${configuredHeapMB}MB max ` +
    `(current: ${currentHeapMB}MB, will grow as needed)`
  );

  const server = new PoolServer(socketPath);

  // Start memory monitoring for pool server process
  const memoryMonitor = new PoolServerMemoryMonitor();
  
  // Connect memory monitor to pool for cache eviction
  memoryMonitor.onCacheEviction(() => {
    console.error('[pool-server] Memory pressure - clearing code cache');
    server.evictCache();
  });

  // Connect emergency handler - graceful shutdown to trigger Rust restart
  memoryMonitor.onEmergency(() => {
    console.error('[pool-server] Emergency memory pressure - initiating graceful shutdown for restart');
    // Reuse the same graceful shutdown logic as normal shutdown
    // This ensures active requests are drained properly (with timeout)
    // Rust will detect the exit and restart the process
    server.emergencyShutdown().catch((err) => {
      console.error('[pool-server] Error during emergency shutdown:', err);
      process.exit(1);
    });
  });

  memoryMonitor.start();

  // Handle uncaught exceptions to prevent silent crashes
  process.on('uncaughtException', async (err) => {
    console.error('[pool-server] Uncaught exception:', err);
    try {
      await server.stop();
    } catch (stopErr) {
      console.error('[pool-server] Error during cleanup:', stopErr);
    }
    process.exit(1);
  });

  process.on('unhandledRejection', async (reason, promise) => {
    console.error('[pool-server] Unhandled rejection at:', promise, 'reason:', reason);
    try {
      await server.stop();
    } catch (stopErr) {
      console.error('[pool-server] Error during cleanup:', stopErr);
    }
    process.exit(1);
  });

  // Handle signals for graceful shutdown
  process.on('SIGINT', async () => {
    debug('Received SIGINT, shutting down...');
    await server.stop();
    process.exit(0);
  });

  process.on('SIGTERM', async () => {
    debug('Received SIGTERM, shutting down...');
    await server.stop();
    process.exit(0);
  });

  try {
    await server.start();
    debug('Server started successfully, listening on', socketPath);
    // Write ready signal to stdout for Rust to detect
    console.log('POOL_SERVER_READY');
    
    // Keep the process alive forever - the server will handle connections
    debug('Entering event loop, waiting for connections...');
    
    // Periodic heartbeat to verify event loop is running (debug mode only)
    if (process.env.PLUGIN_POOL_DEBUG) {
      let heartbeatCount = 0;
      setInterval(() => {
        heartbeatCount++;
        if (heartbeatCount <= 5 || heartbeatCount % 60 === 0) {
          debug(`Heartbeat #${heartbeatCount} - event loop is running`);
        }
      }, 1000);
    }
    
    // Keep process alive
    await new Promise<void>(() => {});
  } catch (err) {
    console.error('[pool-server] Failed to start pool server:', err);
    process.exit(1);
  }
}

main().catch((err) => {
  console.error('[pool-server] Fatal error in main:', err);
  process.exit(1);
});
