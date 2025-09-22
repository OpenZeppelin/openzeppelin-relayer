/**
 * Key-value storage for plugins backed by Redis.
 *
 * This module exposes the `PluginKVStore` interface and a default
 * implementation `DefaultPluginKVStore` backed by Redis.
 *
 * Keys are validated against the pattern /^[A-Za-z0-9:_-]{1,512}$/.
 * Values are JSON-serialized and deserialized transparently.
 * All keys are namespaced per plugin and use a Redis hash-tag to co-locate
 * data in the same cluster slot.
 *
 * Features:
 * - get/set with optional TTL
 * - del/exists
 * - scan with pattern and batching
 * - clear namespace efficiently using pipelined UNLINK
 * - withLock: distributed mutex using SET NX PX and token-verified unlock
 *
 * Configuration:
 * - Uses REDIS_URL if set, otherwise defaults to redis://localhost:6379
 */
import IORedis, { Redis } from 'ioredis';
import { randomUUID } from 'crypto';

/**
 * Minimal async key-value store used by plugins.
 */
export interface PluginKVStore {
  /**
   * Get and JSON-parse a value by key.
   * @typeParam T - Expected value type after JSON parse.
   * @param key - The key to retrieve.
   * @returns Resolves to the parsed value, or null if missing.
   */
  get<T = unknown>(key: string): Promise<T | null>;
  /**
   * Set a JSON-encoded value.
   * @param key - The key to set.
   * @param value - Serializable value; must not be undefined.
   * @param opts - Optional settings.
   * @param opts.ttlSec - Time-to-live in seconds; if > 0, sets expiry.
   * @returns True on success.
   * @throws Error if `value` is undefined or the key is invalid.
   */
  set(key: string, value: unknown, opts?: { ttlSec?: number }): Promise<boolean>;
  /**
   * Delete a key.
   * @param key - The key to remove.
   * @returns True if exactly one key was removed.
   */
  del(key: string): Promise<boolean>;
  /**
   * Check whether a key exists.
   * @param key - The key to check.
   * @returns True if the key exists.
   */
  exists(key: string): Promise<boolean>;

  /**
   * List keys in this namespace matching `pattern`.
   * @param pattern - Glob-like match pattern (default '*').
   * @param batch - SCAN COUNT per iteration (default 500).
   * @returns Array of bare keys (without the namespace prefix).
   */
  listKeys(pattern?: string, batch?: number): Promise<string[]>;
  /**
   * Remove all keys in this namespace.
   * @returns The number of keys deleted.
   */
  clear(): Promise<number>;

  /**
   * Execute `fn` under a distributed lock for `key`.
   * @typeParam T - The return type of `fn`.
   * @param key - The lock key.
   * @param fn - Async function to execute while holding the lock.
   * @param opts - Lock options.
   * @param opts.ttlSec - Lock expiry in seconds (default 30).
   * @param opts.onBusy - Behavior when the lock is busy: 'throw' or 'skip'.
   * @returns The result of `fn`, or null when skipped due to a busy lock.
   * @throws Error if the lock is busy and `onBusy` is 'throw'.
   */
  withLock<T>(
    key: string,
    fn: () => Promise<T>,
    opts?: { ttlSec?: number; onBusy?: 'throw' | 'skip' }
  ): Promise<T | null>;
}

/**
 * Default Redis-backed implementation of `PluginKVStore`.
 *
 * Create one instance per plugin. The `pluginId` is used to namespace
 * keys and to set a descriptive Redis connection name.
 */
export class DefaultPluginKVStore implements PluginKVStore {
  private client: Redis;
  private ns: string;
  private readonly KEY_REGEX = /^[A-Za-z0-9:_-]{1,512}$/;
  private readonly UNLOCK_SCRIPT =
    'if redis.call("GET", KEYS[1]) == ARGV[1] then return redis.call("UNLINK", KEYS[1]) else return 0 end';

