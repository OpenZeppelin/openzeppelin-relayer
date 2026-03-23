import '@jest/globals';
import * as net from 'node:net';
import * as path from 'node:path';
import * as os from 'node:os';
import * as fs from 'node:fs';

/**
 * Regression test: mid-execution socket reconnection.
 *
 * Validates that if the server closes the socket between API calls within the
 * same plugin execution, the next call transparently reconnects with a fresh
 * socket instead of failing permanently.
 *
 * This exercises the reconnect-on-null-socket path added when the SocketPool
 * was removed in favour of per-execution connections.
 */

describe('PluginAPIImpl mid-execution reconnect', () => {
  let socketPath: string;
  let tmpDir: string;
  let server: net.Server;
  let connectionCount: number;

  beforeEach(async () => {
    connectionCount = 0;
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'plugin-test-'));
    socketPath = path.join(tmpDir, 'test.sock');
  });

  afterEach(async () => {
    // Force-close the server and all connections
    await new Promise<void>((resolve) => {
      if (server?.listening) {
        server.close(() => resolve());
        // Destroy any lingering connections by unref'ing
        server.unref();
      } else {
        resolve();
      }
    });

    try {
      if (fs.existsSync(socketPath)) fs.unlinkSync(socketPath);
      if (fs.existsSync(tmpDir)) fs.rmdirSync(tmpDir);
    } catch {
      // best-effort cleanup
    }
  });

  it('reconnects transparently when socket closes between API calls', async () => {
    await new Promise<void>((resolve) => {
      server = net.createServer((conn) => {
        connectionCount++;
        const currentConn = connectionCount;
        let buffer = '';

        conn.on('data', (data) => {
          buffer += data.toString();
          let newlineIndex;
          while ((newlineIndex = buffer.indexOf('\n')) !== -1) {
            const line = buffer.slice(0, newlineIndex);
            buffer = buffer.slice(newlineIndex + 1);
            if (!line) continue;

            try {
              const req = JSON.parse(line);
              const response = JSON.stringify({
                requestId: req.requestId,
                result: { method: req.method, connection: currentConn },
              }) + '\n';
              conn.write(response, () => {
                // Destroy connection 1 right after the first response is flushed.
                // This deterministically triggers the reconnect path.
                if (currentConn === 1) {
                  conn.destroy();
                }
              });
            } catch {
              // ignore
            }
          }
        });
      });
      server.listen(socketPath, () => resolve());
    });

    const { default: executePlugin } = await import('../../lib/pool-executor');

    // Plugin makes two API calls. The server destroys connection 1 right after
    // flushing the first response. The second call must reconnect transparently.
    const pluginCode = `
      module.exports.handler = async function(ctx) {
        const relayer = ctx.api.useRelayer('test-relayer');

        // First call — uses connection 1
        const first = await relayer.getRelayer();

        // Signal that the first call completed. The test server will destroy
        // connection 1 after responding, so wait for the close event to propagate.
        await new Promise(r => setTimeout(r, 100));

        // Second call — connection 1 is dead, must reconnect
        const second = await relayer.getRelayer();
        return { first, second };
      };
    `;

    const result = await executePlugin({
      taskId: 'reconnect-test',
      pluginId: 'test-plugin',
      compiledCode: pluginCode,
      params: {},
      socketPath,
      timeout: 10000,
    });

    expect(result.success).toBe(true);
    expect(result.result).toBeDefined();
    expect(result.result.first.connection).toBe(1);
    expect(result.result.second.connection).toBe(2);
    expect(connectionCount).toBe(2);
  }, 15000);

  it('fails when server is unavailable', async () => {
    // Don't start any server — socket path exists but nothing is listening
    const { default: executePlugin } = await import('../../lib/pool-executor');

    const pluginCode = `
      module.exports.handler = async function(ctx) {
        const relayer = ctx.api.useRelayer('test-relayer');
        return await relayer.getRelayer();
      };
    `;

    const result = await executePlugin({
      taskId: 'no-server-test',
      pluginId: 'test-plugin',
      compiledCode: pluginCode,
      params: {},
      socketPath,
      timeout: 5000,
    });

    expect(result.success).toBe(false);
    expect(result.error).toBeDefined();
  }, 10000);
});
