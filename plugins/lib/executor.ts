#!/usr/bin/env node

/**
 * Plugin executor script for executing user plugins
 * 
 * This is the main entry point for executing specific plugins from the Rust environment.
 * It serves as a bridge between the Rust relayer and TypeScript plugin ecosystem.
 * 
 * Called from: src/services/plugins/script_executor.rs
 * The Rust code invokes this script via ts-node and passes parameters as command line arguments.
 * 
 * This script:
 * 1. Receives plugin execution parameters from Rust via process.argv
 * 2. Loads the user's plugin script dynamically 
 * 3. Calls the plugin's exported 'handler' function
 * 4. Returns results back to the Rust environment
 * 
 * Usage: ts-node executor.ts <socket_path> <params_json> <user_script_path>
 * 
 * Arguments:
 * - socket_path: Unix socket path for communication with relayer
 * - params_json: JSON string containing plugin parameters 
 * - user_script_path: Path to the user's plugin file to execute
 */

import { runUserPlugin } from './plugin';

/**
 * Extract and validate CLI arguments passed from Rust script_executor.rs
 */
function extractCliArguments() {
  // Get arguments: [node, executor.ts, socketPath, paramsJson, userScriptPath]
  const socketPath = process.argv[2];
  const paramsJson = process.argv[3];
  const userScriptPath = process.argv[4];
  
  // Validate required arguments
  if (!socketPath) {
    throw new Error("Socket path is required (argument 1)");
  }
  
  if (!paramsJson) {
    throw new Error("Plugin parameters JSON is required (argument 2)");
  }
  
  if (!userScriptPath) {
    throw new Error("User script path is required (argument 3)");
  }
  
  return { socketPath, paramsJson, userScriptPath };
}

/**
 * Parse and validate plugin parameters
 */
function parsePluginParameters<T = any>(paramsJson: string): T {
  try {
    return JSON.parse(paramsJson) as T;
  } catch (error) {
    throw new Error(`Failed to parse plugin parameters JSON: ${error instanceof Error ? error.message : error}`);
  }
}

/**
 * Main executor logic
 */
async function main(): Promise<void> {
  try {
    // Extract and validate CLI arguments
    const { socketPath, paramsJson, userScriptPath } = extractCliArguments();
    
    // Parse plugin parameters
    const pluginParams = parsePluginParameters(paramsJson);
    
    // Execute plugin with validated parameters
    await runUserPlugin(socketPath, pluginParams, userScriptPath);
  } catch (error) {
    console.error("Plugin executor failed:", error instanceof Error ? error.message : error);
    process.exit(1);
  }
}

// Entry point for executor
main();