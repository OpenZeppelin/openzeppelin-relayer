import { PluginContext } from '../lib/plugin';

/**
 * Simple KV storage example
 *
 * Demonstrates:
 * - JSON set/get (with optional TTL)
 * - exists/del
 * - scan pattern listing
 * - clear namespace
 * - withLock for atomic sections
 *
 * Usage (params.action):
 * - 'demo' (default): run a small end-to-end flow
 * - 'set': { key: string, value: any, ttlSec?: number }
 * - 'get': { key: string }
 * - 'exists': { key: string }
 * - 'del': { key: string }
 * - 'scan': { pattern?: string, batch?: number }
 * - 'clear': {}
 * - 'withLock': { key: string, ttlSec?: number, onBusy?: 'throw' | 'skip' }
 */
export async function handler({ kv, params }: PluginContext) {
  const action = params?.action ?? 'demo';

  switch (action) {
    case 'set': {
      const { key, value, ttlSec } = params ?? {};
      assertString(key, 'key');
      const ok = await kv.set(key, value, { ttlSec: toInt(ttlSec) });
      return { ok };
    }

    case 'get': {
      const { key } = params ?? {};
      assertString(key, 'key');
      const value = await kv.get(key);
      return { value };
    }

    case 'exists': {
      const { key } = params ?? {};
      assertString(key, 'key');
      const exists = await kv.exists(key);
      return { exists };
    }

    case 'del': {
      const { key } = params ?? {};
      assertString(key, 'key');
      const deleted = await kv.del(key);
      return { deleted };
    }

    case 'scan': {
      const { pattern, batch } = params ?? {};
      const keys = await kv.listKeys(pattern ?? '*', toInt(batch, 500));
      return { keys };
    }

    case 'clear': {
      const deleted = await kv.clear();
      return { deleted };
    }

    case 'withLock': {
      const { key, ttlSec, onBusy } = params ?? {};
      assertString(key, 'key');
      const result = await kv.withLock(
        key,
        async () => {
          // Simulate a small critical section
          const stamp = Date.now();
          await kv.set(`example:last-lock:${key}`, { stamp });
          return { ok: true, stamp };
        },
        { ttlSec: toInt(ttlSec, 30), onBusy: onBusy === 'skip' ? 'skip' : 'throw' }
      );
      return { result };
    }

    case 'demo':
    default: {
      // 1) Write JSON and read it back
      await kv.set('example:greeting', { text: 'hello' });
      const greeting = await kv.get<{ text: string }>('example:greeting');

      // 2) Write a TTL value (won't await expiry here)
      await kv.set('example:temp', { expires: true }, { ttlSec: 5 });

      // 3) Check existence and delete
      const existedBefore = await kv.exists('example:to-delete');
      await kv.set('example:to-delete', { remove: true });
      const existedAfterSet = await kv.exists('example:to-delete');
      const deleted = await kv.del('example:to-delete');

      // 4) Scan keys under example:*
      const list = await kv.listKeys('example:*');

      // 5) Use a lock to protect an update
      const lockResult = await kv.withLock(
        'example:lock',
        async () => {
          const count = (await kv.get<number>('example:counter')) ?? 0;
          const next = count + 1;
          await kv.set('example:counter', next);
          return next;
        },
        { ttlSec: 10, onBusy: 'throw' }
      );

      return {
        greeting,
        ttlKeyWritten: true,
        existedBefore,
        existedAfterSet,
        deleted,
        scanned: list,
        lockResult,
      };
    }
  }
}

function toInt(v: unknown, def = 0): number {
  const n = typeof v === 'string' ? parseInt(v, 10) : typeof v === 'number' ? Math.floor(v) : def;
  return Number.isFinite(n) && n > 0 ? n : def;
}

function assertString(v: any, name: string): asserts v is string {
  if (typeof v !== 'string' || v.length === 0) throw new Error(`${name} must be a non-empty string`);
}
