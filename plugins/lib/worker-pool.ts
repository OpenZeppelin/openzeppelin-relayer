/**
 * Worker Pool Manager
 *
 * Manages a Piscina worker pool for executing compiled plugins.
 * Provides a high-level interface for running plugins with proper lifecycle management.
 *
 * ## Performance Tuning
 *
 * ### Memory Configuration
 * Each worker thread runs with an increased heap size (512MB via resourceLimits) to prevent
 * OOM errors under high load. Node.js v8 defaults to ~96MB heap per worker, which is
 * insufficient when handling many concurrent plugin contexts (e.g., 2500+ VUs).
 *
 * Symptoms of insufficient heap:
 * - "FATAL ERROR: CALL_AND_RETRY_LAST Allocation failed - JavaScript heap out of memory"
 * - Workers crash during v8::internal::ContextDeserializer::Deserialize
 * - Queue builds up (>50% capacity warnings) then all requests fail
 *
 * ### Thread Scaling
 * - Worker threads scale dynamically based on PLUGIN_MAX_CONCURRENCY
 * - Formula: max(cpuCount * 2, concurrency / 50, 8) with 64 thread cap
 * - Each worker handles multiple concurrent tasks via Node.js async I/O
 */

import Piscina from 'piscina';
import * as path from 'node:path';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as v8 from 'node:v8';
import { v4 as uuidv4 } from 'uuid';
import { compilePlugin, compilePluginSource, type CompilationResult } from './compiler';
import type { SandboxTask, SandboxResult, LogEntry } from './sandbox-executor';
import type { PluginHeaders } from './plugin';
import {
  DEFAULT_POOL_MIN_THREADS,
  DEFAULT_POOL_IDLE_TIMEOUT_MS,
  DEFAULT_POOL_EXECUTION_TIMEOUT_MS,
  DEFAULT_POOL_MAX_THREADS_FLOOR,
  DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER,
} from './constants';

/**
 * Worker pool configuration options
 */
export interface WorkerPoolOptions {
  /** Minimum number of worker threads to maintain */
  minThreads?: number;
  /** Maximum number of worker threads */
  maxThreads?: number;
  /** Number of concurrent tasks per worker */
  concurrentTasksPerWorker?: number;
  /** Idle timeout before shutting down excess workers (ms) */
  idleTimeout?: number;
  /** Task-level timeout to prevent stuck workers (ms). Defaults to execution timeout + 5s buffer. */
  taskTimeout?: number;
}

/**
 * Plugin execution request
 */
export interface PluginExecutionRequest {
  /** Plugin ID */
  pluginId: string;
  /** Path to plugin source file (for on-demand compilation) */
  pluginPath?: string;
  /** Pre-compiled JavaScript code (if already compiled) */
  compiledCode?: string;
  /** Plugin parameters */
  params: any;
  /** HTTP headers from incoming request */
  headers?: PluginHeaders;
  /** Unix socket path for relayer communication */
  socketPath: string;
  /** HTTP request ID for tracing */
  httpRequestId?: string;
  /** Execution timeout in milliseconds */
  timeout?: number;
}

/**
 * Plugin execution result
 */
export interface PluginExecutionResult {
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

const DEFAULT_TIMEOUT = DEFAULT_POOL_EXECUTION_TIMEOUT_MS;

// Task timeout includes a 5s buffer over execution timeout for cleanup overhead
const DEFAULT_TASK_TIMEOUT = DEFAULT_TIMEOUT + 5000;

const DEFAULT_OPTIONS: Required<WorkerPoolOptions> = {
  minThreads: DEFAULT_POOL_MIN_THREADS,
  maxThreads: Math.max(os.cpus().length, DEFAULT_POOL_MAX_THREADS_FLOOR),
  concurrentTasksPerWorker: DEFAULT_POOL_CONCURRENT_TASKS_PER_WORKER,
  idleTimeout: DEFAULT_POOL_IDLE_TIMEOUT_MS,
  taskTimeout: DEFAULT_TASK_TIMEOUT,
};

/**
 * Path to the pre-compiled sandbox executor.
 * This file is generated at build time by running: npx ts-node build-executor.ts
 */
const PRECOMPILED_EXECUTOR_PATH = path.resolve(__dirname, 'sandbox-executor.js');

/**
 * Metrics tracking for plugin execution
 */
interface PluginMetrics {
  // Execution metrics
  totalExecutions: number;
  successfulExecutions: number;
  failedExecutions: number;

