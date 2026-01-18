/**
 * Plugin Compiler Module
 *
 * Uses esbuild for fast TypeScript → JavaScript compilation with bundling.
 * Dependencies are bundled into the output for self-contained plugins.
 *
 * Security:
 * - Input size limits to prevent memory exhaustion
 * - Path traversal protection
 * - Compilation timeout
 * - Sanitized error messages
 */

import * as esbuild from 'esbuild';
import type { Message } from 'esbuild';
import * as fs from 'node:fs';
import * as path from 'node:path';

/** Maximum source file size (5MB) */
const MAX_SOURCE_SIZE = 5 * 1024 * 1024;

/** Maximum compiled code size (10MB) */
const MAX_COMPILED_SIZE = 10 * 1024 * 1024;

/** Default compilation timeout (30 seconds) */
const DEFAULT_COMPILE_TIMEOUT_MS = 30000;

/** Maximum concurrent compilations in batch mode */
const MAX_CONCURRENT_COMPILATIONS = 10;

/** Default base directory for plugins */
const DEFAULT_PLUGIN_BASE_DIR = path.resolve(process.cwd(), 'plugins');

/**
 * Result of compiling a plugin
 */
export interface CompilationResult {
  /** The compiled JavaScript code */
  code: string;
  /** Source map (optional, for debugging) */
  sourceMap?: string;
  /** Compilation warnings */
  warnings: string[];
}

/**
 * Options for plugin compilation
 */
export interface CompilerOptions {
  /** Target ECMAScript version */
  target?: 'es2020' | 'es2021' | 'es2022' | 'esnext';
  /** Whether to generate source maps ('inline' or 'external') */
  sourcemap?: boolean | 'inline' | 'external';
  /** Whether to minify the output */
  minify?: boolean;
  /** Compilation timeout in milliseconds (default: 30000) */
  timeout?: number;
  /** Base directory for security checks (plugins must be within this directory) */
  baseDir?: string;
  /** Directory to resolve node_modules from (default: process.cwd()) */
  resolveDir?: string;
}

/**
 * Error codes for compilation failures
 */
export type CompilationErrorCode =
  | 'FILE_NOT_FOUND'
  | 'FILE_NOT_READABLE'
  | 'SIZE_LIMIT'
  | 'COMPILE_ERROR'
  | 'TIMEOUT'
  | 'PATH_TRAVERSAL'
  | 'UNKNOWN';

/**
 * Error information from batch compilation
 */
export interface CompilationError {
  /** Path to the plugin that failed */
  pluginPath: string;
  /** Error message (sanitized) */
  message: string;
  /** Error code for categorization */
  code: CompilationErrorCode;
}

/**
 * Result of batch compilation
 */
export interface BatchCompilationResult {
  /** Successfully compiled plugins */
  results: Map<string, CompilationResult>;
  /** Compilation errors */
  errors: CompilationError[];
}

const DEFAULT_OPTIONS: Required<Omit<CompilerOptions, 'timeout' | 'baseDir' | 'resolveDir'>> & {
  timeout: number;
} = {
  target: 'es2022',
  sourcemap: false,
  minify: false,
  timeout: DEFAULT_COMPILE_TIMEOUT_MS,
};

/**
 * Valid error codes for type checking
 */
const VALID_ERROR_CODES: readonly CompilationErrorCode[] = [
  'FILE_NOT_FOUND',
  'FILE_NOT_READABLE',
  'SIZE_LIMIT',
  'COMPILE_ERROR',
  'TIMEOUT',
  'PATH_TRAVERSAL',
  'UNKNOWN',
];

/**
 * Normalize an error code to a valid CompilationErrorCode
 */
function normalizeErrorCode(code: unknown): CompilationErrorCode {
  if (typeof code === 'string' && VALID_ERROR_CODES.includes(code as CompilationErrorCode)) {
    return code as CompilationErrorCode;
  }
  return 'UNKNOWN';
}

/**
 * Sanitize file path for safe error messages.
 * Replaces absolute paths with relative alternatives for cleaner, portable error messages.
 *
 * @param filePath - The file path to sanitize
 * @returns Sanitized path safe for error messages
 */
function sanitizePath(filePath: string): string {
  // Replace current working directory with '.' for relative paths
  const cwd = process.cwd();
  if (filePath.startsWith(cwd)) {
    return '.' + filePath.slice(cwd.length);
  }

  // If path is outside CWD, return as-is (already absolute or relative)
  return filePath;
}

