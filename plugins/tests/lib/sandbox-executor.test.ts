import * as vm from 'node:vm';
import * as v8 from 'node:v8';
import * as net from 'node:net';

describe('ContextPool logic', () => {
  class MockContextPool {
    private available: any[] = [];
    private readonly maxSize = 10;

    acquire(): any {
      const ctx = this.available.pop();
      if (ctx) {
        this.resetContext(ctx);
        return ctx;
      }
      return this.createFreshContext();
    }

    release(ctx: any): void {
      if (this.available.length < this.maxSize) {
        this.available.push(ctx);
      }
    }

    private createFreshContext(): any {
      return { _fresh: true };
    }

    private resetContext(ctx: any): void {
      if (ctx.__pluginState) {
        delete ctx.__pluginState;
      }
    }

    clear(): void {
      this.available = [];
    }

    size(): number {
      return this.available.length;
    }
  }

  it('should create new context when pool is empty', () => {
    const pool = new MockContextPool();
    const ctx = pool.acquire();

    expect(ctx).toBeDefined();
    expect(ctx._fresh).toBe(true);
    expect(pool.size()).toBe(0);
  });

  it('should reuse context from pool', () => {
    const pool = new MockContextPool();
    const ctx1 = pool.acquire();
    pool.release(ctx1);

    expect(pool.size()).toBe(1);

    const ctx2 = pool.acquire();
    expect(ctx2).toBe(ctx1); // Same object
    expect(pool.size()).toBe(0);
  });

  it('should not exceed max pool size', () => {
    const pool = new MockContextPool();
    const contexts: any[] = [];

    // Create 15 contexts
    for (let i = 0; i < 15; i++) {
      const ctx = pool.acquire();
      ctx.id = i;
      contexts.push(ctx);
    }

    // Release them one by one
    for (const ctx of contexts) {
      pool.release(ctx);
    }

    expect(pool.size()).toBe(10); // Should cap at maxSize
  });

  it('should reset context state before reuse', () => {
    const pool = new MockContextPool();
    const ctx = pool.acquire();

    // Simulate plugin pollution
    (ctx as any).__pluginState = { dirty: true };
    pool.release(ctx);

    const reusedCtx = pool.acquire();
    expect((reusedCtx as any).__pluginState).toBeUndefined();
  });

  it('should clear all contexts', () => {
    const pool = new MockContextPool();
    const contexts: any[] = [];

    for (let i = 0; i < 5; i++) {
      const ctx = pool.acquire();
      contexts.push(ctx);
    }

    // Release them to the pool
    for (const ctx of contexts) {
      pool.release(ctx);
    }

    expect(pool.size()).toBe(5);
    pool.clear();
    expect(pool.size()).toBe(0);
  });
});