  // Timing metrics (in ms)
  totalExecutionTime: number;
  minExecutionTime: number;
  maxExecutionTime: number;

  // Cache metrics
  cacheHits: number;
  cacheMisses: number;

  // Compilation metrics
  totalCompilations: number;
  totalCompilationTime: number;

  // Per-plugin execution counts
  pluginExecutions: Map<string, number>;

  // Error tracking
  errorsByType: Map<string, number>;

  // Timestamps
  startTime: number;
  lastExecutionTime: number | null;
}

/**
 * Create initial metrics state
 */
function createInitialMetrics(): PluginMetrics {
  return {
    totalExecutions: 0,
    successfulExecutions: 0,
    failedExecutions: 0,
    totalExecutionTime: 0,
    minExecutionTime: Infinity,
    maxExecutionTime: 0,
    cacheHits: 0,
    cacheMisses: 0,
    totalCompilations: 0,
    totalCompilationTime: 0,
    pluginExecutions: new Map(),
    errorsByType: new Map(),
    startTime: Date.now(),
    lastExecutionTime: null,
  };
}

/**
 * Increment a counter in a bounded Map.
 * If the map exceeds maxSize, removes the oldest entry (FIFO eviction).
 */
function incrementBoundedMap(
  map: Map<string, number>,
  key: string,
  maxSize: number
): void {
  map.set(key, (map.get(key) || 0) + 1);

  // Evict oldest entries if over limit
  if (map.size > maxSize) {
    const firstKey = map.keys().next().value;
    if (firstKey !== undefined) {
      map.delete(firstKey);
    }
  }
}

/**
 * In-memory cache for compiled plugin code
 * Includes memory pressure awareness for proactive eviction
 */
class CompiledCodeCache {
  private cache: Map<string, { code: string; timestamp: number; size: number }> = new Map();
  private maxAge: number;
  private cleanupInterval!: NodeJS.Timeout;
  private memoryCheckInterval!: NodeJS.Timeout;
  private totalCacheSize: number = 0;
  // Max cache size in bytes (100MB default - adjust under memory pressure)
  private maxCacheSize: number = 100 * 1024 * 1024;
  // Track last LRU access for smarter eviction
  private accessOrder: string[] = [];

  constructor(maxAgeMs: number = 3600000) {
    // 1 hour default
    this.maxAge = maxAgeMs;
    this.startCleanupTimer();
    this.startMemoryPressureMonitor();
  }

  /**
   * Start periodic cleanup timer to evict expired entries.
   * Runs every 5 minutes to prevent memory leaks.
   */
  private startCleanupTimer(): void {
    this.cleanupInterval = setInterval(() => {
      this.evictExpired();
    }, 300000); // 5 minutes
    // unref() allows process to exit even if timer is active
    this.cleanupInterval.unref();
  }

  /**
   * Start memory pressure monitoring - proactively evict cache under pressure.
   * This helps prevent GC issues under high load.
   */
  private startMemoryPressureMonitor(): void {
    this.memoryCheckInterval = setInterval(() => {
      this.checkMemoryPressure();
    }, 10000); // Every 10 seconds
    this.memoryCheckInterval.unref();
  }

