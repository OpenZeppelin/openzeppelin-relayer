#!/usr/bin/env ts-node
/**
 * Build script to pre-compile sandbox-executor.ts
 * 
 * Run this during build/release to generate sandbox-executor.js
 * This avoids any runtime compilation overhead.
 * 
 * Usage: npx ts-node build-executor.ts
 */

import * as esbuild from 'esbuild';
import * as path from 'node:path';
import * as fs from 'node:fs';

async function build() {
  const inputPath = path.resolve(__dirname, 'sandbox-executor.ts');
  const outputPath = path.resolve(__dirname, 'sandbox-executor.js');

  console.log('Compiling sandbox-executor.ts...');

  const result = await esbuild.build({
    entryPoints: [inputPath],
    bundle: true,
    platform: 'node',
    target: 'node18',
    format: 'cjs',
    sourcemap: false,
    write: true,
    outfile: outputPath,
    loader: { '.ts': 'ts' },
    external: ['node:*'],
  });

  if (result.errors.length > 0) {
    console.error('Build failed:', result.errors);
    process.exit(1);
  }

  const stats = fs.statSync(outputPath);
  console.log(`✓ Compiled to ${outputPath} (${(stats.size / 1024).toFixed(1)} KB)`);
}

build().catch((err) => {
  console.error('Build failed:', err);
  process.exit(1);
});