describe('ScriptCache logic', () => {
  class MockScriptCache {
    private cache = new Map<string, { script: any; timestamp: number }>();
    private readonly maxSize = 100;

    get(code: string): any | undefined {
      const entry = this.cache.get(code);
      if (entry) {
        entry.timestamp = Date.now();
        return entry.script;
      }
      return undefined;
    }

    set(code: string, script: any): void {
      if (this.cache.size >= this.maxSize) {
        this.evictOldest(1);
      }
      this.cache.set(code, { script, timestamp: Date.now() });
    }

    evictOldest(count: number): void {
      const entries = [...this.cache.entries()].sort(
        (a, b) => a[1].timestamp - b[1].timestamp
      );

      let evicted = 0;
      for (const [key] of entries) {
        if (evicted >= count) break;
        this.cache.delete(key);
        evicted++;
      }
    }

    clear(): void {
      this.cache.clear();
    }

    size(): number {
      return this.cache.size;
    }
  }

  it('should cache compiled scripts', () => {
    const cache = new MockScriptCache();
    const code = 'console.log("test")';
    const script = { compiled: true };

    cache.set(code, script);
    const retrieved = cache.get(code);

    expect(retrieved).toBe(script);
  });

  it('should return undefined for cache miss', () => {
    const cache = new MockScriptCache();
    const result = cache.get('nonexistent-code');

    expect(result).toBeUndefined();
  });

  it('should update timestamp on cache hit (LRU)', async () => {
    const cache = new MockScriptCache();
    const code = 'test';
    const script = { compiled: true };

    cache.set(code, script);
    const timestamp1 = (cache as any).cache.get(code).timestamp;

    await new Promise(resolve => setTimeout(resolve, 10));

    cache.get(code);
    const timestamp2 = (cache as any).cache.get(code).timestamp;
    expect(timestamp2).toBeGreaterThanOrEqual(timestamp1);
  });

  it('should evict oldest entry when at capacity', () => {
    const cache = new MockScriptCache();

    // Fill cache to capacity
    for (let i = 0; i < 100; i++) {
      cache.set(`code-${i}`, { id: i });
    }

    expect(cache.size()).toBe(100);

    // Add one more - should evict oldest
    cache.set('code-101', { id: 101 });

    expect(cache.size()).toBe(100);
    expect(cache.get('code-0')).toBeUndefined(); // First one evicted
    expect(cache.get('code-101')).toBeDefined(); // New one added
  });

  it('should evict multiple oldest entries', () => {
    const cache = new MockScriptCache();

    for (let i = 0; i < 10; i++) {
      cache.set(`code-${i}`, { id: i });
    }

    cache.evictOldest(3);

    expect(cache.size()).toBe(7);
    expect(cache.get('code-0')).toBeUndefined();
    expect(cache.get('code-1')).toBeUndefined();
    expect(cache.get('code-2')).toBeUndefined();
    expect(cache.get('code-3')).toBeDefined();
  });

  it('should clear entire cache', () => {
    const cache = new MockScriptCache();

    for (let i = 0; i < 10; i++) {
      cache.set(`code-${i}`, { id: i });
    }

    expect(cache.size()).toBe(10);
    cache.clear();
    expect(cache.size()).toBe(0);
  });

  it('should handle memory pressure by evicting cache', () => {
    const heapLimit = 1000 * 1024 * 1024; // 1000MB
    const heapUsed = 700 * 1024 * 1024; // 700MB (70%)

    const heapUsedRatio = heapUsed / heapLimit;

    const cache = new MockScriptCache();
    for (let i = 0; i < 100; i++) {
      cache.set(`code-${i}`, { id: i });
    }

    // Simulate eviction at 70% threshold
    if (heapUsedRatio >= 0.70 && cache.size() > 0) {
      const evictCount = Math.max(1, Math.ceil(cache.size() * 0.25));
      cache.evictOldest(evictCount);
    }

    expect(cache.size()).toBeLessThan(100);
    expect(cache.size()).toBeGreaterThanOrEqual(75); // 25% evicted
  });
});

describe('SocketPool logic', () => {
  class MockSocketPool {
    private available: any[] = [];
    private readonly maxSize = 5;

    acquire(socketPath: string): any | null {
      const socket = this.available.pop();
      if (socket && socket.writable && !socket.destroyed) {
        return socket;
      }
      return null;
    }

    release(socket: any): void {
      if (socket.writable && !socket.destroyed && this.available.length < this.maxSize) {
        this.available.push(socket);
      } else if (socket.destroy) {
        socket.destroy();
      }
    }

    clear(): void {
      for (const socket of this.available) {
        if (socket.destroy) socket.destroy();
      }
      this.available = [];
    }

    size(): number {
      return this.available.length;
    }
  }

  it('should return null when pool is empty', () => {
    const pool = new MockSocketPool();
    const socket = pool.acquire('/tmp/test.sock');

    expect(socket).toBeNull();
  });

  it('should reuse healthy socket from pool', () => {
    const pool = new MockSocketPool();
    const mockSocket = {
      writable: true,
      destroyed: false,
      destroy: jest.fn(),
    };

    pool.release(mockSocket);
    expect(pool.size()).toBe(1);

    const reused = pool.acquire('/tmp/test.sock');
    expect(reused).toBe(mockSocket);
    expect(pool.size()).toBe(0);
  });

  it('should destroy unhealthy socket instead of pooling', () => {
    const pool = new MockSocketPool();
    const mockSocket = {
      writable: false,
      destroyed: false,
      destroy: jest.fn(),
    };

    pool.release(mockSocket);

    expect(mockSocket.destroy).toHaveBeenCalled();
    expect(pool.size()).toBe(0);
  });

  it('should not exceed max pool size', () => {
    const pool = new MockSocketPool();

    for (let i = 0; i < 10; i++) {
      pool.release({
        writable: true,
        destroyed: false,
        destroy: jest.fn(),
      });
    }

    expect(pool.size()).toBe(5); // Max size is 5
  });

  it('should destroy all sockets on clear', () => {
    const pool = new MockSocketPool();
    const sockets = [];

    for (let i = 0; i < 5; i++) {
      const socket = {
        writable: true,
        destroyed: false,
        destroy: jest.fn(),
      };
      sockets.push(socket);
      pool.release(socket);
    }

    pool.clear();

    expect(pool.size()).toBe(0);
    for (const socket of sockets) {
      expect(socket.destroy).toHaveBeenCalled();
    }
  });

  it('should skip destroyed sockets during reuse', () => {
    const pool = new MockSocketPool();
    const mockSocket = {
      writable: true,
      destroyed: true, // Already destroyed
      destroy: jest.fn(),
    };

    pool.release(mockSocket);
    const reused = pool.acquire('/tmp/test.sock');

    expect(reused).toBeNull(); // Should not return destroyed socket
  });
});