/**
 * Validate that a path doesn't contain traversal attacks.
 *
 * @param pluginPath - Path to validate
 * @param basePath - Optional base path that the plugin must be within
 * @throws Error if path contains traversal sequences
 */
function validatePathSecurity(pluginPath: string, basePath?: string): void {
  // Normalize the path first
  const normalized = path.normalize(pluginPath);

  // Check if the NORMALIZED path still contains '..' (actual traversal)
  const parts = normalized.split(path.sep);
  if (parts.includes('..')) {
    throw Object.assign(new Error('Path traversal not allowed'), {
      code: 'PATH_TRAVERSAL',
    });
  }

  // If base path specified, ensure plugin is within it
  if (basePath) {
    const absolutePlugin = path.isAbsolute(normalized)
      ? normalized
      : path.resolve(basePath, normalized);
    const absoluteBase = path.resolve(basePath);

    // Ensure plugin path starts with base path (with separator to avoid partial matches)
    if (!absolutePlugin.startsWith(absoluteBase + path.sep) && absolutePlugin !== absoluteBase) {
      throw Object.assign(new Error('Plugin path must be within allowed directory'), {
        code: 'PATH_TRAVERSAL',
      });
    }
  }
}

/**
 * Validate source code input.
 *
 * @param sourceCode - Source code to validate
 * @param sourcePath - Path for error messages
 * @throws Error if validation fails
 */
function validateSourceCode(sourceCode: string, sourcePath: string): void {
  if (typeof sourceCode !== 'string') {
    throw Object.assign(new Error(`Invalid source code type for ${sanitizePath(sourcePath)}`), {
      code: 'COMPILE_ERROR',
    });
  }

  if (sourceCode.length === 0) {
    throw Object.assign(new Error(`Source code is empty for ${sanitizePath(sourcePath)}`), {
      code: 'COMPILE_ERROR',
    });
  }

  if (sourceCode.length > MAX_SOURCE_SIZE) {
    throw Object.assign(
      new Error(
        `Source code exceeds size limit (${(sourceCode.length / 1024 / 1024).toFixed(1)}MB > ${MAX_SOURCE_SIZE / 1024 / 1024}MB)`
      ),
      { code: 'SIZE_LIMIT' }
    );
  }
}

/**
 * Validate compiled code output.
 *
 * @param code - Compiled code to validate
 * @throws Error if validation fails
 */
function validateCompiledCode(code: string): void {
  if (code.length > MAX_COMPILED_SIZE) {
    throw Object.assign(
      new Error(
        `Compiled code exceeds size limit (${(code.length / 1024 / 1024).toFixed(1)}MB > ${MAX_COMPILED_SIZE / 1024 / 1024}MB)`
      ),
      { code: 'SIZE_LIMIT' }
    );
  }
}

/**
 * Check if a file exists and is readable.
 *
 * @param filePath - Path to check
 * @throws Error with specific code if file is not accessible
 */
async function validateFileAccess(filePath: string): Promise<void> {
  try {
    await fs.promises.access(filePath, fs.constants.R_OK);
  } catch (err) {
    const error = err as NodeJS.ErrnoException;
    if (error.code === 'ENOENT') {
      throw Object.assign(new Error(`File not found: ${sanitizePath(filePath)}`), {
        code: 'FILE_NOT_FOUND',
      });
    }
    throw Object.assign(new Error(`File not readable: ${sanitizePath(filePath)}`), {
      code: 'FILE_NOT_READABLE',
    });
  }
}

/**
 * Run a promise with a timeout.
 *
 * @param promise - Promise to run
 * @param timeoutMs - Timeout in milliseconds
 * @param timeoutMessage - Error message on timeout
 * @returns Promise result
 */
async function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  timeoutMessage: string
): Promise<T> {
  let timeoutId: NodeJS.Timeout | undefined;

  const timeoutPromise = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(() => {
      reject(Object.assign(new Error(timeoutMessage), { code: 'TIMEOUT' }));
    }, timeoutMs);
  });

  try {
    const result = await Promise.race([promise, timeoutPromise]);
    clearTimeout(timeoutId!);
    return result;
  } catch (err) {
    clearTimeout(timeoutId!);
    throw err;
  }
}

/**
 * Format an esbuild warning/error message
 */
function formatMessage(msg: Message, defaultPath: string): string {
  const file = msg.location?.file
    ? sanitizePath(msg.location.file)
    : sanitizePath(defaultPath);
  const line = msg.location?.line ?? 0;
  const column = msg.location?.column ?? 0;
  return `${file}:${line}:${column}: ${msg.text}`;
}

