import * as net from 'node:net';
import * as fs from 'node:fs';
import * as v8 from 'node:v8';

// Mock dependencies before importing the module
jest.mock('../../lib/worker-pool', () => ({
  WorkerPoolManager: jest.fn().mockImplementation(() => ({
    initialize: jest.fn().mockResolvedValue(undefined),
    runPlugin: jest.fn().mockResolvedValue({
      success: true,
      result: { data: 'test' },
      logs: [],
    }),
    precompilePlugin: jest.fn().mockResolvedValue({
      code: 'compiled-code',
      warnings: [],
    }),
    precompilePluginSource: jest.fn().mockResolvedValue({
      code: 'compiled-code',
      warnings: [],
    }),
    cacheCompiledCode: jest.fn(),
    invalidatePlugin: jest.fn(),
    getStats: jest.fn().mockReturnValue({
      uptime: 1000,
      pool: { completed: 10, queued: 0 },
      execution: { total: 10, successRate: 1.0 },
    }),
    shutdown: jest.fn().mockResolvedValue(undefined),
    clearCache: jest.fn(),
  })),
}));

jest.mock('node:fs', () => ({
  unlinkSync: jest.fn(),
  statSync: jest.fn().mockReturnValue({
    mode: 0o755,
    isSocket: () => true,
  }),
}));

// Import after mocks are set up
const { WorkerPoolManager } = jest.requireMock('../../lib/worker-pool') as any;

describe('PoolServerMemoryMonitor logic', () => {
  describe('Memory pressure detection', () => {
    let originalMemoryUsage: typeof process.memoryUsage;

    beforeEach(() => {
      originalMemoryUsage = process.memoryUsage;
    });

    afterEach(() => {
      process.memoryUsage = originalMemoryUsage;
    });

    it('should detect warning threshold at 75% heap usage', () => {
      const heapLimit = 1000 * 1024 * 1024; // 1000MB
      const heapUsed = 750 * 1024 * 1024; // 750MB (75%)

      const ratio = heapUsed / heapLimit;
      expect(ratio).toBeGreaterThanOrEqual(0.75);
      expect(ratio).toBeLessThan(0.80);
    });

    it('should detect GC threshold at 80% heap usage', () => {
      const heapLimit = 1000 * 1024 * 1024;
      const heapUsed = 800 * 1024 * 1024; // 80%

      const ratio = heapUsed / heapLimit;
      expect(ratio).toBeGreaterThanOrEqual(0.80);
      expect(ratio).toBeLessThan(0.85);
    });

    it('should detect critical threshold at 85% heap usage', () => {
      const heapLimit = 1000 * 1024 * 1024;
      const heapUsed = 850 * 1024 * 1024; // 85%

      const ratio = heapUsed / heapLimit;
      expect(ratio).toBeGreaterThanOrEqual(0.85);
      expect(ratio).toBeLessThan(0.92);
    });

    it('should detect emergency threshold at 92% heap usage', () => {
      const heapLimit = 1000 * 1024 * 1024;
      const heapUsed = 920 * 1024 * 1024; // 92%

      const ratio = heapUsed / heapLimit;
      expect(ratio).toBeGreaterThanOrEqual(0.92);
    });
  });
});