  /**
   * Check memory pressure and evict cache entries proactively.
   */
  private checkMemoryPressure(): void {
    const usage = process.memoryUsage();
    const heapStats = v8.getHeapStatistics();
    // Use heap_size_limit (the actual max heap) instead of heapTotal (current allocated)
    // heapTotal grows dynamically and can be much smaller than the configured max,
    // causing false positive pressure detection (e.g., 28MB/30MB = 93% when max is 26GB)
    const heapUsedRatio = usage.heapUsed / heapStats.heap_size_limit;

    // At 70% heap usage, start evicting oldest entries
    if (heapUsedRatio >= 0.70) {
      const evictCount = Math.ceil(this.cache.size * 0.25); // Evict 25%
      if (evictCount > 0) {
        console.warn(
          `[cache] Memory pressure detected (${Math.round(heapUsedRatio * 100)}% of heap limit), ` +
          `evicting ${evictCount} oldest entries`
        );
        this.evictOldest(evictCount);
        
        // Force GC if available
        if (global.gc) {
          try {
            global.gc();
          } catch {
            // Ignore
          }
        }
      }
    }

    // At 85% heap usage, be more aggressive
    if (heapUsedRatio >= 0.85) {
      const evictCount = Math.ceil(this.cache.size * 0.5); // Evict 50%
      if (evictCount > 0) {
        console.error(
          `[cache] HIGH memory pressure (${Math.round(heapUsedRatio * 100)}% of heap limit), ` +
          `evicting ${evictCount} entries`
        );
        this.evictOldest(evictCount);
        
        // Force GC
        if (global.gc) {
          try {
            global.gc();
          } catch {
            // Ignore
          }
        }
      }
    }
  }

  /**
   * Evict all expired entries from the cache.
   */
  private evictExpired(): void {
    const now = Date.now();
    let evictedCount = 0;
    for (const [key, entry] of this.cache.entries()) {
      if (now - entry.timestamp > this.maxAge) {
        this.totalCacheSize -= entry.size;
        this.cache.delete(key);
        evictedCount++;
      }
    }
    if (evictedCount > 0) {
      console.log(`[cache] Evicted ${evictedCount} expired entries`);
    }
  }

  /**
   * Evict N oldest entries (LRU-style) - used under memory pressure.
   */
  private evictOldest(count: number): void {
    // Sort by timestamp (oldest first)
    const entries = [...this.cache.entries()].sort(
      (a, b) => a[1].timestamp - b[1].timestamp
    );

    let evicted = 0;
    for (const [key, entry] of entries) {
      if (evicted >= count) break;
      this.totalCacheSize -= entry.size;
      this.cache.delete(key);
      evicted++;
    }

    if (evicted > 0) {
      console.log(`[cache] Evicted ${evicted} oldest entries under memory pressure`);
    }
  }

  get(pluginPath: string): string | undefined {
    const entry = this.cache.get(pluginPath);
    if (!entry) return undefined;

    // Check if expired
    if (Date.now() - entry.timestamp > this.maxAge) {
      this.totalCacheSize -= entry.size;
      this.cache.delete(pluginPath);
      return undefined;
    }

    // Update timestamp for LRU tracking
    entry.timestamp = Date.now();

    return entry.code;
  }

  set(pluginPath: string, code: string): void {
    const size = Buffer.byteLength(code, 'utf8');
    
    // Evict if adding this would exceed max cache size
    while (this.totalCacheSize + size > this.maxCacheSize && this.cache.size > 0) {
      this.evictOldest(1);
    }

    const existing = this.cache.get(pluginPath);
    if (existing) {
      this.totalCacheSize -= existing.size;
    }

    this.cache.set(pluginPath, { code, timestamp: Date.now(), size });
    this.totalCacheSize += size;
  }

  delete(pluginPath: string): boolean {
    const entry = this.cache.get(pluginPath);
    if (entry) {
      this.totalCacheSize -= entry.size;
    }
    return this.cache.delete(pluginPath);
  }

  clear(): void {
    this.cache.clear();
    this.totalCacheSize = 0;
  }

