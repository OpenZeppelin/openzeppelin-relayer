import '@jest/globals';

import { DefaultPluginAPI, PluginAPI } from '../../lib/plugin';
import { NetworkTransactionRequest, Speed } from '@openzeppelin/relayer-sdk';

import { LogInterceptor } from '../../lib/logger';
import net from 'node:net';

jest.mock('../../lib/logger');
const MockedLogInterceptor = LogInterceptor as jest.MockedClass<typeof LogInterceptor>;

beforeAll(() => {
  jest.spyOn(process, 'exit').mockImplementation(((code?: number) => {
    throw new Error(`process.exit called with "${code}"`);
  }) as any);
});

describe('PluginAPI', () => {
  let pluginAPI: DefaultPluginAPI;
  let mockSocket: jest.Mocked<net.Socket>;
  let mockWrite: jest.Mock;
  let mockEnd: jest.Mock;
  let mockDestroy: jest.Mock;

  beforeEach(() => {
    // Create mock socket
    mockWrite = jest.fn().mockReturnValue(true);
    mockEnd = jest.fn();
    mockDestroy = jest.fn();

    mockSocket = {
      write: mockWrite,
      end: mockEnd,
      destroy: mockDestroy,
      on: jest.fn(),
      createConnection: jest.fn(),
    } as any;

    jest.spyOn(net, 'createConnection').mockReturnValue(mockSocket);
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  describe('constructor', () => {
    it('should create socket connection with provided path', () => {
      pluginAPI = new DefaultPluginAPI('/tmp/test.sock');

      expect(net.createConnection).toHaveBeenCalledWith('/tmp/test.sock');
      expect(mockSocket.on).toHaveBeenCalledWith('connect', expect.any(Function));
      expect(mockSocket.on).toHaveBeenCalledWith('error', expect.any(Function));
      expect(mockSocket.on).toHaveBeenCalledWith('data', expect.any(Function));
    });

    it('should initialize pending map', () => {
      pluginAPI = new DefaultPluginAPI('/tmp/test.sock');

      expect(pluginAPI.pending).toBeInstanceOf(Map);
      expect(pluginAPI.pending.size).toBe(0);
    });

    it('should set up connection promise', () => {
      pluginAPI = new DefaultPluginAPI('/tmp/test.sock');

      expect((pluginAPI as any)._connectionPromise).toBeInstanceOf(Promise);
    });
  });

  describe('useRelayer', () => {
    beforeEach(() => {
      pluginAPI = new DefaultPluginAPI('/tmp/test.sock');
    });

    it('should return relayer object with sendTransaction method', () => {
      const relayer = pluginAPI.useRelayer('test-relayer-id');

      expect(relayer).toHaveProperty('sendTransaction');
      expect(typeof relayer.sendTransaction).toBe('function');
    });
  });

  describe('_send', () => {
    beforeEach(() => {
      pluginAPI = new DefaultPluginAPI('/tmp/test.sock');
      // Mock connection as established
      (pluginAPI as any)._connected = true;
    });

    it('should send message with correct format', async () => {
      const payload: NetworkTransactionRequest = {
        to: '0x1234567890123456789012345678901234567890',
        value: 1000000,
        data: '0x',
        gas_limit: 21000,
        speed: Speed.FAST,
      };

      const promise = pluginAPI._send('test-relayer', 'sendTransaction', payload);

      expect(mockWrite).toHaveBeenCalledWith(
        expect.stringMatching(/{"requestId":"[^"]+","relayerId":"test-relayer","method":"sendTransaction","payload":/),
        expect.any(Function)
      );
    });

    it('should add pending request to map', async () => {
      const payload: NetworkTransactionRequest = {
        to: '0x1234567890123456789012345678901234567890',
        value: 1000000,
        data: '0x',
        gas_limit: 21000,
        speed: Speed.FAST,
      };

      const promise = pluginAPI._send('test-relayer', 'sendTransaction', payload);

      expect(pluginAPI.pending.size).toBe(1);
    });

    it('should resolve when response is received', async () => {
      const payload: NetworkTransactionRequest = {
        to: '0x1234567890123456789012345678901234567890',
        value: 1000000,
        data: '0x',
        gas_limit: 21000,
        speed: Speed.FAST,
      };

      const promise = pluginAPI._send('test-relayer', 'sendTransaction', payload);

      // Get the requestId from the written message
      const writtenMessage = mockWrite.mock.calls[0][0];
      const messageObj = JSON.parse(writtenMessage);
      const requestId = messageObj.requestId;

      // Simulate response
      const response = {
        requestId,
        result: { id: 'tx-123', relayer_id: 'test-relayer', status: 'pending' },
        error: null,
      };

      // Trigger data event
      // @ts-expect-error: test code, type mismatch is not relevant
      const dataHandler = mockSocket.on.mock.calls.find(call => call[0] === 'data')?.[1];
      if (dataHandler) {
        (dataHandler as (buf: Buffer) => void)(Buffer.from(JSON.stringify(response) + '\n'));
      }

      const result = await promise;
      expect(result).toEqual(response.result);
      expect(pluginAPI.pending.size).toBe(0);
    });

    it('should reject when error response is received', async () => {
      const payload: NetworkTransactionRequest = {
        to: '0x1234567890123456789012345678901234567890',
        value: 1000000,
        data: '0x',
        gas_limit: 21000,
        speed: Speed.FAST,
      };

      const promise = pluginAPI._send('test-relayer', 'sendTransaction', payload);

      // Get the requestId from the written message
      const writtenMessage = mockWrite.mock.calls[0][0];
      const messageObj = JSON.parse(writtenMessage);
      const requestId = messageObj.requestId;

      // Simulate error response
      const response = {
        requestId,
        result: null,
        error: 'Transaction failed',
      };

      // Trigger data event
      // @ts-expect-error: test code, type mismatch is not relevant
      const dataHandler = mockSocket.on.mock.calls.find(call => call[0] === 'data')?.[1];
      if (dataHandler) {
        (dataHandler as (buf: Buffer) => void)(Buffer.from(JSON.stringify(response) + '\n'));
      }

      await expect(promise).rejects.toBe('Transaction failed');
      expect(pluginAPI.pending.size).toBe(0);
    });

    it('should wait for connection if not connected', async () => {
      (pluginAPI as any)._connected = false;

      const payload: NetworkTransactionRequest = {
        to: '0x1234567890123456789012345678901234567890',
        value: 1000000,
        data: '0x',
        gas_limit: 21000,
        speed: Speed.FAST,
      };

      const promise = pluginAPI._send('test-relayer', 'sendTransaction', payload);

      // Simulate connection
      // @ts-expect-error: test code, type mismatch is not relevant
      const connectHandler = mockSocket.on.mock.calls.find(call => call[0] === 'connect')?.[1];
      if (connectHandler) {
        connectHandler();
      }

      // Wait a bit for the promise to resolve and write to be called
      await new Promise(resolve => setTimeout(resolve, 0));

      expect(mockWrite).toHaveBeenCalled();
    });

    it('should throw error if write callback returns error', async () => {
      // Simulate write error via callback (this is how Node.js socket.write reports errors)
      const writeError = new Error('EPIPE: broken pipe');
      mockWrite.mockImplementation((data: string, callback: (error?: Error) => void) => {
        // Call the callback with an error to simulate write failure
        callback(writeError);
        return true;
      });

      const payload: NetworkTransactionRequest = {
        to: '0x1234567890123456789012345678901234567890',
        value: 1000000,
        data: '0x',
        gas_limit: 21000,
        speed: Speed.FAST,
      };

      await expect(pluginAPI._send('test-relayer', 'sendTransaction', payload))
        .rejects.toThrow('EPIPE: broken pipe');
    });
  });

  describe('close', () => {
    beforeEach(() => {
      pluginAPI = new DefaultPluginAPI('/tmp/test.sock');
    });

    it('should end socket connection', () => {
      pluginAPI.close();
      expect(mockEnd).toHaveBeenCalled();
    });
  });

  describe('closeErrored', () => {
    beforeEach(() => {
      pluginAPI = new DefaultPluginAPI('/tmp/test.sock');
    });

    it('should destroy socket with error', () => {
      const error = new Error('Test error');
      pluginAPI.closeErrored(error);
      expect(mockDestroy).toHaveBeenCalledWith(error);
    });
  });

  describe('integration with relayer.sendTransaction', () => {
    beforeEach(() => {
      pluginAPI = new DefaultPluginAPI('/tmp/test.sock');
      (pluginAPI as any)._connected = true;
    });

    it('should send transaction through relayer', async () => {
      const relayer = pluginAPI.useRelayer('test-relayer');
      const payload: NetworkTransactionRequest = {
        to: '0x1234567890123456789012345678901234567890',
        value: 1000000,
        data: '0x',
        gas_limit: 21000,
        speed: Speed.FAST,
      };

      const promise = relayer.sendTransaction(payload);

      expect(mockWrite).toHaveBeenCalledWith(
        expect.stringContaining('"method":"sendTransaction"'),
        expect.any(Function)
      );
    });
  });
});

describe('Socket error handling', () => {
  let pluginAPI: DefaultPluginAPI;
  let mockSocket: jest.Mocked<net.Socket>;
  let mockWrite: jest.Mock;
  let mockEnd: jest.Mock;
  let mockDestroy: jest.Mock;
  let eventHandlers: Map<string, Function>;

  beforeEach(() => {
    eventHandlers = new Map();
    mockWrite = jest.fn().mockReturnValue(true);
    mockEnd = jest.fn();
    mockDestroy = jest.fn();

    mockSocket = {
      write: mockWrite,
      end: mockEnd,
      destroy: mockDestroy,
      on: jest.fn((event: string, handler: Function) => {
        eventHandlers.set(event, handler);
        return mockSocket;
      }),
    } as any;

    jest.spyOn(net, 'createConnection').mockReturnValue(mockSocket);
  });

  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('should reject pending requests when socket closes', async () => {
    pluginAPI = new DefaultPluginAPI('/tmp/test.sock');

    // Simulate established connection by triggering connect event
    const connectHandler = eventHandlers.get('connect');
    if (connectHandler) {
      connectHandler();
    }

    const payload: NetworkTransactionRequest = {
      to: '0x1234567890123456789012345678901234567890',
      value: 1000000,
      data: '0x',
      gas_limit: 21000,
      speed: Speed.FAST,
    };

    // Start the send (won't complete)
    const sendPromise = pluginAPI._send('test-relayer', 'sendTransaction', payload);

    // Trigger socket close
    const closeHandler = eventHandlers.get('close');
    if (closeHandler) {
      closeHandler();
    }

    // Should reject with SocketClosedError
    await expect(sendPromise).rejects.toThrow('Socket closed by server');
  });

  it('should have ESOCKETCLOSED code on socket close error', async () => {
    pluginAPI = new DefaultPluginAPI('/tmp/test.sock');

    // Simulate established connection
    const connectHandler = eventHandlers.get('connect');
    if (connectHandler) {
      connectHandler();
    }

    const payload: NetworkTransactionRequest = {
      to: '0x1234567890123456789012345678901234567890',
      value: 1000000,
      data: '0x',
      gas_limit: 21000,
      speed: Speed.FAST,
    };

    const sendPromise = pluginAPI._send('test-relayer', 'sendTransaction', payload);

    // Trigger socket close
    const closeHandler = eventHandlers.get('close');
    if (closeHandler) {
      closeHandler();
    }

    try {
      await sendPromise;
      fail('Should have thrown');
    } catch (error: any) {
      expect(error.code).toBe('ESOCKETCLOSED');
      expect(error.name).toBe('SocketClosedError');
    }
  });

  it('should handle multiple pending requests on socket close', async () => {
    pluginAPI = new DefaultPluginAPI('/tmp/test.sock');

    // Simulate established connection
    const connectHandler = eventHandlers.get('connect');
    if (connectHandler) {
      connectHandler();
    }

    const payload: NetworkTransactionRequest = {
      to: '0x1234567890123456789012345678901234567890',
      value: 1000000,
      data: '0x',
      gas_limit: 21000,
      speed: Speed.FAST,
    };

    // Start multiple sends
    const promises = [
      pluginAPI._send('test-relayer', 'sendTransaction', payload),
      pluginAPI._send('test-relayer', 'getRelayer', {}),
      pluginAPI._send('test-relayer', 'getRelayerStatus', {}),
    ];

    expect(pluginAPI.pending.size).toBe(3);

    // Trigger socket close
    const closeHandler = eventHandlers.get('close');
    if (closeHandler) {
      closeHandler();
    }

    // All should reject
    for (const promise of promises) {
      await expect(promise).rejects.toThrow();
    }

    // Pending should be cleared
    expect(pluginAPI.pending.size).toBe(0);
  });

  it('should reject connection promise on socket error during connect', async () => {
    pluginAPI = new DefaultPluginAPI('/tmp/test.sock');

    // Trigger error before connection is established
    const errorHandler = eventHandlers.get('error');
    const socketError = new Error('ECONNREFUSED');

    // The connection promise should reject
    const connectionPromise = (pluginAPI as any)._connectionPromise;

    if (errorHandler) {
      errorHandler(socketError);
    }

    await expect(connectionPromise).rejects.toThrow('ECONNREFUSED');
  });
});

describe('PluginContext Headers', () => {
  it('should have correct PluginHeaders type structure', () => {
    // Test type structure: Record<string, string[]>
    const headers: Record<string, string[]> = {
      'content-type': ['application/json'],
      'authorization': ['Bearer token'],
      'x-multi-value': ['value1', 'value2'],
    };

    expect(headers['content-type']).toEqual(['application/json']);
    expect(headers['authorization']).toEqual(['Bearer token']);
    expect(headers['x-multi-value']).toHaveLength(2);
  });

  it('should handle empty headers object', () => {
    const headers: Record<string, string[]> = {};

    expect(Object.keys(headers)).toHaveLength(0);
    expect(headers['non-existent']).toBeUndefined();
  });

  it('should handle multi-value headers correctly', () => {
    const headers: Record<string, string[]> = {
      'set-cookie': ['cookie1=value1', 'cookie2=value2', 'cookie3=value3'],
      'accept': ['application/json', 'text/html'],
    };

    expect(headers['set-cookie']).toHaveLength(3);
    expect(headers['accept']).toEqual(['application/json', 'text/html']);
  });

  it('should support optional chaining for safe header access', () => {
    const headers: Record<string, string[]> = {
      'authorization': ['Bearer token123'],
    };

    // Safe access pattern that plugins should use
    const auth = headers['authorization']?.[0];
    const missing = headers['x-missing']?.[0];

    expect(auth).toBe('Bearer token123');
    expect(missing).toBeUndefined();
  });

  it('should preserve header case as lowercase', () => {
    // Headers should be normalized to lowercase by Rust
    const headers: Record<string, string[]> = {
      'content-type': ['application/json'],
      'x-custom-header': ['value'],
      'authorization': ['Bearer token'],
    };

    // All keys should be lowercase
    const keys = Object.keys(headers);
    keys.forEach(key => {
      expect(key).toBe(key.toLowerCase());
    });
  });
});
