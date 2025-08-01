#!/usr/bin/env node

/**
 * Wrapper script for executing user plugins
 * 
 * This script is called by the relayer to execute user plugins.
 * It loads the user's plugin script and calls their exported 'handler' function.
 * 
 * Usage: ts-node wrapper.ts <socket_path> <params_json> <user_script_path>
 */

import { runUserPlugin } from './plugin';

// Entry point for wrapper execution
runUserPlugin();