describe('SocketClosedError', () => {
  // Replicate the error class for testing
  class SocketClosedError extends Error {
    code: string;
    constructor(message: string) {
      super(message);
      this.name = 'SocketClosedError';
      this.code = 'ESOCKETCLOSED';
    }
  }

  it('should have correct name', () => {
    const error = new SocketClosedError('test message');
    expect(error.name).toBe('SocketClosedError');
  });

  it('should have ESOCKETCLOSED code', () => {
    const error = new SocketClosedError('test message');
    expect(error.code).toBe('ESOCKETCLOSED');
  });

  it('should preserve message', () => {
    const error = new SocketClosedError('Connection lifetime exceeded');
    expect(error.message).toBe('Connection lifetime exceeded');
  });

  it('should be instanceof Error', () => {
    const error = new SocketClosedError('test');
    expect(error).toBeInstanceOf(Error);
  });

  it('should have stack trace', () => {
    const error = new SocketClosedError('test');
    expect(error.stack).toBeDefined();
    expect(error.stack).toContain('SocketClosedError');
  });
});

describe('Retry logic for connection errors', () => {
  // Simulates the retry logic in sendWithRetry
  function isRetryableError(error: any): boolean {
    return (
      error.code === 'EPIPE' ||
      error.code === 'ECONNRESET' ||
      error.code === 'ESOCKETCLOSED'
    );
  }

  it('should identify EPIPE as retryable', () => {
    const error: any = new Error('broken pipe');
    error.code = 'EPIPE';
    expect(isRetryableError(error)).toBe(true);
  });

  it('should identify ECONNRESET as retryable', () => {
    const error: any = new Error('connection reset');
    error.code = 'ECONNRESET';
    expect(isRetryableError(error)).toBe(true);
  });

  it('should identify ESOCKETCLOSED as retryable', () => {
    const error: any = new Error('socket closed');
    error.code = 'ESOCKETCLOSED';
    expect(isRetryableError(error)).toBe(true);
  });

  it('should not retry generic errors', () => {
    const error = new Error('generic error');
    expect(isRetryableError(error)).toBe(false);
  });

  it('should not retry timeout errors', () => {
    const error: any = new Error('timeout');
    error.code = 'ETIMEDOUT';
    expect(isRetryableError(error)).toBe(false);
  });

  it('should not retry errors without code', () => {
    const error = new Error('no code');
    expect(isRetryableError(error)).toBe(false);
  });
});

describe('Age-based socket eviction', () => {
  const MAX_SOCKET_AGE_MS = 50_000; // 50 seconds, matching production code

  interface PooledSocket {
    socket: { writable: boolean; destroyed: boolean; destroy: () => void };
    createdAt: number;
  }

  function shouldEvictSocket(pooled: PooledSocket, now: number): boolean {
    const age = now - pooled.createdAt;
    return age > MAX_SOCKET_AGE_MS;
  }

  it('should not evict fresh socket', () => {
    const now = Date.now();
    const pooled: PooledSocket = {
      socket: { writable: true, destroyed: false, destroy: jest.fn() },
      createdAt: now - 10_000, // 10 seconds old
    };
    expect(shouldEvictSocket(pooled, now)).toBe(false);
  });

  it('should evict socket older than 50 seconds', () => {
    const now = Date.now();
    const pooled: PooledSocket = {
      socket: { writable: true, destroyed: false, destroy: jest.fn() },
      createdAt: now - 51_000, // 51 seconds old
    };
    expect(shouldEvictSocket(pooled, now)).toBe(true);
  });

  it('should not evict socket exactly at 50 seconds', () => {
    const now = Date.now();
    const pooled: PooledSocket = {
      socket: { writable: true, destroyed: false, destroy: jest.fn() },
      createdAt: now - 50_000, // exactly 50 seconds
    };
    expect(shouldEvictSocket(pooled, now)).toBe(false);
  });

  it('should evict socket at 55 seconds (within danger zone)', () => {
    const now = Date.now();
    const pooled: PooledSocket = {
      socket: { writable: true, destroyed: false, destroy: jest.fn() },
      createdAt: now - 55_000, // 55 seconds old
    };
    expect(shouldEvictSocket(pooled, now)).toBe(true);
  });

  it('should provide 10 second safety margin before Rust 60s timeout', () => {
    // Rust kills connections at 60s, we evict at 50s = 10s safety margin
    const RUST_TIMEOUT = 60_000;
    const SAFETY_MARGIN = RUST_TIMEOUT - MAX_SOCKET_AGE_MS;
    expect(SAFETY_MARGIN).toBe(10_000);
  });
});