  has(pluginPath: string): boolean {
    return this.get(pluginPath) !== undefined;
  }

  /**
   * Get current cache stats.
   */
  stats(): { entries: number; totalSizeBytes: number; maxSizeBytes: number } {
    return {
      entries: this.cache.size,
      totalSizeBytes: this.totalCacheSize,
      maxSizeBytes: this.maxCacheSize,
    };
  }

  /**
   * Destroy the cache and stop cleanup timer.
   */
  destroy(): void {
    if (this.cleanupInterval) {
      clearInterval(this.cleanupInterval);
    }
    if (this.memoryCheckInterval) {
      clearInterval(this.memoryCheckInterval);
    }
    this.clear();
  }
}

/**
 * Worker Pool Manager
 *
 * Manages plugin execution using a Piscina worker pool.
 * Handles compilation caching, task routing, and pool lifecycle.
 */
/** Maximum entries in metrics Maps to prevent unbounded growth */
const MAX_METRICS_ENTRIES = 1000;

export class WorkerPoolManager {
  private pool: Piscina | null = null;
  private options: Required<WorkerPoolOptions>;
  private compiledCache: CompiledCodeCache;
  private initialized: boolean = false;
  /** Promise for in-flight initialization to prevent race conditions */
  private initPromise: Promise<void> | null = null;
  private compiledWorkerPath: string | null = null;
  /** Whether compiledWorkerPath is a temp file that should be cleaned up */
  private isTemporaryWorkerFile: boolean = false;
  private metrics: PluginMetrics;

  constructor(options: WorkerPoolOptions = {}) {
    this.options = { ...DEFAULT_OPTIONS, ...options };
    this.compiledCache = new CompiledCodeCache();
    this.metrics = createInitialMetrics();

    // Register cleanup handlers for unclean shutdown (temp file leak prevention)
    this.registerCleanupHandlers();
  }

  /**
   * Register process handlers to clean up temp files on unexpected exit.
   */
  private registerCleanupHandlers(): void {
    const cleanup = () => {
      if (this.isTemporaryWorkerFile && this.compiledWorkerPath) {
        try {
          fs.unlinkSync(this.compiledWorkerPath);
        } catch {
          // Ignore - best effort cleanup
        }
      }
    };

    // Handle various exit scenarios
    process.once('beforeExit', cleanup);
    process.once('SIGINT', cleanup);
    process.once('SIGTERM', cleanup);
  }

  /**
   * Initialize the worker pool.
   * Call this before executing any plugins.
   * 
   * Uses pre-compiled sandbox-executor.js if available,
   * otherwise compiles it on-the-fly (slower first startup).
   * 
   * Thread-safe: multiple concurrent calls will await the same initialization.
   */
  async initialize(): Promise<void> {
    // Already initialized
    if (this.initialized) return;

    // Another call is initializing - await it (prevents race condition)
    if (this.initPromise) {
      await this.initPromise;
      return;
    }

    // Start initialization and store promise for concurrent callers
    this.initPromise = this.doInitialize();

    try {
      await this.initPromise;
    } finally {
      // Clear promise after completion (success or failure)
      this.initPromise = null;
    }
  }

