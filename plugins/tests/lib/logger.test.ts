import '@jest/globals';
import { LogInterceptor, LogEntry, LogLevel } from '../../lib/logger';

describe('LogInterceptor', () => {
  let logInterceptor: LogInterceptor;
  let originalConsole: any;

  beforeEach(() => {
    // Store original console methods
    originalConsole = {
      log: console.log,
      error: console.error,
      warn: console.warn,
      info: console.info,
      debug: console.debug,
    };

    logInterceptor = new LogInterceptor();
  });

  afterEach(() => {
    // Restore original console methods
    Object.assign(console, originalConsole);
  });

  describe('constructor', () => {
    it('should initialize with empty logs array', () => {
      expect(logInterceptor.getLogs()).toEqual([]);
    });

    it('should store original console methods', () => {
      expect(logInterceptor).toHaveProperty('originalConsole');
    });
  });

  describe('start()', () => {
    it('should intercept console.log calls', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.log('test message');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0]).toEqual({
        level: 'log',
        message: 'test message'
      });

      expect(mockLog).toHaveBeenCalledWith(JSON.stringify({
        level: 'log',
        message: 'test message'
      }));
    });

    it('should intercept console.error calls', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.error('error message');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0]).toEqual({
        level: 'error',
        message: 'error message'
      });
    });

    it('should intercept console.warn calls', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.warn('warning message');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0]).toEqual({
        level: 'warn',
        message: 'warning message'
      });
    });

    it('should intercept console.info calls', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.info('info message');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0]).toEqual({
        level: 'info',
        message: 'info message'
      });
    });

    it('should intercept console.debug calls', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.debug('debug message');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0]).toEqual({
        level: 'debug',
        message: 'debug message'
      });
    });

    it('should handle multiple arguments', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.log('message', 123, { key: 'value' });

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0].message).toBe('message 123 {"key":"value"}');
    });

    it('should handle Error objects', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      const error = new Error('test error');
      console.error(error);

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0].message).toBe('test error');
    });

    it('should handle non-string objects', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      const obj = { test: 'object' };
      console.log(obj);

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0].message).toBe('{"test":"object"}');
    });

    it('should accumulate multiple log entries', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.log('first message');
      console.error('second message');
      console.warn('third message');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(3);
      expect(logs[0].level).toBe('log');
      expect(logs[1].level).toBe('error');
      expect(logs[2].level).toBe('warn');
    });
  });

  describe('addResult()', () => {
    it('should add result log entry', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.addResult('test result');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0]).toEqual({
        level: 'result',
        message: 'test result'
      });

      expect(mockLog).toHaveBeenCalledWith(JSON.stringify({
        level: 'result',
        message: 'test result'
      }));
    });

    it('should add result after other logs', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.log('regular log');
      logInterceptor.addResult('test result');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(2);
      expect(logs[0].level).toBe('log');
      expect(logs[1].level).toBe('result');
    });
  });

  describe('stop()', () => {
    it('should restore original console methods', () => {
      const originalLog = console.log;
      const originalError = console.error;

      logInterceptor.start();
      expect(console.log).not.toBe(originalLog);

      logInterceptor.stop();
      expect(console.log).toBe(originalLog);
      expect(console.error).toBe(originalError);
    });

    it('should not affect collected logs', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.log('test message');
      logInterceptor.stop();

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(1);
      expect(logs[0].message).toBe('test message');
    });
  });

  describe('getLogs()', () => {
    it('should return a copy of logs array', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();
      console.log('test message');

      const logs1 = logInterceptor.getLogs();
      const logs2 = logInterceptor.getLogs();

      expect(logs1).toEqual(logs2);
      expect(logs1).not.toBe(logs2); // Should be different references
    });

    it('should return empty array when no logs', () => {
      const logs = logInterceptor.getLogs();
      expect(logs).toEqual([]);
    });
  });

  describe('integration scenarios', () => {
    it('should handle complete workflow', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      // Start intercepting
      logInterceptor.start();

      // Add various log types
      console.log('info message');
      console.error('error occurred');
      console.warn('warning message');
      console.info('information');
      console.debug('debug info');

      // Add result
      logInterceptor.addResult('operation completed');

      // Stop intercepting
      logInterceptor.stop();

      // Verify all logs were captured
      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(6);

      const levels = logs.map(log => log.level);
      expect(levels).toEqual(['log', 'error', 'warn', 'info', 'debug', 'result']);

      const messages = logs.map(log => log.message);
      expect(messages).toEqual([
        'info message',
        'error occurred',
        'warning message',
        'information',
        'debug info',
        'operation completed'
      ]);
    });

    it('should handle complex objects and errors', () => {
      const mockLog = jest.spyOn(console, 'log').mockImplementation(() => {});

      logInterceptor.start();

      const complexObj = {
        nested: {
          array: [1, 2, 3],
          string: 'test'
        },
        boolean: true,
        number: 42
      };

      console.log('Complex object:', complexObj);
      console.error(new Error('Something went wrong'));
      console.warn('Warning with', 123, 'numbers');

      const logs = logInterceptor.getLogs();
      expect(logs).toHaveLength(3);

      expect(logs[0].message).toContain('Complex object:');
      expect(logs[0].message).toContain('"nested"');

      expect(logs[1].message).toBe('Something went wrong');

      expect(logs[2].message).toBe('Warning with 123 numbers');
    });
  });
});