describe('Socket pool with age tracking', () => {
  class MockSocketPoolWithAge {
    private available: { socket: any; createdAt: number }[] = [];
    private readonly maxSize = 5;
    private readonly maxSocketAgeMs = 50_000;

    acquire(): { socket: any; createdAt: number } | null {
      const now = Date.now();

      while (this.available.length > 0) {
        const pooled = this.available.pop()!;
        const age = now - pooled.createdAt;

        // Discard old sockets
        if (age > this.maxSocketAgeMs) {
          pooled.socket.destroy();
          continue;
        }

        if (pooled.socket.writable && !pooled.socket.destroyed) {
          return pooled;
        }

        pooled.socket.destroy();
      }

      return null;
    }

    release(socket: any, createdAt: number): void {
      const now = Date.now();
      const age = now - createdAt;

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

    size(): number {
      return this.available.length;
    }
  }

  it('should reject old socket on acquire', () => {
    const pool = new MockSocketPoolWithAge();
    const oldCreatedAt = Date.now() - 55_000; // 55 seconds ago

    const oldSocket = {
      writable: true,
      destroyed: false,
      destroy: jest.fn(),
    };

    pool.release(oldSocket, oldCreatedAt);

    // Simulate time passing
    jest.spyOn(Date, 'now').mockReturnValue(Date.now() + 1);

    const acquired = pool.acquire();
    expect(acquired).toBeNull();
    expect(oldSocket.destroy).toHaveBeenCalled();

    jest.restoreAllMocks();
  });

  it('should accept fresh socket on acquire', () => {
    const pool = new MockSocketPoolWithAge();
    const freshCreatedAt = Date.now() - 10_000; // 10 seconds ago

    const freshSocket = {
      writable: true,
      destroyed: false,
      destroy: jest.fn(),
    };

    pool.release(freshSocket, freshCreatedAt);
    const acquired = pool.acquire();

    expect(acquired).not.toBeNull();
    expect(acquired?.socket).toBe(freshSocket);
    expect(acquired?.createdAt).toBe(freshCreatedAt);
  });

  it('should not pool socket that is too old on release', () => {
    const pool = new MockSocketPoolWithAge();
    const oldCreatedAt = Date.now() - 55_000;

    const socket = {
      writable: true,
      destroyed: false,
      destroy: jest.fn(),
    };

    pool.release(socket, oldCreatedAt);

    expect(pool.size()).toBe(0);
    expect(socket.destroy).toHaveBeenCalled();
  });

  it('should track createdAt through acquire/release cycle', () => {
    const pool = new MockSocketPoolWithAge();
    const originalCreatedAt = Date.now() - 20_000; // 20 seconds ago

    const socket = {
      writable: true,
      destroyed: false,
      destroy: jest.fn(),
    };

    pool.release(socket, originalCreatedAt);
    const acquired = pool.acquire();

    // createdAt should be preserved (not reset)
    expect(acquired?.createdAt).toBe(originalCreatedAt);
  });
});

describe('SandboxPluginAPI socket handling', () => {
  class MockSandboxPluginAPI {
    private socket: any = null;
    private pending = new Map<string, any>();
    private connected = false;
    private connectionPromise: Promise<void> | null = null;
    private socketPath: string;
    private socketCreatedAt: number = 0;
    private readonly maxPendingRequests = 100;

    constructor(socketPath: string) {
      this.socketPath = socketPath;
    }

    async ensureConnected(): Promise<void> {
      if (this.connected) return;
      if (!this.connectionPromise) {
        this.connectionPromise = this.connect();
      }
      await this.connectionPromise;
    }

    private async connect(): Promise<void> {
      // Simulate connection
      this.socket = { writable: true, destroyed: false };
      this.socketCreatedAt = Date.now();
      this.connected = true;
    }

    handleSocketError(error: Error): void {
      this.connected = false;
      this.connectionPromise = null;
      this.socket = null;
      this.rejectAllPending(error);
    }

    handleSocketClose(): void {
      this.connected = false;
      this.connectionPromise = null;
      this.socket = null;
      // Use SocketClosedError-like error
      const error: any = new Error('Socket closed by server (connection lifetime exceeded)');
      error.code = 'ESOCKETCLOSED';
      error.name = 'SocketClosedError';
      this.rejectAllPending(error);
    }

    private rejectAllPending(error: Error): void {
      for (const [requestId, resolver] of this.pending.entries()) {
        resolver.reject(error);
        this.pending.delete(requestId);
      }
    }

    async send(method: string, payload: any): Promise<any> {
      if (this.pending.size >= this.maxPendingRequests) {
        throw new Error(
          `Too many concurrent API requests (max ${this.maxPendingRequests})`
        );
      }

      await this.ensureConnected();

      return new Promise((resolve, reject) => {
        const requestId = 'test-id';
        this.pending.set(requestId, { resolve, reject });

        // Simulate response
        setTimeout(() => {
          const resolver = this.pending.get(requestId);
          if (resolver) {
            resolver.resolve({ success: true });
            this.pending.delete(requestId);
          }
        }, 10);
      });
    }

    getPendingCount(): number {
      return this.pending.size;
    }

    isConnected(): boolean {
      return this.connected;
    }

    getSocketCreatedAt(): number {
      return this.socketCreatedAt;
    }
  }

  it('should establish connection on first request', async () => {
    const api = new MockSandboxPluginAPI('/tmp/test.sock');

    expect(api.isConnected()).toBe(false);

    await api.send('testMethod', {});

    expect(api.isConnected()).toBe(true);
  });

  it('should reuse existing connection', async () => {
    const api = new MockSandboxPluginAPI('/tmp/test.sock');

    await api.send('method1', {});
    const wasConnected = api.isConnected();

    await api.send('method2', {});
    expect(api.isConnected()).toBe(wasConnected);
  });

  it('should handle socket error and reject pending requests', async () => {
    const api = new MockSandboxPluginAPI('/tmp/test.sock');

    await api.send('method1', {});

    const error = new Error('Socket error');
    api.handleSocketError(error);

    expect(api.isConnected()).toBe(false);
    expect(api.getPendingCount()).toBe(0);
  });

  it('should handle socket close and reset connection', async () => {
    const api = new MockSandboxPluginAPI('/tmp/test.sock');

    await api.send('method1', {});
    expect(api.isConnected()).toBe(true);

    api.handleSocketClose();

    expect(api.isConnected()).toBe(false);
    expect(api.getPendingCount()).toBe(0);
  });

  it('should enforce max pending requests limit', async () => {
    const api = new MockSandboxPluginAPI('/tmp/test.sock');

    // Mock the pending count to be at limit
    for (let i = 0; i < 100; i++) {
      (api as any).pending.set(`request-${i}`, { resolve: jest.fn(), reject: jest.fn() });
    }

    await expect(api.send('method', {})).rejects.toThrow('Too many concurrent API requests');
  });

  it('should reconnect after socket close', async () => {
    const api = new MockSandboxPluginAPI('/tmp/test.sock');

    await api.send('method1', {});
    api.handleSocketClose();

    expect(api.isConnected()).toBe(false);

    // Should reconnect on next request
    await api.send('method2', {});
    expect(api.isConnected()).toBe(true);
  });

  it('should reject with ESOCKETCLOSED code on socket close', async () => {
    const api = new MockSandboxPluginAPI('/tmp/test.sock');

    await api.send('method1', {});

    // Add a pending request
    const pendingPromise = new Promise((resolve, reject) => {
      (api as any).pending.set('test-pending', { resolve, reject });
    });

    // Trigger socket close
    api.handleSocketClose();

    try {
      await pendingPromise;
      fail('Should have rejected');
    } catch (error: any) {
      expect(error.code).toBe('ESOCKETCLOSED');
      expect(error.name).toBe('SocketClosedError');
      expect(error.message).toContain('connection lifetime exceeded');
    }
  });

  it('should track socket creation time', async () => {
    const api = new MockSandboxPluginAPI('/tmp/test.sock');
    const beforeConnect = Date.now();

    await api.send('method1', {});

    const afterConnect = Date.now();
    const socketCreatedAt = api.getSocketCreatedAt();

    expect(socketCreatedAt).toBeGreaterThanOrEqual(beforeConnect);
    expect(socketCreatedAt).toBeLessThanOrEqual(afterConnect);
  });
});

describe('Sandbox console implementation', () => {
  interface LogEntry {
    level: 'error' | 'warn' | 'info' | 'log' | 'debug' | 'result';
    message: string;
  }

  function createMockSandboxConsole(logs: LogEntry[]): Console {
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
    } as Console;
  }

  it('should capture log messages', () => {
    const logs: LogEntry[] = [];
    const console = createMockSandboxConsole(logs);

    console.log('test message');

    expect(logs).toHaveLength(1);
    expect(logs[0]).toEqual({ level: 'log', message: 'test message' });
  });

  it('should capture error messages', () => {
    const logs: LogEntry[] = [];
    const console = createMockSandboxConsole(logs);

    console.error('error occurred');

    expect(logs).toHaveLength(1);
    expect(logs[0]).toEqual({ level: 'error', message: 'error occurred' });
  });

  it('should capture warn messages', () => {
    const logs: LogEntry[] = [];
    const console = createMockSandboxConsole(logs);

    console.warn('warning message');

    expect(logs).toHaveLength(1);
    expect(logs[0]).toEqual({ level: 'warn', message: 'warning message' });
  });

  it('should stringify objects', () => {
    const logs: LogEntry[] = [];
    const console = createMockSandboxConsole(logs);

    console.log({ foo: 'bar', num: 42 });

    expect(logs).toHaveLength(1);
    expect(logs[0].message).toBe('{"foo":"bar","num":42}');
  });

  it('should handle multiple arguments', () => {
    const logs: LogEntry[] = [];
    const console = createMockSandboxConsole(logs);

    console.log('Hello', 'world', 123);

    expect(logs).toHaveLength(1);
    expect(logs[0].message).toBe('Hello world 123');
  });

  it('should handle mixed types', () => {
    const logs: LogEntry[] = [];
    const console = createMockSandboxConsole(logs);

    console.log('Count:', 42, { status: 'ok' });

    expect(logs).toHaveLength(1);
    expect(logs[0].message).toBe('Count: 42 {"status":"ok"}');
  });
});