  /**
   * Internal initialization logic.
   */
  private async doInitialize(): Promise<void> {
    // Use pre-compiled sandbox-executor.js if it exists
    if (fs.existsSync(PRECOMPILED_EXECUTOR_PATH)) {
      this.compiledWorkerPath = PRECOMPILED_EXECUTOR_PATH;
      this.isTemporaryWorkerFile = false;
    } else {
      // Fallback: compile on-the-fly (for fresh checkouts, dev mode, etc.)
      console.warn(
        `[pool] Pre-compiled sandbox executor not found at ${PRECOMPILED_EXECUTOR_PATH}. ` +
        `Compiling on-the-fly. Run 'npm run build:executor' in plugins/ for faster startup.`
      );
      this.compiledWorkerPath = await this.compileExecutorOnTheFly();
      this.isTemporaryWorkerFile = true;
    }

    // Worker heap size from Rust config (based on concurrent tasks per worker)
    // Formula: 512 + (concurrent_tasks * 8) MB - ensures enough heap for burst context creation
    // Default to 2048MB if not passed (conservative for high load)
    const workerHeapMb = parseInt(process.env.PLUGIN_WORKER_HEAP_MB || '2048', 10);
    
    console.error(`[worker-pool] Worker heap size: ${workerHeapMb}MB (per worker thread)`);

    this.pool = new Piscina({
      filename: this.compiledWorkerPath,
      minThreads: this.options.minThreads,
      maxThreads: this.options.maxThreads,
      concurrentTasksPerWorker: this.options.concurrentTasksPerWorker,
      idleTimeout: this.options.idleTimeout,
      recordTiming: true,  // Unless you need waitTime/runTime metrics
      // Worker heap size scaled by concurrent tasks per worker
      // Each vm.createContext() uses ~4-8MB, plus GC headroom
      // Set by Rust via PLUGIN_WORKER_HEAP_MB env var
      resourceLimits: {
        maxOldGenerationSizeMb: workerHeapMb,
      },
    });

    // Track worker crashes for self-healing monitoring
    let workerCrashCount = 0;
    let lastCrashReset = Date.now();
    const CRASH_WINDOW_MS = 60000; // 1 minute window for crash rate monitoring

    // Listen to worker events for monitoring
    this.pool.on('error', (err) => {
      console.error('[worker-pool] Worker error (will be replaced):', err.message);
      // Track errors that indicate memory issues
      const errMsg = err.message.toLowerCase();
      if (errMsg.includes('heap') || errMsg.includes('memory') || errMsg.includes('allocation')) {
        console.error('[worker-pool] Memory-related worker error detected - consider increasing worker heap size');
      }
      // Don't throw - Piscina handles recovery
    });

    this.pool.on('workerExit', (workerId: number, exitCode: number) => {
      // Reset crash counter periodically
      const now = Date.now();
      if (now - lastCrashReset > CRASH_WINDOW_MS) {
        workerCrashCount = 0;
        lastCrashReset = now;
      }

      if (exitCode !== 0) {
        workerCrashCount++;
        console.error(
          `[worker-pool] Worker ${workerId} exited with code ${exitCode} ` +
          `(replaced automatically, crashes in window: ${workerCrashCount})`
        );

        // Detect crash storms (many workers dying = likely OOM or GC issue)
        if (workerCrashCount >= 3) {
          console.error(
            `[worker-pool] WARNING: ${workerCrashCount} worker crashes in ${CRASH_WINDOW_MS}ms - ` +
            `possible memory pressure. Consider reducing PLUGIN_MAX_CONCURRENCY or increasing heap.`
          );
        }

        // Exit codes that indicate memory issues
        if (exitCode === 134 || exitCode === 137 || exitCode === 9) {
          console.error(
            `[worker-pool] Worker ${workerId} killed with signal (code ${exitCode}) - ` +
            `likely OOM. Clearing script cache as mitigation.`
          );
          // Clear cache as mitigation
          this.compiledCache.clear();
        }
      }
    });

    this.initialized = true;
  }

  /**
   * Compile the sandbox executor on-the-fly when pre-compiled version is not available.
   * This is slower but ensures the pool can start in any environment.
   */
  private async compileExecutorOnTheFly(): Promise<string> {
    const sandboxExecutorPath = path.resolve(__dirname, 'sandbox-executor.ts');

    const esbuild = await import('esbuild');
    const result = await esbuild.build({
      entryPoints: [sandboxExecutorPath],
      bundle: true,
      platform: 'node',
      target: 'node18',
      format: 'cjs',
      sourcemap: false,
      write: false,
      loader: { '.ts': 'ts' },
      external: ['node:*'],
    });

    // Write to temp file
    const tempPath = path.join(os.tmpdir(), `sandbox-executor-${uuidv4()}.js`);
    fs.writeFileSync(tempPath, result.outputFiles[0].text);
    return tempPath;
  }