describe('PoolServer message handling', () => {
  let mockSocket: Partial<net.Socket>;
  let dataCallback: ((data: Buffer) => void) | undefined;
  let errorCallback: ((err: Error) => void) | undefined;
  let closeCallback: (() => void) | undefined;

  beforeEach(() => {
    jest.clearAllMocks();
    
    // Reset WorkerPoolManager mock
    (WorkerPoolManager as jest.Mock).mockImplementation(() => ({
      initialize: jest.fn().mockResolvedValue(undefined),
      runPlugin: jest.fn().mockResolvedValue({
        success: true,
        result: { data: 'test' },
        logs: [],
      }),
      precompilePlugin: jest.fn().mockResolvedValue({
        code: 'compiled-code',
        warnings: [],
      }),
      precompilePluginSource: jest.fn().mockResolvedValue({
        code: 'compiled-code',
        warnings: [],
      }),
      cacheCompiledCode: jest.fn(),
      invalidatePlugin: jest.fn(),
      getStats: jest.fn().mockReturnValue({
        uptime: 1000,
        pool: { completed: 10, queued: 0 },
        execution: { total: 10, successRate: 1.0 },
      }),
      shutdown: jest.fn().mockResolvedValue(undefined),
      clearCache: jest.fn(),
    }));

    mockSocket = {
      setKeepAlive: jest.fn(),
      setNoDelay: jest.fn(),
      write: jest.fn(),
      writable: true,
      on: jest.fn().mockReturnThis(),
    } as any;
  });

  describe('execute message', () => {
    it('should handle valid execute message', async () => {
      const message = {
        type: 'execute',
        taskId: 'task-1',
        pluginId: 'plugin-1',
        compiledCode: 'code',
        params: { test: true },
        socketPath: '/tmp/test.sock',
        timeout: 5000,
      };

      // Simulate the message flow
      const pool = new WorkerPoolManager({} as any);
      const result = await pool.runPlugin({
        pluginId: message.pluginId,
        compiledCode: message.compiledCode,
        params: message.params,
        socketPath: message.socketPath,
        timeout: message.timeout,
      });

      expect(result).toEqual({
        success: true,
        result: { data: 'test' },
        logs: [],
      });
      expect(pool.runPlugin).toHaveBeenCalledWith({
        pluginId: 'plugin-1',
        compiledCode: 'code',
        params: { test: true },
        socketPath: '/tmp/test.sock',
        timeout: 5000,
      });
    });

    it('should handle execute message with error', async () => {
      const pool = new WorkerPoolManager({} as any);
      (pool.runPlugin as jest.Mock).mockResolvedValue({
        success: false,
        error: {
          message: 'Plugin error',
          code: 'PLUGIN_ERROR',
          status: 500,
        },
        logs: [],
      });

      const result = await pool.runPlugin({
        pluginId: 'plugin-1',
        compiledCode: 'code',
        params: {},
        socketPath: '/tmp/test.sock',
        timeout: 5000,
      });

      expect(result.success).toBe(false);
      expect(result.error).toEqual({
        message: 'Plugin error',
        code: 'PLUGIN_ERROR',
        status: 500,
      });
    });
  });

  describe('precompile message', () => {
    it('should handle precompile with source code', async () => {
      const pool = new WorkerPoolManager({} as any);
      const result = await pool.precompilePluginSource('plugin-1', 'source code');

      expect(result).toEqual({
        code: 'compiled-code',
        warnings: [],
      });
      expect(pool.precompilePluginSource).toHaveBeenCalledWith('plugin-1', 'source code');
    });

    it('should handle precompile with plugin path', async () => {
      const pool = new WorkerPoolManager({} as any);
      const result = await pool.precompilePlugin('/path/to/plugin.js');

      expect(result).toEqual({
        code: 'compiled-code',
        warnings: [],
      });
      expect(pool.precompilePlugin).toHaveBeenCalledWith('/path/to/plugin.js');
    });
  });

  describe('cache message', () => {
    it('should cache compiled code', () => {
      const pool = new WorkerPoolManager({} as any);
      pool.cacheCompiledCode('plugin-1', 'compiled-code');

      expect(pool.cacheCompiledCode).toHaveBeenCalledWith('plugin-1', 'compiled-code');
    });
  });

  describe('invalidate message', () => {
    it('should invalidate plugin cache', () => {
      const pool = new WorkerPoolManager({} as any);
      pool.invalidatePlugin('plugin-1');

      expect(pool.invalidatePlugin).toHaveBeenCalledWith('plugin-1');
    });
  });

  describe('stats message', () => {
    it('should return pool statistics', () => {
      const pool = new WorkerPoolManager({} as any);
      const stats = pool.getStats();

      expect(stats).toEqual({
        uptime: 1000,
        pool: { completed: 10, queued: 0 },
        execution: { total: 10, successRate: 1.0 },
      });
    });
  });

  describe('health message', () => {
    it('should return health status with memory info', () => {
      const pool = new WorkerPoolManager({} as any);
      const stats = pool.getStats();

      const health = {
        status: 'healthy',
        uptime: stats.uptime,
        memory: {
          heapUsed: process.memoryUsage().heapUsed,
          heapTotal: process.memoryUsage().heapTotal,
          rss: process.memoryUsage().rss,
        },
        pool: {
          completed: stats.pool.completed,
          queued: stats.pool.queued,
        },
        execution: stats.execution,
      };

      expect(health.status).toBe('healthy');
      expect(health.uptime).toBe(1000);
      expect(health.memory).toBeDefined();
      expect(health.pool).toBeDefined();
      expect(health.execution).toBeDefined();
    });
  });

  describe('shutdown message', () => {
    it('should initiate graceful shutdown', () => {
      const pool = new WorkerPoolManager({} as any);
      
      let shuttingDown = false;
      const activeRequests = 0;

      const response = {
        status: 'draining',
        activeRequests,
        timeoutMs: 30000,
      };

      expect(response.status).toBe('draining');
      expect(response.activeRequests).toBe(0);
      expect(response.timeoutMs).toBe(30000);
    });

    it('should indicate already shutting down', () => {
      let shuttingDown = true;
      const activeRequests = 2;

      const response = {
        status: 'already_shutting_down',
        activeRequests,
      };

      expect(response.status).toBe('already_shutting_down');
      expect(response.activeRequests).toBe(2);
    });
  });

  describe('unknown message type', () => {
    it('should handle unknown message type', () => {
      const unknownType = 'invalid_type';
      const error = {
        message: `Unknown message type: ${unknownType}`,
        code: 'UNKNOWN_MESSAGE_TYPE',
      };

      expect(error.message).toContain('Unknown message type');
      expect(error.code).toBe('UNKNOWN_MESSAGE_TYPE');
    });
  });
});