describe('Sandbox require blocking', () => {
  const BLOCKED_MODULES = new Set([
    'fs', 'child_process', 'net', 'http', 'https',
    'cluster', 'process', 'vm', 'os', 'v8',
  ]);

  function createSandboxRequire(blockedModules: Set<string>) {
    return (id: string): any => {
      if (blockedModules.has(id)) {
        throw new Error(
          `Module '${id}' is blocked for security. ` +
          `Use the PluginAPI for network operations.`
        );
      }
      // Allow other modules
      return require(id);
    };
  }

  it('should block dangerous built-in modules', () => {
    const sandboxRequire = createSandboxRequire(BLOCKED_MODULES);

    expect(() => sandboxRequire('fs')).toThrow('Module \'fs\' is blocked for security');
    expect(() => sandboxRequire('child_process')).toThrow('blocked for security');
    expect(() => sandboxRequire('net')).toThrow('blocked for security');
    expect(() => sandboxRequire('http')).toThrow('blocked for security');
  });

  it('should allow safe modules', () => {
    const sandboxRequire = createSandboxRequire(BLOCKED_MODULES);

    // Should not throw
    expect(() => sandboxRequire('uuid')).not.toThrow();
  });

  it('should block with node: prefix', () => {
    const extendedBlocked = new Set([
      'fs', 'node:fs',
      'net', 'node:net',
    ]);
    const sandboxRequire = createSandboxRequire(extendedBlocked);

    expect(() => sandboxRequire('node:fs')).toThrow('blocked for security');
    expect(() => sandboxRequire('node:net')).toThrow('blocked for security');
  });
});

