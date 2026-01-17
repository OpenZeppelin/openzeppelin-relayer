#!/usr/bin/env ts-node
/**
 * Build script to pre-compile direct-executor.ts
 *
 * Run this during build/release to generate direct-executor.js
 * This avoids any runtime compilation overhead.
 *
 * Usage: npx ts-node build-executor.ts
 *        npx ts-node build-executor.ts --force  (skip cache check)
 *
 * Features:
 * - Input validation
 * - Output verification (syntax check without execution)
 * - SHA256 integrity hash (saved to .sha256 file)
 * - Build metadata banner
 * - Atomic writes (temp file + rename)
 * - Build caching (skip if input unchanged)
 * - Clean up on failure
 */

import * as path from 'node:path';
import * as fs from 'node:fs';
import * as crypto from 'node:crypto';
import * as vm from 'node:vm';
import * as os from 'node:os';

const inputPath = path.resolve(__dirname, 'direct-executor.ts');
const outputPath = path.resolve(__dirname, 'direct-executor.js');
const hashPath = path.resolve(__dirname, 'direct-executor.js.sha256');
const cacheHashPath = path.resolve(__dirname, '.build-cache-hash');

/**
 * Check if esbuild is available
 */
async function validateDependencies(): Promise<typeof import('esbuild')> {
  try {
    return await import('esbuild');
  } catch {
    throw new Error(
      'esbuild is not installed. Run: npm install esbuild\n' +
      'Or if using yarn: yarn add esbuild'
    );
  }
}

/**
 * Calculate SHA256 hash of file content
 */
function calculateHash(filePath: string): string {
  const content = fs.readFileSync(filePath);
  return crypto.createHash('sha256').update(content).digest('hex');
}

/**
 * Calculate SHA256 hash of string content
 */
function calculateContentHash(content: string): string {
  return crypto.createHash('sha256').update(content).digest('hex');
}

/**
 * Safe file deletion - handles EBUSY, ENOENT gracefully
 */
function safeUnlink(filePath: string): boolean {
  try {
    if (fs.existsSync(filePath)) {
      fs.unlinkSync(filePath);
      return true;
    }
  } catch (err) {
    const error = err as NodeJS.ErrnoException;
    // Ignore common non-critical errors
    if (error.code === 'ENOENT') return false; // File doesn't exist
    if (error.code === 'EBUSY') {
      console.warn(`  ⚠ File busy, skipping cleanup: ${filePath}`);
      return false;
    }
    if (error.code === 'EPERM') {
      console.warn(`  ⚠ Permission denied, skipping cleanup: ${filePath}`);
      return false;
    }
    // Re-throw unexpected errors
    throw err;
  }
  return false;
}

/**
 * Clean up partial output files on failure
 */
function cleanup(tempPath?: string): void {
  console.log('Cleaning up...');
  if (tempPath) safeUnlink(tempPath);
  safeUnlink(outputPath);
  safeUnlink(hashPath);
}

/**
 * Validate input file exists and is readable
 */
function validateInput(): void {
  if (!fs.existsSync(inputPath)) {
    throw new Error(`Input file not found: ${inputPath}`);
  }

  const stats = fs.statSync(inputPath);
  if (!stats.isFile()) {
    throw new Error(`Input path is not a file: ${inputPath}`);
  }

  if (stats.size === 0) {
    throw new Error(`Input file is empty: ${inputPath}`);
  }

  try {
    fs.accessSync(inputPath, fs.constants.R_OK);
  } catch {
    throw new Error(`Input file is not readable: ${inputPath}`);
  }
}

/**
 * Check if build can be skipped (input unchanged)
 */
function checkBuildCache(inputHash: string, force: boolean): boolean {
  if (force) {
    console.log('  → Force flag set, skipping cache check');
    return false;
  }

  // Check if output exists
  if (!fs.existsSync(outputPath)) {
    console.log('  → Output file missing, rebuild required');
    return false;
  }

  // Check if hash file exists
  if (!fs.existsSync(cacheHashPath)) {
    console.log('  → Cache hash missing, rebuild required');
    return false;
  }

  // Compare input hash with cached hash
  try {
    const cachedHash = fs.readFileSync(cacheHashPath, 'utf-8').trim();
    if (cachedHash === inputHash) {
      return true; // Cache hit - no rebuild needed
    }
    console.log('  → Input changed, rebuild required');
  } catch {
    console.log('  → Cache read failed, rebuild required');
  }

  return false;
}

/**
 * Verify output file has valid JavaScript syntax (without executing)
 */
function verifySyntax(filePath: string): void {
  const content = fs.readFileSync(filePath, 'utf-8');

  try {
    // Use vm.Script to parse without executing
    // This catches syntax errors without side effects
    new vm.Script(content, { filename: filePath });
  } catch (err) {
    const error = err as Error;
    throw new Error(`Output has invalid JavaScript syntax: ${error.message}`);
  }
}

/**
 * Verify output file is valid
 */
function verifyOutput(filePath: string): void {
  if (!fs.existsSync(filePath)) {
    throw new Error(`Output file was not created: ${filePath}`);
  }

  const stats = fs.statSync(filePath);
  if (stats.size === 0) {
    throw new Error(`Output file is empty: ${filePath}`);
  }

  // Syntax check without execution (no side effects)
  verifySyntax(filePath);
}

