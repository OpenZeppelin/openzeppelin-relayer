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
 * Environment Variables for tuning (high concurrency e.g., 1000+ parallel requests):
 * - PLUGIN_POOL_DEBUG: Set to 'true' to enable debug logging
 * - PLUGIN_POOL_MAX_THREADS: Maximum worker threads (default: CPU count or 8, whichever is higher)
 * - PLUGIN_POOL_CONCURRENT_TASKS: Concurrent tasks per worker (default: 10)
 * - PLUGIN_POOL_IDLE_TIMEOUT: Idle timeout in ms (default: 60000)
 * - PLUGIN_POOL_BACKLOG: Socket connection backlog (default: 1024)
 * - PLUGIN_POOL_MAX_CONNECTIONS: Max Rust->Pool connections (default: 64, set in Rust)
 */

import * as net from 'node:net';
import * as fs from 'node:fs';
import { WorkerPoolManager, type PluginExecutionRequest } from './worker-pool';
import { compilePlugin, compilePluginSource } from './compiler';
import {
  DEFAULT_POOL_MIN_THREADS,
  DEFAULT_POOL_MIN_THREADS_DIVISOR,
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

  constructor(socketPath: string) {
    this.socketPath = socketPath;
    
    // Default to number of CPUs for worker threads, with higher concurrency per worker
    // For 1000+ parallel requests, tune via environment variables
    const cpuCount = require('os').cpus().length;
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
    
    debug(`Initializing pool with maxThreads=${maxThreads}, concurrentTasksPerWorker=${concurrentTasksPerWorker}`);
    
    this.pool = new WorkerPoolManager({
      minThreads: Math.max(DEFAULT_POOL_MIN_THREADS, Math.floor(cpuCount / DEFAULT_POOL_MIN_THREADS_DIVISOR)),
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
    
    // Don't set maxConnections at all - the default is unlimited
    // Setting it to 0 might reject all connections!
    // this.server.maxConnections = 0; // REMOVED
    
    // Log any server-level errors
    this.server.on('error', (err) => {
      console.error('[pool-server] Server error:', err);
    });
    
    this.server.on('connection', (socket) => {
      debug('Server connection event from:', socket.remoteAddress);
    });

    // Backlog for pending connections (default 511, increase for high concurrency)
    const backlog = parseInt(process.env.PLUGIN_POOL_BACKLOG || String(DEFAULT_POOL_SOCKET_BACKLOG), 10);

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
    let processing = false;
    const pendingLines: string[] = [];

    const processQueue = async () => {
      if (processing) return;
      processing = true;
      
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
      
      processing = false;
    };

    socket.on('data', (data) => {
      buffer += data.toString();
      debug(`[${clientId}] Received data: ${data.length} bytes, buffer: ${buffer.length}`);
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
      });
    });

    socket.on('error', (err) => {
      // Connection resets are normal during shutdown, don't log as errors
      if ((err as any).code === 'ECONNRESET') {
        debug(`[${clientId}] Connection reset`);
      } else {
        console.warn(`[pool-server] [${clientId}] Socket error:`, err.message);
      }
    });
    
    socket.on('close', () => {
      debug(`[${clientId}] Client disconnected`);
    });
  }

  /**
   * Handle a message from Rust
   */
  private async handleMessage(message: Message): Promise<Response> {
    const { taskId } = message;

    try {
      switch (message.type) {
        case 'execute':
          return await this.handleExecute(message);

        case 'precompile':
          return await this.handlePrecompile(message);

        case 'cache':
          return this.handleCache(message);

        case 'invalidate':
          return this.handleInvalidate(message);

        case 'stats':
          return this.handleStats(message);

        case 'health':
          return this.handleHealth(message);

        case 'shutdown':
          return await this.handleShutdown(message);

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
   * Shutdown the server
   */
  private async handleShutdown(message: ShutdownMessage): Promise<Response> {
    // Send response first, then shutdown
    setTimeout(async () => {
      await this.stop();
      process.exit(0);
    }, 100);

    return {
      taskId: message.taskId,
      success: true,
    };
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

  const server = new PoolServer(socketPath);

  // Handle uncaught exceptions to prevent silent crashes
  process.on('uncaughtException', (err) => {
    console.error('[pool-server] Uncaught exception:', err);
  });

  process.on('unhandledRejection', (reason, promise) => {
    console.error('[pool-server] Unhandled rejection at:', promise, 'reason:', reason);
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