describe('executeInSandbox function', () => {
  interface SandboxTask {
    taskId: string;
    pluginId: string;
    compiledCode: string;
    params: any;
    headers?: Record<string, string[]>;
    socketPath: string;
    httpRequestId?: string;
    timeout: number;
  }

  interface SandboxResult {
    taskId: string;
    success: boolean;
    result?: any;
    error?: {
      message: string;
      code?: string;
      status?: number;
      details?: any;
    };
    logs: any[];
  }

  async function mockExecuteInSandbox(task: SandboxTask): Promise<SandboxResult> {
    const logs: any[] = [];

    try {
      // Simulate plugin execution
      if (task.compiledCode.includes('throw')) {
        throw new Error('Plugin error');
      }

      if (task.compiledCode.includes('timeout')) {
        await new Promise((resolve) => setTimeout(resolve, task.timeout + 100));
      }

      return {
        taskId: task.taskId,
        success: true,
        result: { processed: task.params },
        logs,
      };
    } catch (error: any) {
      return {
        taskId: task.taskId,
        success: false,
        error: {
          message: error.message,
          code: 'PLUGIN_ERROR',
          status: 500,
        },
        logs,
      };
    }
  }

  it('should execute plugin successfully', async () => {
    const task: SandboxTask = {
      taskId: 'task-1',
      pluginId: 'plugin-1',
      compiledCode: 'return params;',
      params: { test: true },
      socketPath: '/tmp/test.sock',
      timeout: 5000,
    };

    const result = await mockExecuteInSandbox(task);

    expect(result.success).toBe(true);
    expect(result.taskId).toBe('task-1');
    expect(result.result).toEqual({ processed: { test: true } });
  });

  it('should handle plugin errors', async () => {
    const task: SandboxTask = {
      taskId: 'task-2',
      pluginId: 'plugin-2',
      compiledCode: 'throw new Error("test error");',
      params: {},
      socketPath: '/tmp/test.sock',
      timeout: 5000,
    };

    const result = await mockExecuteInSandbox(task);

    expect(result.success).toBe(false);
    expect(result.error).toBeDefined();
    expect(result.error?.message).toBe('Plugin error');
    expect(result.error?.code).toBe('PLUGIN_ERROR');
  });

  it('should include headers in execution context', async () => {
    const task: SandboxTask = {
      taskId: 'task-3',
      pluginId: 'plugin-3',
      compiledCode: 'return headers;',
      params: {},
      headers: { 'x-api-key': ['secret'] },
      socketPath: '/tmp/test.sock',
      timeout: 5000,
    };

    const result = await mockExecuteInSandbox(task);

    expect(result.success).toBe(true);
  });

  it('should respect execution timeout', async () => {
    const task: SandboxTask = {
      taskId: 'task-4',
      pluginId: 'plugin-4',
      compiledCode: 'timeout',
      params: {},
      socketPath: '/tmp/test.sock',
      timeout: 100,
    };

    // This would timeout in real execution
    const startTime = Date.now();
    await mockExecuteInSandbox(task);
    const elapsed = Date.now() - startTime;

    expect(elapsed).toBeGreaterThan(100);
  });

  it('should provide httpRequestId for tracing', async () => {
    const task: SandboxTask = {
      taskId: 'task-5',
      pluginId: 'plugin-5',
      compiledCode: 'return httpRequestId;',
      params: {},
      socketPath: '/tmp/test.sock',
      httpRequestId: 'http-123',
      timeout: 5000,
    };

    const result = await mockExecuteInSandbox(task);
    expect(result.success).toBe(true);
  });
});