  /**
   * Pre-compile a plugin and cache the result.
   *
   * @param pluginPath - Path to the plugin source file
   * @param pluginId - Optional plugin identifier for caching
   * @returns Compilation result
   */
  async precompilePlugin(pluginPath: string, pluginId?: string): Promise<CompilationResult> {
    const startTime = Date.now();
    const result = await compilePlugin(pluginPath);
    const compilationTime = Date.now() - startTime;

    this.metrics.totalCompilations++;
    this.metrics.totalCompilationTime += compilationTime;

    // Cache by pluginId if provided, otherwise by path (single key to avoid duplication)
    const cacheKey = pluginId || pluginPath;
    this.compiledCache.set(cacheKey, result.code);
    return result;
  }

  /**
   * Pre-compile a plugin from source code and cache the result.
   *
   * @param pluginId - Plugin identifier for caching
   * @param sourceCode - TypeScript source code
   * @returns Compilation result
   */
  async precompilePluginSource(
    pluginId: string,
    sourceCode: string
  ): Promise<CompilationResult> {
    const startTime = Date.now();
    const result = await compilePluginSource(sourceCode, `${pluginId}.ts`);
    const compilationTime = Date.now() - startTime;

    this.metrics.totalCompilations++;
    this.metrics.totalCompilationTime += compilationTime;

    this.compiledCache.set(pluginId, result.code);
    return result;
  }

  /**
   * Store pre-compiled code in the cache.
   * Useful when compiled code comes from external storage (Redis).
   *
   * @param pluginId - Plugin identifier
   * @param compiledCode - Pre-compiled JavaScript code
   */
  cacheCompiledCode(pluginId: string, compiledCode: string): void {
    this.compiledCache.set(pluginId, compiledCode);
  }

  /**
   * Get compiled code from cache.
   *
   * @param pluginId - Plugin identifier
   * @returns Compiled code or undefined if not cached
   */
  getCachedCode(pluginId: string): string | undefined {
    return this.compiledCache.get(pluginId);
  }

