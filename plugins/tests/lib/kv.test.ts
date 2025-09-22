import { DefaultPluginKVStore } from '../../lib/kv';

jest.mock('ioredis', () => require('ioredis-mock'));

describe('PluginKV', () => {
  let kv: DefaultPluginKVStore;

  beforeEach(async () => {
    kv = new DefaultPluginKVStore('test-plugin');
    await kv.connect();
  });

  afterEach(async () => {
    await kv.clear();
    await kv.disconnect();
  });

  describe('basic operations', () => {
    test('get/set operations with JSON values and return boolean', async () => {
      const ok = await kv.set('key1', { value: 'test', nested: { prop: 123 } });
      expect(ok).toBe(true);
      const result = await kv.get('key1');
      expect(result).toEqual({ value: 'test', nested: { prop: 123 } });
    });

    test('get returns null for non-existent key', async () => {
      const result = await kv.get('nonexistent');
      expect(result).toBeNull();
    });

    test('set with TTL expires key', async () => {
      const ok = await kv.set('tempkey', 'tempvalue', { ttlSec: 1 });
      expect(ok).toBe(true);
      const result1 = await kv.get('tempkey');
      expect(result1).toBe('tempvalue');

      // Wait for expiry
      await new Promise((resolve) => setTimeout(resolve, 1100));
      const result2 = await kv.get('tempkey');
      expect(result2).toBeNull();
    });

    test('set throws error for undefined value', async () => {
      await expect(kv.set('key', undefined as any)).rejects.toThrow('value must not be undefined');
    });

    test('del operation', async () => {
      await kv.set('key1', 'value1');
      const deleted = await kv.del('key1');
      expect(deleted).toBe(true);

      const result = await kv.get('key1');
      expect(result).toBeNull();

      // Deleting non-existent key
      const deleted2 = await kv.del('nonexistent');
      expect(deleted2).toBe(false);
    });

    test('exists operation', async () => {
      await kv.set('key1', 'value1');
      const exists1 = await kv.exists('key1');
      expect(exists1).toBe(true);

      const exists2 = await kv.exists('nonexistent');
      expect(exists2).toBe(false);
    });
  });

  // IfMode removed; basic set/get covers behavior

  describe('scan and clear', () => {
    test('scan returns only data keys and respects pattern', async () => {
      await kv.set('user:1', { name: 'Alice' });
      await kv.set('user:2', { name: 'Bob' });
      // Hold a lock during scan to ensure lock keys are not included
      await kv.withLock('some-resource', async () => {
        const userKeys = await kv.listKeys('user:*');
        expect(userKeys).toHaveLength(2);
        expect(userKeys).toContain('user:1');
        expect(userKeys).toContain('user:2');
      });

      const allKeys = await kv.listKeys();
      expect(allKeys).toHaveLength(2);
    });

    test('clear removes all keys in namespace', async () => {
      const n = 20;
      for (let i = 0; i < n; i++) {
        await kv.set(`key${i}`, `value${i}`);
      }
      const deleted = await kv.clear();
      expect(deleted).toBeGreaterThanOrEqual(n);
      const keys = await kv.listKeys();
      expect(keys).toHaveLength(0);
    });
  });

  describe('locking operations', () => {
    test('withLock acquires and releases lock', async () => {
      let lockAcquired = false;
      const result = await kv.withLock('test-resource', async () => {
        lockAcquired = true;
        return 'success';
      });
      expect(lockAcquired).toBe(true);
      expect(result).toBe('success');
    });

    test('withLock prevents concurrent access (one throws lock busy)', async () => {
      const results: number[] = [];
      let counter = 0;
      const promises = [
        kv.withLock('counter', async () => {
          const val = counter;
          await new Promise((resolve) => setTimeout(resolve, 50));
          counter = val + 1;
          results.push(counter);
          return counter;
        }),
        kv.withLock('counter', async () => {
          const val = counter;
          await new Promise((resolve) => setTimeout(resolve, 50));
          counter = val + 1;
          results.push(counter);
          return counter;
        }),
      ];
      const outcomes = await Promise.allSettled(promises);
      const successful = outcomes.filter((o) => o.status === 'fulfilled');
      const failed = outcomes.filter((o) => o.status === 'rejected');
      expect(successful).toHaveLength(1);
      expect(failed).toHaveLength(1);
      if (failed[0].status === 'rejected') {
        expect((failed[0] as PromiseRejectedResult).reason.message).toBe('lock busy');
      }
    });

    test('withLock onBusy=skip returns null when lock is held', async () => {
      let holder1Started = false;
      const holder1 = kv.withLock('resource', async () => {
        holder1Started = true;
        await new Promise((resolve) => setTimeout(resolve, 100));
        return 'holder1';
      });

      const holder2 = kv.withLock('resource', async () => 'holder2', { onBusy: 'skip' });
      const [result1, result2] = await Promise.all([holder1, holder2]);
      expect(holder1Started).toBe(true);
      expect(result1).toBe('holder1');
      expect(result2).toBeNull();
    });

    test('lock key is removed when fn throws', async () => {
      await expect(
        kv.withLock('boom', async () => {
          throw new Error('explode');
        })
      ).rejects.toThrow('explode');

      // Next attempt should succeed, meaning lock was released
      const result = await kv.withLock('boom', async () => 'ok');
      expect(result).toBe('ok');
    });
  });

  describe('namespace isolation', () => {
    test('different plugins have isolated namespaces', async () => {
      const kv1 = new DefaultPluginKVStore('plugin1');
      const kv2 = new DefaultPluginKVStore('plugin2');

      try {
        await kv1.connect();
        await kv2.connect();

        await kv1.set('shared', 'value1');
        await kv2.set('shared', 'value2');

        const val1 = await kv1.get('shared');
        const val2 = await kv2.get('shared');

        expect(val1).toBe('value1');
        expect(val2).toBe('value2');
      } finally {
        await kv1.clear();
        await kv2.clear();
        await kv1.disconnect();
        await kv2.disconnect();
      }
    });
  });

  describe('error handling', () => {
    test('invalid keys throw errors', async () => {
      await expect(kv.set('', 'value')).rejects.toThrow('invalid key');
      await expect(kv.set('key with spaces', 'value')).rejects.toThrow('invalid key');
      await expect(kv.get('')).rejects.toThrow('invalid key');
      await expect(kv.del('key with spaces')).rejects.toThrow('invalid key');
    });
  });
});