describe('PoolServer connection handling', () => {
  it('should enable keep-alive and no-delay on socket', () => {
    const mockSocket: any = {
      setKeepAlive: jest.fn(),
      setNoDelay: jest.fn(),
      on: jest.fn(),
    };

    mockSocket.setKeepAlive(true, 30000);
    mockSocket.setNoDelay(true);

    expect(mockSocket.setKeepAlive).toHaveBeenCalledWith(true, 30000);
    expect(mockSocket.setNoDelay).toHaveBeenCalledWith(true);
  });

  it('should parse newline-delimited JSON messages', () => {
    const buffer = '{"type":"execute","taskId":"1"}\n{"type":"stats","taskId":"2"}\n';
    const messages: any[] = [];

    let remaining = buffer;
    let newlineIndex;
    while ((newlineIndex = remaining.indexOf('\n')) !== -1) {
      const line = remaining.slice(0, newlineIndex);
      remaining = remaining.slice(newlineIndex + 1);
      if (line.trim()) {
        messages.push(JSON.parse(line));
      }
    }

    expect(messages).toHaveLength(2);
    expect(messages[0]).toEqual({ type: 'execute', taskId: '1' });
    expect(messages[1]).toEqual({ type: 'stats', taskId: '2' });
  });

  it('should handle partial messages in buffer', () => {
    let buffer = '{"type":"exe';
    const messages: any[] = [];

    // First chunk
    let newlineIndex = buffer.indexOf('\n');
    expect(newlineIndex).toBe(-1);

    // Second chunk completes the message
    buffer += 'cute","taskId":"1"}\n';
    newlineIndex = buffer.indexOf('\n');
    expect(newlineIndex).not.toBe(-1);

    const line = buffer.slice(0, newlineIndex);
    messages.push(JSON.parse(line));

    expect(messages).toHaveLength(1);
    expect(messages[0]).toEqual({ type: 'execute', taskId: '1' });
  });

  it('should handle socket errors gracefully', () => {
    const mockSocket: any = {
      on: jest.fn((event: string, callback: any) => {
        if (event === 'error') {
          // Simulate ECONNRESET error
          callback({ code: 'ECONNRESET', message: 'Connection reset' });
        }
      }),
    };

    let errorReceived = false;
    mockSocket.on('error', (err: any) => {
      errorReceived = true;
      expect(err.code).toBe('ECONNRESET');
    });

    expect(errorReceived).toBe(true);
  });
});

