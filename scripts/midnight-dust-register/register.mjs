#!/usr/bin/env node
/**
 * Midnight DUST Registration Script (standalone, non-interactive)
 *
 * Registers a relayer's unshielded NIGHT UTXOs for DUST generation so the
 * wallet starts accruing DUST that subsequent fee payments can spend. This
 * is a Midnight-side transaction signed by the wallet's own unshielded key —
 * no Cardano or Lace involvement required.
 *
 * Adapted from the Midnight tutorial "Generating DUST Programmatically"
 * (docs.midnight.network/guides/generating-dust-programmatically), trimmed
 * to a non-interactive manual-invocation shape: the seed comes from a flag
 * or the relayer's keystore, and the script exits on success (or timeout).
 *
 * Usage:
 *   node register.mjs --seed-hex <64-hex> [--network preview]
 *   node register.mjs --keystore-path <path> --passphrase <pass> [--network preview]
 *
 * Optional overrides (defaults target preview testnet):
 *   --rpc-url         default: https://rpc.preview.midnight.network
 *   --indexer-http    default: https://indexer.preview.midnight.network/api/v4/graphql
 *   --indexer-ws      default: wss://indexer.preview.midnight.network/api/v4/graphql/ws
 *   --proof-server    default: http://localhost:6300
 *   --wait-secs       default: 180   (how long to poll for DUST balance after submit)
 */

import crypto from 'node:crypto';
import fs from 'node:fs';
import process from 'node:process';
import { keccak_256 } from '@noble/hashes/sha3';
import { WebSocket } from 'ws';
globalThis.WebSocket ??= WebSocket;

import * as Rx from 'rxjs';
import { HDWallet, Roles } from '@midnight-ntwrk/wallet-sdk-hd';
import * as ledger from '@midnight-ntwrk/ledger-v8';
import { unshieldedToken } from '@midnight-ntwrk/ledger-v8';
import { WalletFacade } from '@midnight-ntwrk/wallet-sdk-facade';
import { ShieldedWallet } from '@midnight-ntwrk/wallet-sdk-shielded';
import { DustWallet } from '@midnight-ntwrk/wallet-sdk-dust-wallet';
import {
  createKeystore as createUnshieldedKeystore,
  InMemoryTransactionHistoryStorage,
  PublicKey,
  UnshieldedWallet,
} from '@midnight-ntwrk/wallet-sdk-unshielded-wallet';
import { setNetworkId, getNetworkId } from '@midnight-ntwrk/midnight-js-network-id';
import {
  DustAddress,
  MidnightBech32m,
  ShieldedAddress,
  ShieldedCoinPublicKey,
  ShieldedEncryptionPublicKey,
} from '@midnight-ntwrk/wallet-sdk-address-format';

// ---------------------------------------------------------------------------
// Arg parsing + seed resolution
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const out = {
    network: 'preview',
    rpcUrl: 'https://rpc.preview.midnight.network',
    indexerHttp: 'https://indexer.preview.midnight.network/api/v4/graphql',
    indexerWs: 'wss://indexer.preview.midnight.network/api/v4/graphql/ws',
    proofServer: 'http://localhost:6300',
    waitSecs: 180,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => argv[++i];
    switch (a) {
      case '--seed-hex': out.seedHex = next(); break;
      case '--keystore-path': out.keystorePath = next(); break;
      case '--passphrase': out.passphrase = next(); break;
      case '--network': out.network = next(); break;
      case '--rpc-url': out.rpcUrl = next(); break;
      case '--indexer-http': out.indexerHttp = next(); break;
      case '--indexer-ws': out.indexerWs = next(); break;
      case '--proof-server': out.proofServer = next(); break;
      case '--wait-secs': out.waitSecs = Number(next()); break;
      case '-h': case '--help':
        console.error(fs.readFileSync(new URL(import.meta.url), 'utf8').split('\n').slice(1, 29).join('\n'));
        process.exit(0);
      default:
        console.error(`Unknown argument: ${a}`);
        process.exit(1);
    }
  }
  return out;
}

