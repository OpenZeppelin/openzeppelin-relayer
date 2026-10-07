import '@jest/globals';
import * as path from 'node:path';
import * as fs from 'node:fs';

/**
 * Regression: compiler-externalized `@openzeppelin/relayer-sdk` must resolve
 * when the pool worker passes `require` into the plugin factory.
 *
 * Piscina may load pool-executor from a temp path; ambient `require` then
 * cannot see `plugins/node_modules`. executePlugin must use a require rooted
 * at `plugins/package.json` (cwd-relative) so SDK resolution still works.
 */
describe('executePlugin SDK require resolution', () => {
  it('resolves @openzeppelin/relayer-sdk from plugins/node_modules', async () => {
    const pluginsPkg = path.resolve(process.cwd(), 'package.json');
    expect(fs.existsSync(pluginsPkg)).toBe(true);
    expect(JSON.parse(fs.readFileSync(pluginsPkg, 'utf8')).name).toBe('plugins');

    const { default: executePlugin } = await import('../../lib/pool-executor');

    // Mirrors compiler output: SDK left external, required at plugin load time.
    const pluginCode = `
      const sdk = require('@openzeppelin/relayer-sdk');
      module.exports.handler = async function () {
        return {
          ok: true,
          hasPluginError: typeof sdk.pluginError === 'function',
        };
      };
    `;

    const result = await executePlugin({
      taskId: 'sdk-require-test',
      pluginId: 'sdk-require',
      compiledCode: pluginCode,
      params: {},
      socketPath: path.join(process.cwd(), 'nonexistent-for-sdk-require.sock'),
      timeout: 5000,
    });

    expect(result.success).toBe(true);
    expect(result.error).toBeUndefined();
    expect(result.result).toEqual({ ok: true, hasPluginError: true });
  });
});