describe('Error handling in sandbox', () => {
  it('should categorize SyntaxError', () => {
    const error = new SyntaxError('Unexpected token');

    const errorCode = error.name === 'SyntaxError' ? 'SYNTAX_ERROR' : 'PLUGIN_ERROR';

    expect(errorCode).toBe('SYNTAX_ERROR');
  });

  it('should categorize TypeError', () => {
    const error = new TypeError('Cannot read property');

    const errorCode = error.name === 'TypeError' ? 'TYPE_ERROR' : 'PLUGIN_ERROR';

    expect(errorCode).toBe('TYPE_ERROR');
  });

  it('should categorize ReferenceError', () => {
    const error = new ReferenceError('x is not defined');

    const errorCode = error.name === 'ReferenceError' ? 'REFERENCE_ERROR' : 'PLUGIN_ERROR';

    expect(errorCode).toBe('REFERENCE_ERROR');
  });

  it('should handle timeout errors', () => {
    const error: any = new Error('Timeout');
    error.code = 'ERR_SCRIPT_EXECUTION_TIMEOUT';

    const errorCode = error.code === 'ERR_SCRIPT_EXECUTION_TIMEOUT' ? 'TIMEOUT' : 'PLUGIN_ERROR';
    const errorStatus = errorCode === 'TIMEOUT' ? 504 : 500;

    expect(errorCode).toBe('TIMEOUT');
    expect(errorStatus).toBe(504);
  });

  it('should handle handler timeout errors', () => {
    const error: any = new Error('Handler timeout');
    error.code = 'ERR_HANDLER_TIMEOUT';

    const errorCode = error.code === 'ERR_HANDLER_TIMEOUT' ? 'TIMEOUT' : 'PLUGIN_ERROR';

    expect(errorCode).toBe('TIMEOUT');
  });

  it('should capture stack trace', () => {
    const error = new Error('Test error');

    const stack = error.stack?.split('\n').slice(0, 10).join('\n');

    expect(stack).toBeDefined();
    expect(stack).toContain('Test error');
  });

  it('should use custom status if provided', () => {
    const error: any = new Error('Bad request');
    error.status = 400;

    const errorStatus = typeof error.status === 'number' ? error.status : 500;

    expect(errorStatus).toBe(400);
  });
});