// Ethereum v3 keystore decoder (scrypt + aes-128-ctr) — matches the format
// used by the relayer's oz-keystore crate and the existing midnight-keygen
// test keystores.
function decodeKeystore(path, passphrase) {
  const ks = JSON.parse(fs.readFileSync(path, 'utf8'));
  if (ks.version !== 3) throw new Error(`Unsupported keystore version: ${ks.version}`);
  const { crypto: c } = ks;
  if (c.kdf !== 'scrypt') throw new Error(`Unsupported kdf: ${c.kdf}`);
  if (c.cipher !== 'aes-128-ctr') throw new Error(`Unsupported cipher: ${c.cipher}`);

  const { n: N, r, p, dklen, salt } = c.kdfparams;
  const dk = crypto.scryptSync(
    Buffer.from(passphrase, 'utf8'),
    Buffer.from(salt, 'hex'),
    dklen,
    { N, r, p, maxmem: 2 * N * r * 128 + (1 << 25) },
  );

  const ciphertext = Buffer.from(c.ciphertext, 'hex');
  // Ethereum v3 keystore uses Keccak-256 for the MAC (not NIST SHA-3-256).
  // Node's crypto doesn't ship keccak so we pull it from @noble/hashes.
  const macInput = Buffer.concat([dk.slice(16, 32), ciphertext]);
  const mac = Buffer.from(keccak_256(macInput)).toString('hex');
  if (mac !== c.mac) throw new Error('Keystore MAC mismatch (wrong passphrase?)');

  const iv = Buffer.from(c.cipherparams.iv, 'hex');
  const decipher = crypto.createDecipheriv('aes-128-ctr', dk.slice(0, 16), iv);
  const plaintext = Buffer.concat([decipher.update(ciphertext), decipher.final()]);
  if (plaintext.length !== 32) throw new Error(`Decoded key is ${plaintext.length} bytes, expected 32`);
  return plaintext;
}

function resolveSeed(args) {
  if (args.seedHex && args.keystorePath) {
    throw new Error('Pass only one of --seed-hex or --keystore-path');
  }
  if (args.seedHex) {
    const hex = args.seedHex.replace(/^0x/, '').trim();
    if (!/^[0-9a-fA-F]{64}$/.test(hex)) throw new Error('--seed-hex must be 64 hex chars');
    return Buffer.from(hex, 'hex');
  }
  if (args.keystorePath) {
    const pass = args.passphrase ?? process.env.KEYSTORE_PASSPHRASE;
    if (!pass) throw new Error('Missing passphrase (pass --passphrase or set KEYSTORE_PASSPHRASE)');
    return decodeKeystore(args.keystorePath, pass);
  }
  throw new Error('Missing key source: pass --seed-hex or --keystore-path');
}

// ---------------------------------------------------------------------------
// Wallet construction — mirrors the Midnight tutorial's buildWallet exactly,
// with the configuration parameterized instead of hard-coded to preprod.
// ---------------------------------------------------------------------------

function deriveKeys(seedBuffer) {
  const hd = HDWallet.fromSeed(seedBuffer);
  if (hd.type !== 'seedOk') {
    throw new Error(`HDWallet.fromSeed failed (type=${hd.type}) — is the seed a valid 32-byte buffer?`);
  }
  const result = hd.hdWallet
    .selectAccount(0)
    .selectRoles([Roles.Zswap, Roles.NightExternal, Roles.Dust])
    .deriveKeysAt(0);
  if (result.type !== 'keysDerived') {
    throw new Error(`HD derivation failed: ${result.type}`);
  }
  const keys = result.keys;
  hd.hdWallet.clear();
  return keys;
}

async function buildWallet(args, keys) {
  setNetworkId(args.network);
  const shieldedSecretKeys = ledger.ZswapSecretKeys.fromSeed(keys[Roles.Zswap]);
  const dustSecretKey = ledger.DustSecretKey.fromSeed(keys[Roles.Dust]);
  const unshieldedKeystore = createUnshieldedKeystore(keys[Roles.NightExternal], getNetworkId());

  const shieldedConfig = {
    networkId: getNetworkId(),
    indexerClientConnection: {
      indexerHttpUrl: args.indexerHttp,
      indexerWsUrl: args.indexerWs,
    },
    provingServerUrl: new URL(args.proofServer),
    relayURL: new URL(args.rpcUrl.replace(/^http/, 'ws')),
  };
  const unshieldedConfig = {
    networkId: getNetworkId(),
    indexerClientConnection: {
      indexerHttpUrl: args.indexerHttp,
      indexerWsUrl: args.indexerWs,
    },
    txHistoryStorage: new InMemoryTransactionHistoryStorage(),
  };
  const dustConfig = {
    ...shieldedConfig,
    costParameters: {
      additionalFeeOverhead: 300_000_000_000_000n,
      feeBlocksMargin: 5,
    },
  };

  const wallet = await WalletFacade.init({
    configuration: { ...shieldedConfig, ...unshieldedConfig, ...dustConfig },
    shielded: (cfg) => ShieldedWallet(cfg).startWithSecretKeys(shieldedSecretKeys),
    unshielded: (cfg) => UnshieldedWallet(cfg).startWithPublicKey(PublicKey.fromKeyStore(unshieldedKeystore)),
    dust: (cfg) =>
      DustWallet(cfg).startWithSecretKey(dustSecretKey, ledger.LedgerParameters.initialParameters().dust),
  });

  await wallet.start(shieldedSecretKeys, dustSecretKey);
  return { wallet, unshieldedKeystore };
}