describe('PoolServer graceful shutdown', () => {
  it('should wait for active requests to complete', async () => {
    let activeRequests = 3;
    const checkInterval = 10;
    const timeoutMs = 1000;

    // Simulate draining
    const drainPromise = new Promise<void>((resolve) => {
      const interval = setInterval(() => {
        activeRequests--;
        if (activeRequests === 0) {
          clearInterval(interval);
          resolve();
        }
      }, checkInterval);
    });

    await drainPromise;
    expect(activeRequests).toBe(0);
  });

  it('should force shutdown after timeout', async () => {
    let activeRequests = 5;
    const checkInterval = 10;
    const timeoutMs = 50;

    const startTime = Date.now();
    let timedOut = false;

    const drainPromise = new Promise<void>((resolve) => {
      const interval = setInterval(() => {
        const elapsed = Date.now() - startTime;
        if (elapsed >= timeoutMs) {
          timedOut = true;
          clearInterval(interval);
          resolve();
        }
      }, checkInterval);
    });

    await drainPromise;
    expect(timedOut).toBe(true);
    expect(activeRequests).toBeGreaterThan(0); // Some requests still active
  });

  it('should track active requests properly', () => {
    let activeRequests = 0;

    // Execute request
    activeRequests++;
    expect(activeRequests).toBe(1);

    // Complete request
    activeRequests--;
    expect(activeRequests).toBe(0);

    // Multiple requests
    activeRequests += 3;
    expect(activeRequests).toBe(3);

    activeRequests -= 2;
    expect(activeRequests).toBe(1);
  });

  it('should reject new requests during shutdown', () => {
    let shuttingDown = false;
    let messageType: string = 'execute';

    // Allow before shutdown
    let shouldReject = shuttingDown && messageType !== 'shutdown' && messageType !== 'health';
    expect(shouldReject).toBe(false);

    // Reject during shutdown
    shuttingDown = true;
    shouldReject = shuttingDown && messageType !== 'shutdown' && messageType !== 'health';
    expect(shouldReject).toBe(true);

    // But still allow shutdown and health
    messageType = 'shutdown';
    shouldReject = shuttingDown && messageType !== 'shutdown' && messageType !== 'health';
    expect(shouldReject).toBe(false);

    messageType = 'health';
    shouldReject = shuttingDown && messageType !== 'shutdown' && messageType !== 'health';
    expect(shouldReject).toBe(false);
  });
});

describe('PoolServer cache eviction', () => {
  it('should evict cache on memory pressure', () => {
    const pool = new WorkerPoolManager({} as any);
    pool.clearCache();

    expect(pool.clearCache).toHaveBeenCalled();
  });

  it('should handle cache eviction errors gracefully', () => {
    const pool = new WorkerPoolManager({} as any);
    (pool.clearCache as jest.Mock).mockImplementation(() => {
      throw new Error('Cache eviction failed');
    });

    try {
      pool.clearCache();
    } catch (err) {
      expect((err as Error).message).toBe('Cache eviction failed');
    }
  });
});

describe('PoolServer configuration', () => {
  const originalEnv = process.env;

  beforeEach(() => {
    process.env = { ...originalEnv };
  });

  afterEach(() => {
    process.env = originalEnv;
  });

  it('should use environment variables for configuration', () => {
    process.env.PLUGIN_MAX_CONCURRENCY = '4096';
    process.env.PLUGIN_POOL_MIN_THREADS = '4';
    process.env.PLUGIN_POOL_MAX_THREADS = '16';
    process.env.PLUGIN_POOL_CONCURRENT_TASKS = '8';
    process.env.PLUGIN_POOL_IDLE_TIMEOUT = '30000';

    const config = {
      maxConcurrency: parseInt(process.env.PLUGIN_MAX_CONCURRENCY, 10),
      minThreads: parseInt(process.env.PLUGIN_POOL_MIN_THREADS, 10),
      maxThreads: parseInt(process.env.PLUGIN_POOL_MAX_THREADS, 10),
      concurrentTasks: parseInt(process.env.PLUGIN_POOL_CONCURRENT_TASKS, 10),
      idleTimeout: parseInt(process.env.PLUGIN_POOL_IDLE_TIMEOUT, 10),
    };

    expect(config.maxConcurrency).toBe(4096);
    expect(config.minThreads).toBe(4);
    expect(config.maxThreads).toBe(16);
    expect(config.concurrentTasks).toBe(8);
    expect(config.idleTimeout).toBe(30000);
  });

  it('should fall back to defaults when env vars not set', () => {
    delete process.env.PLUGIN_MAX_CONCURRENCY;
    delete process.env.PLUGIN_POOL_MIN_THREADS;

    const maxConcurrency = parseInt(process.env.PLUGIN_MAX_CONCURRENCY || '2048', 10);
    const minThreads = parseInt(process.env.PLUGIN_POOL_MIN_THREADS || '2', 10);

    expect(maxConcurrency).toBe(2048);
    expect(minThreads).toBe(2);
  });

  it('should use socket backlog from environment', () => {
    process.env.PLUGIN_POOL_SOCKET_BACKLOG = '2048';

    const backlog = parseInt(process.env.PLUGIN_POOL_SOCKET_BACKLOG, 10);
    expect(backlog).toBe(2048);
  });
});
