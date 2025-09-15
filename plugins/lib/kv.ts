import IORedis, { Redis } from 'ioredis';
import { randomUUID } from 'crypto';

export interface PluginKVStore {
  connect(): Promise<void>;
  disconnect(): Promise<void>;

  get<T = unknown>(key: string): Promise<T | null>;
  set(key: string, value: unknown, opts?: { ttlSec?: number }): Promise<boolean>;
  del(key: string): Promise<boolean>;
  exists(key: string): Promise<boolean>;

  scan(pattern?: string, batch?: number): Promise<string[]>;
  clear(): Promise<number>;

  withLock<T>(
    key: string,
    fn: () => Promise<T>,
    opts?: { ttlSec?: number; onBusy?: 'throw' | 'skip' }
  ): Promise<T | null>;
}

export class DefaultPluginKVStore implements PluginKVStore {
  private client: Redis;
  private ns: string;
  private readonly KEY_REGEX = /^[A-Za-z0-9:_-]{1,512}$/;
  private readonly UNLOCK_SCRIPT =
    'if redis.call("GET", KEYS[1]) == ARGV[1] then return redis.call("UNLINK", KEYS[1]) else return 0 end';

  constructor(pluginId: string) {
    const url = process.env.REDIS_URL ?? 'redis://localhost:6379';

    this.client = new IORedis(url, {
      connectionName: `plugin_kv:${pluginId}`,
      lazyConnect: true,
      enableOfflineQueue: false,
      enableAutoPipelining: true,
      maxRetriesPerRequest: 1,
      retryStrategy: (n) => Math.min(n * 50, 1000),
    });

    const pid = pluginId.replace(/[{}]/g, '');
    this.ns = `plugin_kv:{${pid}}`;
  }

  async connect(): Promise<void> {
    await this.client.connect();
  }

  async disconnect(): Promise<void> {
    try {
      await this.client.quit();
    } catch {
      this.client.disconnect();
    }
  }

  // Key builder
  private key(seg: 'data' | 'lock', key: string): string {
    if (!this.KEY_REGEX.test(key)) throw new Error('invalid key');
    return `${this.ns}:${seg}:${key}`;
  }

  async get<T = unknown>(key: string): Promise<T | null> {
    const v = await this.client.get(this.key('data', key));
    if (v == null) return null;
    return JSON.parse(v) as T;
  }

  async set(key: string, value: unknown, opts?: { ttlSec?: number }): Promise<boolean> {
    if (value === undefined) throw new Error('value must not be undefined');
    const payload = JSON.stringify(value);
    const k = this.key('data', key);
    const ex = Math.max(0, Math.floor(opts?.ttlSec ?? 0));
    const res = ex > 0 ? await this.client.set(k, payload, 'EX', ex) : await this.client.set(k, payload);
    return res === 'OK';
  }

  async del(key: string): Promise<boolean> {
    const k = this.key('data', key);
    const n = await this.client.unlink(k);
    return n === 1;
  }

  async exists(key: string): Promise<boolean> {
    return (await this.client.exists(this.key('data', key))) === 1;
  }

  async scan(pattern = '*', batch = 500): Promise<string[]> {
    const out: string[] = [];
    let cursor = '0';
    const prefix = `${this.ns}:data:`;
    do {
      const [next, keys] = await this.client.scan(cursor, 'MATCH', `${prefix}${pattern}`, 'COUNT', batch);
      cursor = next;
      for (const k of keys) out.push(k.slice(prefix.length));
    } while (cursor !== '0');
    return out;
  }

  async clear(): Promise<number> {
    let cursor = '0';
    let deleted = 0;
    do {
      const [next, keys] = await this.client.scan(cursor, 'MATCH', `${this.ns}:*`, 'COUNT', 1000);
      cursor = next;
      if (keys.length) {
        // Pipeline UNLINK in chunks to avoid huge argument lists
        const chunkSize = 512;
        for (let i = 0; i < keys.length; i += chunkSize) {
          const chunk = keys.slice(i, i + chunkSize);
          const pipeline = this.client.pipeline();
          chunk.forEach((k) => pipeline.unlink(k));
          const results = await pipeline.exec();
          if (results) {
            for (const [, res] of results) {
              if (typeof res === 'number') deleted += res;
            }
          }
        }
      }
    } while (cursor !== '0');
    return deleted;
  }

  async withLock<T>(
    key: string,
    fn: () => Promise<T>,
    opts?: { ttlSec?: number; onBusy?: 'throw' | 'skip' }
  ): Promise<T | null> {
    const ttlSec = opts?.ttlSec ?? 30;
    const onBusy = opts?.onBusy ?? 'throw';
    const lockKey = this.key('lock', key);
    const token = randomUUID();
    const ok = await this.client.set(lockKey, token, 'PX', ttlSec * 1000, 'NX');
    if (ok !== 'OK') {
      if (onBusy === 'skip') return null;
      throw new Error('lock busy');
    }

    try {
      return await fn();
    } finally {
      await this.client.eval(this.UNLOCK_SCRIPT, 1, lockKey, token);
    }
  }
}
