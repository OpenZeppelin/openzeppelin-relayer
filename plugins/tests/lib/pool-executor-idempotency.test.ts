import '@jest/globals';

import { attachIdempotencyKey } from '../../lib/plugin';

/**
 * Pool executor (`PluginAPIImpl`) serializes legacy socket messages with
 * camelCase `idempotencyKey` via `attachIdempotencyKey`. Cover that shared
 * helper here (the class itself is not exported).
 */
describe('pool-executor idempotencyKey serialization', () => {
  it('sets camelCase idempotencyKey when supplied', () => {
    const msg: Record<string, unknown> = {
      requestId: 'r1',
      relayerId: 'test-relayer',
      method: 'sendTransaction',
      payload: { to: '0x1' },
    };
    attachIdempotencyKey(msg, { idempotencyKey: 'aa:0xpool' }, 'idempotencyKey');
    expect(msg.idempotencyKey).toBe('aa:0xpool');
  });

  it('forwards an explicitly empty idempotencyKey', () => {
    const msg: Record<string, unknown> = { method: 'sendTransaction' };
    attachIdempotencyKey(msg, { idempotencyKey: '' }, 'idempotencyKey');
    expect(msg).toHaveProperty('idempotencyKey', '');
  });

  it('omits the field when options are absent', () => {
    const msg: Record<string, unknown> = { method: 'sendTransaction' };
    attachIdempotencyKey(msg, undefined, 'idempotencyKey');
    expect(msg).not.toHaveProperty('idempotencyKey');
  });
});