/**
 * Compiles a TypeScript plugin file to JavaScript using esbuild with bundling.
 * All imports are resolved and bundled into the output for self-contained execution.
 *
 * @param pluginPath - Path to the TypeScript plugin file
 * @param options - Compilation options
 * @returns Compiled JavaScript code and metadata
 * @throws Error if compilation fails
 *
 * @example
 * ```typescript
 * const result = await compilePlugin('plugins/my-plugin.ts');
 * console.log(result.code); // Compiled and bundled JavaScript
 * ```
 */
export async function compilePlugin(
  pluginPath: string,
  options: CompilerOptions = {}
): Promise<CompilationResult> {
  const opts = { ...DEFAULT_OPTIONS, ...options };

  // Security: validate path with base directory
  const baseDir = opts.baseDir ?? DEFAULT_PLUGIN_BASE_DIR;
  validatePathSecurity(pluginPath, baseDir);

  // Resolve to absolute path
  const absolutePath = path.isAbsolute(pluginPath)
    ? pluginPath
    : path.resolve(process.cwd(), pluginPath);

  // Validate file exists and is readable
  await validateFileAccess(absolutePath);

  // Read source with size check
  const stats = await fs.promises.stat(absolutePath);
  if (stats.size > MAX_SOURCE_SIZE) {
    throw Object.assign(
      new Error(
        `Source file exceeds size limit (${(stats.size / 1024 / 1024).toFixed(1)}MB > ${MAX_SOURCE_SIZE / 1024 / 1024}MB)`
      ),
      { code: 'SIZE_LIMIT' }
    );
  }

  const sourceCode = await fs.promises.readFile(absolutePath, 'utf-8');

  // Use the file's directory as resolveDir for proper import resolution
  const resolveDir = opts.resolveDir ?? path.dirname(absolutePath);

  return compilePluginSource(sourceCode, pluginPath, { ...opts, resolveDir });
}

/**
 * Compiles TypeScript source code to JavaScript with bundling.
 * This variant accepts source code directly (useful when code is already in memory).
 * All imports are resolved and bundled into the output.
 *
 * @param sourceCode - TypeScript source code
 * @param sourcePath - Original path (for error messages and source maps)
 * @param options - Compilation options
 * @returns Compiled JavaScript code and metadata
 * @throws Error if compilation fails
 *
 * @example
 * ```typescript
 * const source = `
 *   import { ethers } from 'ethers';
 *   export async function handler({ api }) {
 *     return ethers.utils.formatEther('1000000000000000000');
 *   }
 * `;
 * const result = await compilePluginSource(source, 'inline-plugin.ts');
 * // result.code contains bundled code with ethers included
 * ```
 */
export async function compilePluginSource(
  sourceCode: string,
  sourcePath: string,
  options: CompilerOptions = {}
): Promise<CompilationResult> {
  const opts = { ...DEFAULT_OPTIONS, ...options };

  // Validate input
  validateSourceCode(sourceCode, sourcePath);

  const doCompile = async (): Promise<CompilationResult> => {
    try {
      // Use esbuild.build() with stdin to bundle imports
      const result = await esbuild.build({
        stdin: {
          contents: sourceCode,
          loader: 'ts',
          sourcefile: sourcePath,
          resolveDir: opts.resolveDir ?? process.cwd(),
        },
        bundle: true, // CRITICAL: Bundle to resolve and inline all imports
        platform: 'node',
        target: opts.target,
        format: 'cjs', // CommonJS for Function constructor compatibility (executed via new Function())
        sourcemap: opts.sourcemap === 'external' ? true : opts.sourcemap === 'inline' ? 'inline' : false,
        minify: opts.minify,
        write: false, // Keep in memory, don't write to disk
        external: [
          // Don't bundle Node.js built-ins - they're available in the sandbox
          'node:*',
          // The SDK is already available in the sandbox
          '@openzeppelin/relayer-sdk',
        ],
        logLevel: 'silent', // We handle errors ourselves
      });

      // Validate we got output
      if (!result.outputFiles || result.outputFiles.length === 0) {
        throw Object.assign(new Error('esbuild produced no output'), {
          code: 'COMPILE_ERROR',
        });
      }

      const code = result.outputFiles[0].text;

      // Validate output size
      validateCompiledCode(code);

      // Format warnings
      const warnings = result.warnings.map((w) => formatMessage(w, sourcePath));

      // Handle source map based on configuration
      let sourceMap: string | undefined;
      if (opts.sourcemap === 'external' && result.outputFiles.length > 1) {
        sourceMap = result.outputFiles[1].text;
      }
      // For 'inline', the source map is embedded in the code

      return {
        code,
        sourceMap,
        warnings,
      };
    } catch (error) {
      if (error instanceof Error) {
        // Check if it's already one of our errors
        if ((error as any).code && VALID_ERROR_CODES.includes((error as any).code)) {
          throw error;
        }

        // Sanitize error message - replace absolute paths with relative/safe versions
        const sanitizedMessage = sanitizePath(error.message);

        throw Object.assign(new Error(`Compilation failed: ${sanitizedMessage}`), {
          code: 'COMPILE_ERROR',
        });
      }
      throw error;
    }
  };

  // Run with timeout
  return withTimeout(
    doCompile(),
    opts.timeout ?? DEFAULT_COMPILE_TIMEOUT_MS,
    `Compilation timed out after ${opts.timeout ?? DEFAULT_COMPILE_TIMEOUT_MS}ms`
  );
}