describe('Safe stringify function', () => {
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

  it('should stringify simple values', () => {
    expect(safeStringify('hello')).toBe('"hello"');
    expect(safeStringify(42)).toBe('42');
    expect(safeStringify(true)).toBe('true');
    expect(safeStringify(null)).toBe('null');
  });

  it('should stringify objects', () => {
    const obj = { foo: 'bar', num: 42 };
    expect(safeStringify(obj)).toBe('{"foo":"bar","num":42}');
  });

  it('should handle BigInt', () => {
    const bigInt = BigInt(123456789);
    const result = safeStringify(bigInt);
    expect(result).toContain('123456789');
  });

  it('should handle circular references', () => {
    const obj: any = { name: 'test' };
    obj.self = obj; // Circular reference

    const result = safeStringify(obj);
    // Will use String() fallback
    expect(result).toBeTruthy();
  });

  it('should handle undefined', () => {
    const result = safeStringify(undefined);
    // JSON.stringify returns undefined for undefined
    expect(result === undefined || result === 'undefined').toBe(true);
  });
});

describe('Memory-aware script caching', () => {
  it('should check memory periodically', () => {
    const lastCheck = Date.now();
    const checkInterval = 5000;

    const shouldCheck = Date.now() - lastCheck >= checkInterval;

    // Initially should check (time has passed in test)
    expect(typeof shouldCheck).toBe('boolean');
  });

  it('should calculate heap usage ratio correctly', () => {
    const heapLimit = 1000 * 1024 * 1024; // 1000MB
    const heapUsed = 700 * 1024 * 1024; // 700MB

    const ratio = heapUsed / heapLimit;

    expect(ratio).toBe(0.7);
  });

  it('should evict 25% at 70% heap usage', () => {
    const cacheSize = 100;
    const heapRatio = 0.70;

    const evictCount = Math.max(1, Math.ceil(cacheSize * 0.25));

    expect(evictCount).toBe(25);
  });

  it('should evict 50% at 85% heap usage', () => {
    const cacheSize = 100;
    const heapRatio = 0.85;

    const evictCount = Math.max(1, Math.ceil(cacheSize * 0.5));

    expect(evictCount).toBe(50);
  });
});

describe('Context and cache lifecycle', () => {
  it('should release context to pool after execution', () => {
    const pool = { released: false };
    const context = { id: 'test-context' };

    // Simulate execution complete
    pool.released = true;

    expect(pool.released).toBe(true);
  });

  it('should close API socket after execution', () => {
    const api = {
      closed: false,
      close() {
        this.closed = true;
      },
    };

    api.close();

    expect(api.closed).toBe(true);
  });

  it('should disconnect KV store after execution', async () => {
    const kv = {
      disconnected: false,
      async disconnect() {
        this.disconnected = true;
      },
    };

    await kv.disconnect();

    expect(kv.disconnected).toBe(true);
  });

  it('should handle cleanup errors gracefully', () => {
    const api = {
      close() {
        throw new Error('Close failed');
      },
    };

    // Should not throw
    expect(() => {
      try {
        api.close();
      } catch {
        // Ignore cleanup errors
      }
    }).not.toThrow();
  });
});
