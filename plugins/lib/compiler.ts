/**
 * Plugin Compiler Module
 *
 * Uses esbuild for fast in-memory TypeScript → JavaScript compilation.
 * No filesystem writes - compiled code is returned for storage in memory/Redis.
 */

import * as esbuild from 'esbuild';
import type { Message } from 'esbuild';
import * as fs from 'node:fs';
import * as path from 'node:path';

export interface CompilationResult {
  /** The compiled JavaScript code */
  code: string;
  /** Source map (optional, for debugging) */
  sourceMap?: string;
  /** Compilation warnings */
  warnings: string[];
}

export interface CompilerOptions {
  /** Target ECMAScript version */
  target?: 'es2020' | 'es2021' | 'es2022' | 'esnext';
  /** Whether to generate source maps */
  sourcemap?: boolean;
  /** Whether to minify the output */
  minify?: boolean;
}

const DEFAULT_OPTIONS: Required<CompilerOptions> = {
  target: 'es2022',
  sourcemap: false,
  minify: false,
};

/**
 * Compiles a TypeScript plugin file to JavaScript in-memory using esbuild.
 *
 * @param pluginPath - Path to the TypeScript plugin file
 * @param options - Compilation options
 * @returns Compiled JavaScript code and metadata
 */
export async function compilePlugin(
  pluginPath: string,
  options: CompilerOptions = {}
): Promise<CompilationResult> {
  const opts = { ...DEFAULT_OPTIONS, ...options };

  // Read the source file
  const absolutePath = path.isAbsolute(pluginPath)
    ? pluginPath
    : path.resolve(process.cwd(), pluginPath);

  const sourceCode = await fs.promises.readFile(absolutePath, 'utf-8');

  return compilePluginSource(sourceCode, pluginPath, opts);
}

/**
 * Compiles TypeScript source code to JavaScript in-memory.
 * This variant accepts source code directly (useful when code is already in memory).
 *
 * @param sourceCode - TypeScript source code
 * @param sourcePath - Original path (for error messages and source maps)
 * @param options - Compilation options
 * @returns Compiled JavaScript code and metadata
 */
export async function compilePluginSource(
  sourceCode: string,
  sourcePath: string,
  options: CompilerOptions = {}
): Promise<CompilationResult> {
  const opts = { ...DEFAULT_OPTIONS, ...options };

  try {
    const result = await esbuild.transform(sourceCode, {
      loader: 'ts',
      target: opts.target,
      sourcemap: opts.sourcemap ? 'inline' : false,
      minify: opts.minify,
      sourcefile: sourcePath,
      format: 'cjs', // CommonJS for vm.Script compatibility
      platform: 'node',
      // Don't bundle - we want to keep imports for the sandbox to resolve
      // The sandbox will provide the necessary globals
    });

    const warnings = result.warnings.map((w: Message) => {
      const location = w.location
        ? `${w.location.file}:${w.location.line}:${w.location.column}`
        : sourcePath;
      return `${location}: ${w.text}`;
    });

    return {
      code: result.code,
      sourceMap: result.map || undefined,
      warnings,
    };
  } catch (error) {
    if (error instanceof Error) {
      throw new Error(`Failed to compile plugin ${sourcePath}: ${error.message}`);
    }
    throw error;
  }
}

/**
 * Batch compile multiple plugins.
 *
 * @param pluginPaths - Array of plugin file paths
 * @param options - Compilation options
 * @returns Map of plugin path to compilation result
 */
export async function compilePlugins(
  pluginPaths: string[],
  options: CompilerOptions = {}
): Promise<Map<string, CompilationResult>> {
  const results = new Map<string, CompilationResult>();

  // Compile in parallel for better performance
  const compilations = await Promise.allSettled(
    pluginPaths.map(async (pluginPath) => {
      const result = await compilePlugin(pluginPath, options);
      return { pluginPath, result };
    })
  );

  for (const compilation of compilations) {
    if (compilation.status === 'fulfilled') {
      results.set(compilation.value.pluginPath, compilation.value.result);
    } else {
      // Log compilation errors but continue with other plugins
      console.error(`Compilation failed: ${compilation.reason}`);
    }
  }

  return results;
}

/**
 * Creates a wrapped version of compiled code that exports the handler.
 * This wrapping is necessary for vm.Script execution.
 *
 * @param compiledCode - The compiled JavaScript code
 * @returns Wrapped code ready for vm.Script
 */
export function wrapForVm(compiledCode: string): string {
  // The compiled code uses CommonJS (module.exports / exports.handler)
  // We wrap it to capture the exports in the vm context
  return `
(function(exports, require, module, __filename, __dirname) {
${compiledCode}
});
`;
}