/**
 * Batch compile multiple plugins with concurrency limits.
 * Returns both successful results and detailed error information.
 *
 * @param pluginPaths - Array of plugin file paths
 * @param options - Compilation options
 * @returns Map of successful results and array of errors
 *
 * @example
 * ```typescript
 * const { results, errors } = await compilePlugins(['a.ts', 'b.ts', 'c.ts']);
 *
 * // Handle successes
 * for (const [path, result] of results) {
 *   console.log(`Compiled ${path}: ${result.code.length} bytes`);
 * }
 *
 * // Handle errors
 * for (const err of errors) {
 *   console.error(`${err.pluginPath}: [${err.code}] ${err.message}`);
 * }
 * ```
 */
export async function compilePlugins(
  pluginPaths: string[],
  options: CompilerOptions = {}
): Promise<BatchCompilationResult> {
  const results = new Map<string, CompilationResult>();
  const errors: CompilationError[] = [];

  // Process in batches to limit concurrency
  for (let i = 0; i < pluginPaths.length; i += MAX_CONCURRENT_COMPILATIONS) {
    const batch = pluginPaths.slice(i, i + MAX_CONCURRENT_COMPILATIONS);

    const compilations = await Promise.allSettled(
      batch.map(async (pluginPath) => {
        const result = await compilePlugin(pluginPath, options);
        return { pluginPath, result };
      })
    );

    // Process results with defensive checks
    for (let j = 0; j < batch.length; j++) {
      const pluginPath = batch[j];
      const compilation = compilations[j];

      if (!compilation) {
        // Should never happen, but defensive programming
        errors.push({
          pluginPath,
          message: 'Compilation result missing',
          code: 'UNKNOWN',
        });
        continue;
      }

      if (compilation.status === 'fulfilled') {
        results.set(compilation.value.pluginPath, compilation.value.result);
      } else {
        const error = compilation.reason as Error & { code?: string };
        errors.push({
          pluginPath,
          message: error.message || 'Unknown error',
          code: normalizeErrorCode(error.code),
        });
      }
    }
  }

  return { results, errors };
}

/**
 * Creates a wrapped version of compiled code that exports the handler.
 *
 * @deprecated This function is no longer used internally. The compiled code is executed
 * directly via `new Function()` constructor in direct-executor.ts. This function is kept
 * for backward compatibility but may be removed in a future version.
 *
 * @param compiledCode - The compiled JavaScript code
 * @returns Wrapped code as an IIFE
 * @throws Error if input is invalid
 *
 * @example
 * ```typescript
 * const compiled = await compilePluginSource(source, 'plugin.ts');
 * const wrapped = wrapForVm(compiled.code);
 * // Note: This is legacy code. Current execution uses new Function() directly.
 * ```
 */
export function wrapForVm(compiledCode: string): string {
  // Validate input
  if (typeof compiledCode !== 'string') {
    throw new Error('wrapForVm: compiledCode must be a string');
  }

  if (compiledCode.length === 0) {
    throw new Error('wrapForVm: compiledCode cannot be empty');
  }

  if (compiledCode.length > MAX_COMPILED_SIZE) {
    throw new Error(
      `wrapForVm: code exceeds size limit (${(compiledCode.length / 1024 / 1024).toFixed(1)}MB)`
    );
  }

  // The compiled code uses CommonJS (module.exports / exports.handler)
  // We wrap it in an IIFE for legacy vm.Script compatibility
  return `
(function(exports, require, module, __filename, __dirname) {
${compiledCode}
});
`;
}
