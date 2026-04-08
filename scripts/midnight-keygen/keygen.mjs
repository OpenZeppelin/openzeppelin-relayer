#!/usr/bin/env node
/**
 * Midnight Wallet Key Derivation Script
 *
 * Derives bech32m addresses and viewing key from a 32-byte hex seed.
 *
 * Usage:
 *   node keygen.mjs <seed-hex>
 *   node keygen.mjs --random
 *   node keygen.mjs --random --network preview
 */

import { randomBytes } from 'crypto';
import { bech32m } from 'bech32';
import { SecretKeys, signatureVerifyingKey } from '@midnight-ntwrk/ledger';

const args = process.argv.slice(2);
let seedHex;
let network = 'preview';

for (let i = 0; i < args.length; i++) {
  if (args[i] === '--random') seedHex = randomBytes(32).toString('hex');
  else if (args[i] === '--env') { seedHex = process.env[args[++i]]; if (!seedHex) { console.error(`Env var not set`); process.exit(1); } }
  else if (args[i] === '--network') network = args[++i];
  else if (!args[i].startsWith('--')) seedHex = args[i];
}

if (!seedHex || !/^[0-9a-fA-F]{64}$/.test(seedHex)) {
  console.error(`Usage: node keygen.mjs <64-hex-char-seed> | --random [--network preview|preprod|mainnet]`);
  process.exit(1);
}

const HRP = {
  preview: { addr: 'mn_addr_preview', shielded: 'mn_shield-addr_preview', esk: 'mn_shield-esk_preview' },
  preprod: { addr: 'mn_addr_preprod', shielded: 'mn_shield-addr_preprod', esk: 'mn_shield-esk_preprod' },
  mainnet: { addr: 'mn_addr',         shielded: 'mn_shield-addr',         esk: 'mn_shield-esk' },
  devnet:  { addr: 'mn_addr_dev',     shielded: 'mn_shield-addr_dev',     esk: 'mn_shield-esk_dev' },
};

const hrp = HRP[network];
if (!hrp) { console.error(`Unknown network: ${network}`); process.exit(1); }

const seed = new Uint8Array(Buffer.from(seedHex, 'hex'));
const keys = SecretKeys.fromSeed(seed);

// Shielded address (from coin public key)
const coinPubBytes = Buffer.from(keys.coinPublicKey, 'hex');
const shieldedAddress = bech32m.encode(hrp.shielded, bech32m.toWords(coinPubBytes), 256);

// Unshielded address (from signature verifying key)
// SecretKeys.fromSeed produces v0.2 keys; signatureVerifyingKey needs v1.0
// Convert: v0.2 = 00 02 00 + 32 raw bytes → v1.0 = 01 00 + 32 raw bytes
const cskBytes = keys.coinSecretKey.yesIKnowTheSecurityImplicationsOfThis_serialize();
const rawSigningKey = cskBytes.slice(3); // skip 3-byte v0.2 header
const v1SigningKeyHex = '0100' + Buffer.from(rawSigningKey).toString('hex');
const verifyingKeyHex = signatureVerifyingKey(v1SigningKeyHex);
const vkRawBytes = Buffer.from(verifyingKeyHex.slice(4), 'hex'); // skip 0100 version prefix
const unshieldedAddress = bech32m.encode(hrp.addr, bech32m.toWords(vkRawBytes), 256);

// Viewing key (encryption secret key)
const eskBytes = keys.encryptionSecretKey.yesIKnowTheSecurityImplicationsOfThis_serialize();
const viewingKey = bech32m.encode(hrp.esk, bech32m.toWords(eskBytes), 256);

console.log(JSON.stringify({
  seed: seedHex,
  unshielded_address: unshieldedAddress,
  shielded_address: shieldedAddress,
  viewing_key: viewingKey,
  network,
}, null, 2));

console.error(`
# Set these environment variables for the relayer:
export MIDNIGHT_ADDRESS="${unshieldedAddress}"
export MIDNIGHT_VIEWING_KEY="${viewingKey}"

# The unshielded address is what you fund from Lace (for tDUST).
# The shielded address is for private token transfers.`);