  /**
   * Create a store bound to a plugin namespace.
   * - pluginId: used for the namespace and Redis connection name.
   *   Curly braces are stripped to avoid nested hash-tags.
   * - Uses REDIS_URL if provided; enables lazyConnect and auto pipelining.
   */
  constructor(pluginId: string) {
    const url = process.env.REDIS_URL ?? 'redis://localhost:6379';

    this.client = new IORedis(url, {
      connectionName: `plugin_kv:${pluginId}`,
      lazyConnect: true,
      enableOfflineQueue: true,
      enableAutoPipelining: true,
      maxRetriesPerRequest: 1,
      retryStrategy: (n) => Math.min(n * 50, 1000),
    });

    const pid = pluginId.replace(/[{}]/g, '');
    this.ns = `plugin_kv:{${pid}}`;
  }

  /**
   * Build a namespaced Redis key for the given segment and user key.
   * Validates the user-provided key and throws on invalid input.
   * @param seg - The key segment: 'data' or 'lock'.
   * @param key - The user-provided key to namespace.
   * @returns The fully-qualified Redis key.
   * @throws Error if the key does not match the allowed pattern.
   */
  private key(seg: 'data' | 'lock', key: string): string {
    if (!this.KEY_REGEX.test(key)) throw new Error('invalid key');
    return `${this.ns}:${seg}:${key}`;
  }

  /**
   * Retrieve and JSON-parse the value for a key.
   * Returns null if the key is not present.
   * @typeParam T - Expected value type after JSON parse.
   * @param key - The key to retrieve.
   * @returns Resolves to the parsed value, or null if missing.
   */
  async get<T = unknown>(key: string): Promise<T | null> {
    const v = await this.client.get(this.key('data', key));
    if (v == null) return null;
    return JSON.parse(v) as T;
  }

  /**
   * Store a JSON-encoded value under the key.
   * - If opts.ttlSec > 0, sets an expiry in seconds; otherwise no expiry.
   * - Throws if value is undefined.
   * Returns true on success.
   * @param key - The key to set.
   * @param value - Serializable value; must not be undefined.
   * @param opts - Optional settings.
   * @param opts.ttlSec - Time-to-live in seconds; if > 0, sets expiry.
   * @returns True on success.
   * @throws Error if `value` is undefined or the key is invalid.
   */
  async set(key: string, value: unknown, opts?: { ttlSec?: number }): Promise<boolean> {
    if (value === undefined) throw new Error('value must not be undefined');
    const payload = JSON.stringify(value);
    const k = this.key('data', key);
    const ex = Math.max(0, Math.floor(opts?.ttlSec ?? 0));
    const res = ex > 0 ? await this.client.set(k, payload, 'EX', ex) : await this.client.set(k, payload);
    return res === 'OK';
  }

  /**
   * Remove the key.
   * @param key - The key to remove.
   * @returns True if exactly one key was unlinked.
   */
  async del(key: string): Promise<boolean> {
    const k = this.key('data', key);
    const n = await this.client.unlink(k);
    return n === 1;
  }

  /**
   * Check whether a key exists.
   * @param key - The key to check.
   * @returns True if the key exists.
   */
  async exists(key: string): Promise<boolean> {
    return (await this.client.exists(this.key('data', key))) === 1;
  }

  /**
   * List keys in this store matching `pattern` (glob-like, default '*').
   * Returns bare keys (without namespace). Uses SCAN with COUNT=`batch`.
   * @param pattern - Glob-like match pattern (default '*').
   * @param batch - SCAN COUNT per iteration (default 500).
   * @returns Array of bare keys (without the namespace prefix).
   */
  async listKeys(pattern = '*', batch = 500): Promise<string[]> {
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

  /**
   * Delete all keys in this plugin namespace.
   * Returns the number of keys removed.
   * @returns The number of keys deleted.
   */
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

  /**
   * Run `fn` while holding a distributed lock for `key`.
   * The lock is acquired with SET NX PX and released via a token-checked
   * Lua script to avoid unlocking another client's lock.
   * - opts.ttlSec: lock expiry in seconds (default 30)
   * - opts.onBusy: 'throw' (default) or 'skip' to return null when busy
   * @typeParam T - The return type of `fn`.
   * @param key - The lock key.
   * @param fn - Async function to execute while holding the lock.
   * @param opts - Lock options.
   * @param opts.ttlSec - Lock expiry in seconds (default 30).
   * @param opts.onBusy - Behavior when the lock is busy: 'throw' or 'skip'.
   * @returns The result of `fn`, or null when skipped due to a busy lock.
   * @throws Error if the lock is busy and `onBusy` is 'throw'.
   */
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