// ---------------------------------------------------------------------------
// Main flow
// ---------------------------------------------------------------------------

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const seed = resolveSeed(args);
  console.error(`seed loaded (${seed.length} bytes)`);

  const keys = deriveKeys(seed);
  const { wallet, unshieldedKeystore } = await buildWallet(args, keys);
  console.error(`unshielded_address: ${unshieldedKeystore.getBech32Address()}`);
  console.error('waiting for initial sync…');

  const synced = await Rx.firstValueFrom(
    wallet.state().pipe(Rx.filter((s) => s.isSynced)),
  );

  // Dump all three derived addresses so a caller can cross-check against
  // whatever other tool (Rust relayer /status, keygen.mjs, midnight_dust_prep)
  // derives from the same seed. Any mismatch here means the caller is looking
  // at two different wallets.
  const networkId = getNetworkId();
  const coinPubKey = ShieldedCoinPublicKey.fromHexString(synced.shielded.coinPublicKey.toHexString());
  const encPubKey = ShieldedEncryptionPublicKey.fromHexString(synced.shielded.encryptionPublicKey.toHexString());
  const shieldedAddress = MidnightBech32m.encode(networkId, new ShieldedAddress(coinPubKey, encPubKey)).toString();
  const dustAddressBech = MidnightBech32m.encode(networkId, synced.dust.address).toString();
  console.error(`shielded_address:   ${shieldedAddress}`);
  console.error(`unshielded_address: ${unshieldedKeystore.getBech32Address()}`);
  console.error(`dust_address:       ${dustAddressBech}`);

  const nightBalance = synced.unshielded.balances[unshieldedToken().raw] ?? 0n;
  const dustBalance = synced.dust.balance(new Date());
  console.error(`NIGHT: ${nightBalance}   DUST: ${dustBalance}`);

  if (nightBalance === 0n) {
    throw new Error('Unshielded NIGHT balance is 0 — fund the unshielded address before registering');
  }

  if (synced.dust.availableCoins.length > 0 && dustBalance > 0n) {
    console.error('DUST is already flowing — nothing to do.');
    console.log(JSON.stringify({ status: 'already_registered', night_balance: nightBalance.toString(), dust_balance: dustBalance.toString() }, null, 2));
    await wallet.stop?.();
    return;
  }

  const unregistered = synced.unshielded.availableCoins.filter(
    (c) => c.meta?.registeredForDustGeneration !== true,
  );
  console.error(`unregistered NIGHT coins: ${unregistered.length}`);

  const dustReceiverAddr = MidnightBech32m.encode(getNetworkId(), synced.dust.address).toString();
  console.error(`dust_receiver (self): ${dustReceiverAddr}`);
  const dustReceiver = MidnightBech32m.parse(dustReceiverAddr).decode(DustAddress, getNetworkId());

  if (unregistered.length > 0) {
    console.error('building recipe → finalize → submit…');
    const recipe = await wallet.registerNightUtxosForDustGeneration(
      unregistered,
      unshieldedKeystore.getPublicKey(),
      (payload) => unshieldedKeystore.signData(payload),
      dustReceiver,
    );
    const finalized = await wallet.finalizeRecipe(recipe);
    const submitResult = await wallet.submitTransaction(finalized);
    const txId = submitResult?.txHash ?? submitResult?.hash ?? JSON.stringify(submitResult);
    console.error(`submitted: ${txId}`);
  } else {
    console.error('all coins already marked registered — just waiting for DUST to appear');
  }

  // Poll for DUST balance up to --wait-secs
  const deadline = Date.now() + args.waitSecs * 1000;
  let last = 0n;
  while (Date.now() < deadline) {
    const now = await Rx.firstValueFrom(wallet.state());
    if (now.isSynced) {
      const bal = now.dust.balance(new Date());
      if (bal !== last) {
        console.error(`  …dust balance: ${bal}`);
        last = bal;
      }
      if (bal > 0n) break;
    }
    await new Promise((r) => setTimeout(r, 5000));
  }

  console.log(JSON.stringify({
    status: last > 0n ? 'dust_flowing' : 'submitted_waiting',
    registered: unregistered.length,
    dust_balance: last.toString(),
  }, null, 2));

  await wallet.stop?.();
}

main().catch((err) => {
  console.error(`\nERROR: ${err?.stack ?? err}`);
  process.exit(1);
});