  /**
   * Execute a plugin in a worker.
   *
   * @param request - Plugin execution request
   * @returns Plugin execution result
   */
  async runPlugin(request: PluginExecutionRequest): Promise<PluginExecutionResult> {
    if (!this.initialized || !this.pool) {
      await this.initialize();
    }

    const executionStartTime = Date.now();
    let cacheHit = false;

    // Get compiled code (from request, cache, or compile on-demand)
    let compiledCode = request.compiledCode;

    if (!compiledCode && request.pluginPath) {
      // Try cache first
      compiledCode = this.compiledCache.get(request.pluginPath);

      if (compiledCode) {
        cacheHit = true;
        this.metrics.cacheHits++;
      } else {
        // Compile on-demand
        this.metrics.cacheMisses++;
        const compileStartTime = Date.now();
        const result = await compilePlugin(request.pluginPath);
        this.metrics.totalCompilations++;
        this.metrics.totalCompilationTime += Date.now() - compileStartTime;
        compiledCode = result.code;
        this.compiledCache.set(request.pluginPath, compiledCode);
      }
    }

    if (!compiledCode) {
      // Try by plugin ID
      compiledCode = this.compiledCache.get(request.pluginId);
      if (compiledCode) {
        cacheHit = true;
        this.metrics.cacheHits++;
      } else {
        this.metrics.cacheMisses++;
      }
    }

    if (!compiledCode) {
      this.metrics.totalExecutions++;
      this.metrics.failedExecutions++;
      const errorCode = 'NO_COMPILED_CODE';
      incrementBoundedMap(this.metrics.errorsByType, errorCode, MAX_METRICS_ENTRIES);
      return {
        success: false,
        error: {
          message: `No compiled code available for plugin ${request.pluginId}`,
          code: errorCode,
          status: 500,
        },
        logs: [],
      };
    }

    // Create task for the worker
    const task: SandboxTask = {
      taskId: uuidv4(),
      pluginId: request.pluginId,
      compiledCode,
      params: request.params,
      headers: request.headers,
      socketPath: request.socketPath,
      httpRequestId: request.httpRequestId,
      timeout: request.timeout ?? DEFAULT_TIMEOUT,
    };

    // Track per-plugin execution (bounded to prevent memory leak)
    incrementBoundedMap(this.metrics.pluginExecutions, request.pluginId, MAX_METRICS_ENTRIES);

    // Use task timeout to prevent permanently stuck workers
    // This is a safety net beyond the handler-level timeout in sandbox-executor
    const taskTimeout = this.options.taskTimeout;
    let timeoutId: NodeJS.Timeout | undefined;

    try {
      const runPromise = this.pool!.run(task);

      const timeoutPromise = new Promise<never>((_, reject) => {
        timeoutId = setTimeout(() => {
          reject(new Error(`Task timed out after ${taskTimeout}ms (worker may be stuck)`));
        }, taskTimeout);
      });

      const result: SandboxResult = await Promise.race([runPromise, timeoutPromise]);

      // Update execution metrics
      const executionTime = Date.now() - executionStartTime;
      this.metrics.totalExecutions++;
      this.metrics.lastExecutionTime = Date.now();
      this.metrics.totalExecutionTime += executionTime;
      this.metrics.minExecutionTime = Math.min(this.metrics.minExecutionTime, executionTime);
      this.metrics.maxExecutionTime = Math.max(this.metrics.maxExecutionTime, executionTime);

      if (result.success) {
        this.metrics.successfulExecutions++;
      } else {
        this.metrics.failedExecutions++;
        if (result.error?.code) {
          incrementBoundedMap(this.metrics.errorsByType, result.error.code, MAX_METRICS_ENTRIES);
        }
      }

      return {
        success: result.success,
        result: result.result,
        error: result.error,
        logs: result.logs,
      };
    } catch (error) {
      const err = error as Error;

      // Update execution metrics for error case
      const executionTime = Date.now() - executionStartTime;
      this.metrics.totalExecutions++;
      this.metrics.failedExecutions++;
      this.metrics.lastExecutionTime = Date.now();
      this.metrics.totalExecutionTime += executionTime;
      this.metrics.minExecutionTime = Math.min(this.metrics.minExecutionTime, executionTime);
      this.metrics.maxExecutionTime = Math.max(this.metrics.maxExecutionTime, executionTime);
      incrementBoundedMap(this.metrics.errorsByType, 'WORKER_ERROR', MAX_METRICS_ENTRIES);

      return {
        success: false,
        error: {
          message: err.message || String(error),
          code: 'WORKER_ERROR',
          status: 500,
        },
        logs: [],
      };
    } finally {
      // Clear timeout to prevent timer leak
      if (timeoutId) {
        clearTimeout(timeoutId);
      }
    }
  }

  /**
   * Clear the compiled code cache.
   */
  clearCache(): void {
    this.compiledCache.clear();
  }

  /**
   * Invalidate a specific plugin from the cache.
   * Removes entries by both pluginId and any associated path.
   *
   * @param pluginId - Plugin identifier to invalidate
   * @param pluginPath - Optional path to also invalidate
   */
  invalidatePlugin(pluginId: string, pluginPath?: string): void {
    this.compiledCache.delete(pluginId);
    if (pluginPath) {
      this.compiledCache.delete(pluginPath);
    }
  }