/**
 * Atomic file write: write to temp, then rename
 */
function atomicWriteFile(targetPath: string, content: string): string {
  const tempPath = path.join(
    os.tmpdir(),
    `build-executor-${crypto.randomUUID()}.tmp`
  );

  fs.writeFileSync(tempPath, content, 'utf-8');
  fs.renameSync(tempPath, targetPath);

  return tempPath;
}

/**
 * Write hash to .sha256 file for runtime verification
 */
function writeHashFile(hash: string): void {
  const content = `${hash}  direct-executor.js\n`;
  atomicWriteFile(hashPath, content);
}

/**
 * Save input hash for build caching
 */
function saveBuildCache(inputHash: string): void {
  atomicWriteFile(cacheHashPath, inputHash);
}

/**
 * Generate build metadata banner
 */
function generateBanner(inputHash: string): string {
  const now = new Date().toISOString();
  const nodeVersion = process.version;
  return `/**
 * Auto-generated by build-executor.ts
 * Build time: ${now}
 * Node version: ${nodeVersion}
 * Source: direct-executor.ts
 * Source hash: ${inputHash.substring(0, 16)}
 * DO NOT EDIT - Regenerate with: npx ts-node build-executor.ts
 */`;
}

async function build(): Promise<void> {
  const forceRebuild = process.argv.includes('--force');
  let tempOutputPath: string | undefined;

  console.log('=== Building direct-executor ===');
  console.log(`Input:  ${inputPath}`);
  console.log(`Output: ${outputPath}`);

  // Step 1: Validate dependencies
  console.log('\n[1/7] Checking dependencies...');
  const esbuild = await validateDependencies();
  console.log('  ✓ esbuild available');

  // Step 2: Validate input
  console.log('\n[2/7] Validating input file...');
  validateInput();
  const inputHash = calculateHash(inputPath);
  console.log(`  ✓ Input valid (SHA256: ${inputHash.substring(0, 16)}...)`);

  // Step 3: Check build cache
  console.log('\n[3/7] Checking build cache...');
  if (checkBuildCache(inputHash, forceRebuild)) {
    console.log('  ✓ Build cache valid - skipping rebuild');
    console.log('\n=== Build skipped (up to date) ===');
    return;
  }

  // Step 4: Build to temp file (atomic write preparation)
  console.log('\n[4/7] Compiling TypeScript...');
  tempOutputPath = path.join(os.tmpdir(), `direct-executor-${crypto.randomUUID()}.js`);

  const result = await esbuild.build({
    entryPoints: [inputPath],
    bundle: true,
    platform: 'node',
    target: 'node18',
    format: 'cjs',
    sourcemap: false,
    write: true,
    outfile: tempOutputPath,
    loader: { '.ts': 'ts' },
    external: ['node:*'],
    banner: {
      js: generateBanner(inputHash),
    },
  });

  // Handle errors
  if (result.errors.length > 0) {
    console.error('\n❌ Build errors:');
    for (const error of result.errors) {
      console.error(`  - ${error.text}`);
      if (error.location) {
        console.error(`    at ${error.location.file}:${error.location.line}:${error.location.column}`);
      }
    }
    cleanup(tempOutputPath);
    process.exit(1);
  }

  // Handle warnings
  if (result.warnings.length > 0) {
    console.log('\n⚠️  Build warnings:');
    for (const warning of result.warnings) {
      console.log(`  - ${warning.text}`);
      if (warning.location) {
        console.log(`    at ${warning.location.file}:${warning.location.line}:${warning.location.column}`);
      }
    }
  }

  console.log('  ✓ Compilation successful');

  // Step 5: Verify output (syntax check, no execution)
  console.log('\n[5/7] Verifying output syntax...');
  verifyOutput(tempOutputPath);
  console.log('  ✓ Output has valid JavaScript syntax');

  // Step 6: Atomic move to final location
  console.log('\n[6/7] Finalizing output...');
  fs.renameSync(tempOutputPath, outputPath);
  tempOutputPath = undefined; // Clear so cleanup doesn't try to delete
  console.log('  ✓ Output written atomically');

  // Step 7: Write hash files and cache
  console.log('\n[7/7] Writing integrity hash...');
  const outputHash = calculateHash(outputPath);
  writeHashFile(outputHash);
  saveBuildCache(inputHash);
  console.log(`  ✓ SHA256: ${outputHash}`);
  console.log(`  ✓ Hash saved to: ${hashPath}`);

  // Print summary
  const stats = fs.statSync(outputPath);
  console.log('\n=== Build complete! ===');
  console.log('─'.repeat(50));
  console.log(`  Output:     ${outputPath}`);
  console.log(`  Hash file:  ${hashPath}`);
  console.log(`  Size:       ${(stats.size / 1024).toFixed(1)} KB`);
  console.log(`  Input hash: ${inputHash.substring(0, 16)}...`);
  console.log(`  Output hash: ${outputHash.substring(0, 16)}...`);
  console.log('─'.repeat(50));
}

// Run build with proper error handling
build().catch((err) => {
  const error = err as Error;
  console.error(`\n❌ Build failed: ${error.message}`);
  if (error.stack) {
    console.error('\nStack trace:');
    console.error(error.stack.split('\n').slice(1, 5).join('\n'));
  }
  cleanup();
  process.exit(1);
});
