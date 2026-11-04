import '@jest/globals';

import net from 'node:net';
import { PluginAPIImpl } from '../../lib/pool-executor';
import { NetworkTransactionRequest, Speed } from '@openzeppelin/relayer-sdk';

/**
 * Goes through PluginAPIImpl.sendTransaction so removing attachIdempotencyKey
 * from pool-executor.ts fails here. Calling the helper directly would stay green.
 */
describe('pool-executor idempotencyKey serialization', () => {
  const payload: NetworkTransactionRequest = {
    to: '0x1234567890123456789012345678901234567890',
    value: 0,
    data: '0x',
    gas_limit: 21000,
    speed: Speed.FAST,
  };

  let written: string;
  let dataHandler: ((buf: Buffer) => void) | undefined;

  beforeEach(() => {
    written = '';
    dataHandler = undefined;
    const handlers = new Map<string, Array<(...args: unknown[]) => void>>();
    const on = (event: string, handler: (...args: unknown[]) => void) => {
      const list = handlers.get(event) ?? [];
      list.push(handler);
      handlers.set(event, list);
      return mockSocket;
    };
    const mockSocket = {
      write: jest.fn((message: string, cb?: (err?: Error) => void) => {
        written = message;
        cb?.();
        return true;
      }),
      on,
      once: on,
      destroy: jest.fn(),
      removeListener: jest.fn(),
    };
    jest.spyOn(net, 'createConnection').mockImplementation(() => {
      queueMicrotask(() => {
        handlers.get('connect')?.forEach((handler) => handler());
      });
      dataHandler = (buf: Buffer) => {
        handlers.get('data')?.forEach((handler) => handler(buf));
      };
      return mockSocket as unknown as net.Socket;
    });
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  async function send(options?: { idempotencyKey?: string }) {
    const api = new PluginAPIImpl('/tmp/pool-idem.sock');
    const relayer = api.useRelayer('test-relayer');
    const pending = relayer.sendTransaction(payload, options);
    await new Promise((resolve) => setImmediate(resolve));
    const message = JSON.parse(written);
    dataHandler?.(
      Buffer.from(
        JSON.stringify({
          requestId: message.requestId,
          result: { id: 'tx-pool', relayer_id: 'test-relayer', status: 'pending' },
          error: null,
        }) + '\n',
      ),
    );
    await pending;
    return message as Record<string, unknown>;
  }

  it('sets camelCase idempotencyKey when supplied', async () => {
    const message = await send({ idempotencyKey: 'aa:0xpool' });
    expect(message.idempotencyKey).toBe('aa:0xpool');
    expect(message.method).toBe('sendTransaction');
  });

  it('forwards an explicitly empty idempotencyKey', async () => {
    const message = await send({ idempotencyKey: '' });
    expect(message).toHaveProperty('idempotencyKey', '');
  });

  it('omits the field when options are absent', async () => {
    const message = await send();
    expect(message).not.toHaveProperty('idempotencyKey');
  });
});