  /**
   * Get pool statistics including execution metrics.
   */
  getStats(): {
    pool: {
      completed: number;
      queued: number;
      runTime: { average: number; max: number; min: number };
      waitTime: { average: number; max: number; min: number };
    } | null;
    execution: {
      total: number;
      successful: number;
      failed: number;
      successRate: number;
      avgExecutionTime: number;
      minExecutionTime: number;
      maxExecutionTime: number;
    };
    cache: {
      hits: number;
      misses: number;
      hitRate: number;
    };
    compilation: {
      total: number;
      totalTime: number;
      avgTime: number;
    };
    plugins: Record<string, number>;
    errors: Record<string, number>;
    uptime: number;
  } {
    const poolStats = this.pool ? {
      completed: this.pool.completed,
      queued: this.pool.queueSize,
      runTime: {
        average: this.pool.runTime.average,
        max: this.pool.runTime.max,
        min: this.pool.runTime.min,
      },
      waitTime: {
        average: this.pool.waitTime.average,
        max: this.pool.waitTime.max,
        min: this.pool.waitTime.min,
      },
    } : null;

    const totalCacheAccesses = this.metrics.cacheHits + this.metrics.cacheMisses;

    return {
      pool: poolStats,
      execution: {
        total: this.metrics.totalExecutions,
        successful: this.metrics.successfulExecutions,
        failed: this.metrics.failedExecutions,
        successRate: this.metrics.totalExecutions > 0
          ? this.metrics.successfulExecutions / this.metrics.totalExecutions
          : 1,
        avgExecutionTime: this.metrics.totalExecutions > 0
          ? this.metrics.totalExecutionTime / this.metrics.totalExecutions
          : 0,
        minExecutionTime: this.metrics.minExecutionTime === Infinity
          ? 0
          : this.metrics.minExecutionTime,
        maxExecutionTime: this.metrics.maxExecutionTime,
      },
      cache: {
        hits: this.metrics.cacheHits,
        misses: this.metrics.cacheMisses,
        hitRate: totalCacheAccesses > 0
          ? this.metrics.cacheHits / totalCacheAccesses
          : 0,
      },
      compilation: {
        total: this.metrics.totalCompilations,
        totalTime: this.metrics.totalCompilationTime,
        avgTime: this.metrics.totalCompilations > 0
          ? this.metrics.totalCompilationTime / this.metrics.totalCompilations
          : 0,
      },
      plugins: Object.fromEntries(this.metrics.pluginExecutions),
      errors: Object.fromEntries(this.metrics.errorsByType),
      uptime: Date.now() - this.metrics.startTime,
    };
  }

  /**
   * Reset metrics to initial state.
   * Useful for testing or periodic resets.
   */
  resetMetrics(): void {
    this.metrics = createInitialMetrics();
  }

  /**
   * Shutdown the worker pool.
   * Call this when the application is shutting down.
   */
  async shutdown(): Promise<void> {
    if (this.pool) {
      await this.pool.destroy();
      this.pool = null;
      this.initialized = false;
    }

    // Clean up temporary compiled worker file to prevent disk space leak
    if (this.isTemporaryWorkerFile && this.compiledWorkerPath) {
      try {
        fs.unlinkSync(this.compiledWorkerPath);
      } catch {
        // Ignore errors - file may already be deleted or inaccessible
      }
    }

    this.compiledWorkerPath = null;
    this.isTemporaryWorkerFile = false;
    // Properly destroy cache (clears timer + entries)
    this.compiledCache.destroy();
  }
}

// Singleton instance for convenience
let defaultPool: WorkerPoolManager | null = null;

/**
 * Get or create the default worker pool instance.
 */
export function getDefaultPool(options?: WorkerPoolOptions): WorkerPoolManager {
  if (!defaultPool) {
    defaultPool = new WorkerPoolManager(options);
  }
  return defaultPool;
}

/**
 * Shutdown the default worker pool.
 */
export async function shutdownDefaultPool(): Promise<void> {
  if (defaultPool) {
    await defaultPool.shutdown();
    defaultPool = null;
  }
}
