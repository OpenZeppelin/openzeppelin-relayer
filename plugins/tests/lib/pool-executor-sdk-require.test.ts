import '@jest/globals';
import * as path from 'node:path';
import * as fs from 'node:fs';
import * as os from 'node:os';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';

/**
 * Regression: when Piscina loads the worker from a temp path (on-the-fly
 * compile), ambient `require` cannot see `plugins/node_modules`. executePlugin
 * must root resolution at `<relayer-cwd>/plugins/package.json`.
 *
 * Jest runs with cwd=`plugins/`, so an in-process import of `plugins/lib/`
 * would still resolve the SDK via directory walking and would not catch a
 * revert to ambient `require`. Compile the worker into `os.tmpdir()` the same
 * way `compileExecutorOnTheFly` does, then spawn it with cwd set to the
 * Relayer root (parent of `plugins/`).
 */
describe('executePlugin SDK require resolution', () => {
  const pluginsDir = path.resolve(__dirname, '../..');
  const repoRoot = path.resolve(pluginsDir, '..');
  let tempWorkerPath: string | undefined;

  afterEach(() => {
    if (tempWorkerPath && fs.existsSync(tempWorkerPath)) {
      fs.unlinkSync(tempWorkerPath);
      tempWorkerPath = undefined;
    }
  });

  it('resolves @openzeppelin/relayer-sdk from a temp worker with Relayer cwd', async () => {
    expect(JSON.parse(fs.readFileSync(path.join(pluginsDir, 'package.json'), 'utf8')).name).toBe(
      'plugins'
    );
    expect(fs.existsSync(path.join(repoRoot, 'plugins', 'package.json'))).toBe(true);

    const esbuild = await import('esbuild');
    const buildResult = await esbuild.build({
      entryPoints: [path.join(pluginsDir, 'lib', 'pool-executor.ts')],
      bundle: true,
      platform: 'node',
      target: 'node18',
      format: 'cjs',
      sourcemap: false,
      write: false,
      loader: { '.ts': 'ts' },
      external: ['node:*'],
    });

    tempWorkerPath = path.join(os.tmpdir(), `pool-executor-sdk-require-${randomUUID()}.js`);
    fs.writeFileSync(tempWorkerPath, buildResult.outputFiles[0].text);

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

    const childScript = `
      const executePlugin = require(${JSON.stringify(tempWorkerPath)}).default;
      (async () => {
        const result = await executePlugin({
          taskId: 'sdk-require-test',
          pluginId: 'sdk-require',
          compiledCode: ${JSON.stringify(pluginCode)},
          params: {},
          socketPath: ${JSON.stringify(path.join(repoRoot, 'nonexistent-for-sdk-require.sock'))},
          timeout: 5000,
        });
        process.stdout.write(JSON.stringify(result));
      })().catch((err) => {
        console.error(err);
        process.exit(1);
      });
    `;

    const { stdout, stderr, code } = await new Promise<{
      stdout: string;
      stderr: string;
      code: number | null;
    }>((resolve, reject) => {
      const child = spawn(process.execPath, ['-e', childScript], {
        cwd: repoRoot,
        env: process.env,
      });
      let stdout = '';
      let stderr = '';
      child.stdout.on('data', (chunk: Buffer) => {
        stdout += chunk.toString();
      });
      child.stderr.on('data', (chunk: Buffer) => {
        stderr += chunk.toString();
      });
      child.on('error', reject);
      child.on('close', (exitCode) => resolve({ stdout, stderr, code: exitCode }));
    });

    expect(code).toBe(0);
    // Node may emit DeprecationWarning on stderr from transitive deps; fail only on real errors.
    expect(stderr).not.toMatch(/Cannot find module|Error:/);
    const pluginResult = JSON.parse(stdout) as {
      success: boolean;
      error?: string;
      result?: { ok: boolean; hasPluginError: boolean };
    };
    expect(pluginResult.success).toBe(true);
    expect(pluginResult.error).toBeUndefined();
    expect(pluginResult.result).toEqual({ ok: true, hasPluginError: true });
  }, 30000);
});
