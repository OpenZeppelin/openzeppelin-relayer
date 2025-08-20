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

    it('should throw error if write fails', async () => {
      mockWrite.mockReturnValue(false);

      const payload: NetworkTransactionRequest = {
        to: '0x1234567890123456789012345678901234567890',
        value: 1000000,
        data: '0x',
        gas_limit: 21000,
        speed: Speed.FAST,
      };

      await expect(pluginAPI._send('test-relayer', 'sendTransaction', payload))
        .rejects.toThrow('Failed to send message to relayer');
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
