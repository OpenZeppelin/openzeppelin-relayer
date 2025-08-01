import fs from 'fs';
import path from 'path';
import { Networks } from '@stellar/stellar-sdk';

export interface LaunchtubeConfig {
  fundRelayerId: string;
  sequenceRelayerIds: string[];
  maxFee: number;
  network: 'testnet' | 'mainnet';
  rpcUrl: string;
}

/**
 * Load configuration from config.json
 */
export function loadConfig(): LaunchtubeConfig {
  // Try config file first
  const configPath = path.join(__dirname, '..', 'config.json');

  if (!fs.existsSync(configPath)) {
    throw new Error(`Configuration file not found: ${configPath}`);
  }

  try {
    const configData = fs.readFileSync(configPath, 'utf8');
    const config = JSON.parse(configData);

    // Validate required fields
    if (!config.fundRelayerId) {
      throw new Error('Missing fundRelayerId in config');
    }

    if (
      !config.sequenceRelayerIds ||
      !Array.isArray(config.sequenceRelayerIds) ||
      config.sequenceRelayerIds.length === 0
    ) {
      throw new Error('Missing or empty sequenceRelayerIds in config');
    }

    if (!config.network) {
      throw new Error('Missing network in config');
    }

    if (!config.rpcUrl) {
      throw new Error('Missing rpcUrl in config');
    }

    if (!config.maxFee) {
      throw new Error('Missing maxFee in config');
    }

    return {
      fundRelayerId: config.fundRelayerId,
      sequenceRelayerIds: config.sequenceRelayerIds,
      maxFee: config.maxFee,
      network: config.network,
      rpcUrl: config.rpcUrl,
    };
  } catch (error) {
    if (error instanceof SyntaxError) {
      throw new Error(`Invalid JSON in config file: ${error.message}`);
    }
    throw error;
  }
}

/**
 * Get the network passphrase based on the configuration
 */
export function getNetworkPassphrase(network: 'testnet' | 'mainnet'): string {
  return network === 'mainnet' ? Networks.PUBLIC : Networks.TESTNET;
}
