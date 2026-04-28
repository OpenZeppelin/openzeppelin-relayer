#!/usr/bin/env node
/**
 * Midnight Shielded Sync Spike
 *
 * Standalone script that uses the reference Midnight wallet packages
 * (`@midnight-ntwrk/wallet-sdk-shielded` + facade) to sync a wallet from a
 * 32-byte seed and dump the shielded state it discovers. Lets us answer:
 *
 *   "Is the reference SDK able to ingest our shielded tx where the Rust
 *    relayer's update_from_tx fails with InvalidDustSpendProof?"
 *
 * If this script reports a non-zero shielded balance and prints the coin we
 * received (10 tZe → 10_000_000n in ledger units), the reference path works
 * → our Rust ingestion is the diverging factor → port the relevant pattern.
 *
 * If this script also fails or reports zero balance, the constraint is
 * upstream and we send a precise repro to the Midnight team.
 *
 * Usage:
 *   node spike.mjs --keystore-path <path> --passphrase <pass>
 *   node spike.mjs --seed-hex <64-hex>
 *
 * Optional overrides (defaults target preview testnet):
 *   --network         default: preview
 *   --indexer-http    default: https://indexer.preview.midnight.network/api/v4/graphql
 *   --indexer-ws      default: wss://indexer.preview.midnight.network/api/v4/graphql/ws
 *   --rpc-url         default: https://rpc.preview.midnight.network
 *   --proof-server    default: http://localhost:6300
 *   --raw-dump-secs   default: 8     (how long to passively dump zswap events
 *                                     before starting the wallet sync)
 *   --sync-wait-secs  default: 60    (max time to wait for isSynced)
 */

import crypto from 'node:crypto';
import fs from 'node:fs';
import process from 'node:process';
import { keccak_256 } from '@noble/hashes/sha3';
import WS, { WebSocket } from 'ws';
globalThis.WebSocket ??= WebSocket;

import * as Rx from 'rxjs';
import { HDWallet, Roles } from '@midnight-ntwrk/wallet-sdk-hd';
import * as ledger from '@midnight-ntwrk/ledger-v8';
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
  MidnightBech32m,
  ShieldedAddress,
  ShieldedCoinPublicKey,
  ShieldedEncryptionPublicKey,
} from '@midnight-ntwrk/wallet-sdk-address-format';

// ---------------------------------------------------------------------------
// Arg parsing
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const out = {
    network: 'preview',
    rpcUrl: 'https://rpc.preview.midnight.network',
    indexerHttp: 'https://indexer.preview.midnight.network/api/v4/graphql',
    indexerWs: 'wss://indexer.preview.midnight.network/api/v4/graphql/ws',
    proofServer: 'http://localhost:6300',
    rawDumpSecs: 8,
    syncWaitSecs: 60,
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
      case '--raw-dump-secs': out.rawDumpSecs = Number(next()); break;
      case '--sync-wait-secs': out.syncWaitSecs = Number(next()); break;
      default:
        if (a.startsWith('--')) throw new Error(`unknown flag: ${a}`);
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Keystore decoder — lifted from scripts/midnight-dust-register/register.mjs.
// Ethereum v3 keystore (scrypt + aes-128-ctr + keccak-256 mac).
// ---------------------------------------------------------------------------

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
// HD derivation + wallet construction
// ---------------------------------------------------------------------------

function deriveKeys(seedBuffer) {
  const hd = HDWallet.fromSeed(seedBuffer);
  if (hd.type !== 'seedOk') {
    throw new Error(`HDWallet.fromSeed failed (type=${hd.type})`);
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
// Diagnostic A — passive raw dump of zswapLedgerEvents + shieldedTransactions
// over WS. Tells us what the indexer actually emits, independent of any
// wallet/SDK code. Useful to compare against Rust's sync_shielded path.
// ---------------------------------------------------------------------------

async function dumpRawShieldedEvents(args) {
  console.error('--- raw zswapLedgerEvents (passive WS dump) ---');
  const ws = new WS(args.indexerWs, 'graphql-transport-ws');
  await new Promise((res, rej) => { ws.once('open', res); ws.once('error', rej); });
  ws.send(JSON.stringify({ type: 'connection_init' }));

  const subId = 'zswap-' + Math.random().toString(36).slice(2, 8);
  let gotAck = false;
  let seenCount = 0;
  let firstSamples = [];

  ws.on('message', (raw) => {
    let msg;
    try { msg = JSON.parse(raw.toString()); } catch { return; }
    if (msg.type === 'connection_ack') {
      gotAck = true;
      ws.send(JSON.stringify({
        id: subId,
        type: 'subscribe',
        payload: {
          query: 'subscription { zswapLedgerEvents(id: 0) { __typename id maxId protocolVersion raw } }',
        },
      }));
      return;
    }
    if (msg.type === 'next' && msg.id === subId) {
      seenCount++;
      if (firstSamples.length < 3) {
        const evt = msg.payload?.data?.zswapLedgerEvents;
        firstSamples.push({ __typename: evt?.__typename, id: evt?.id, raw_len: evt?.raw?.length ?? 0 });
      }
    }
  });

  await new Promise((r) => setTimeout(r, args.rawDumpSecs * 1000));
  ws.send(JSON.stringify({ id: subId, type: 'complete' }));
  ws.close();

  console.error(`  connection_ack:  ${gotAck}`);
  console.error(`  events seen:     ${seenCount} (in ${args.rawDumpSecs}s)`);
  console.error(`  first samples:   ${JSON.stringify(firstSamples)}`);
  console.error('--- end zswapLedgerEvents ---');
}

// ---------------------------------------------------------------------------
// Diagnostic B — wait for the SDK wallet to report isSynced, then dump
// shielded balances and coin commitments.
// ---------------------------------------------------------------------------

async function dumpSyncedShieldedState(args, wallet, unshieldedKeystore) {
  console.error(`waiting for initial sync (max ${args.syncWaitSecs}s)…`);

  // The SDK's `state()` is an Rx Observable<WalletState>. We wait for the
  // first emission that has isSynced=true (or time out).
  const synced = await Rx.firstValueFrom(
    Rx.merge(
      wallet.state().pipe(Rx.filter((s) => s.isSynced)),
      Rx.timer(args.syncWaitSecs * 1000).pipe(
        Rx.map(() => { throw new Error(`sync did not complete within ${args.syncWaitSecs}s`); }),
      ),
    ),
  );

  // Addresses for cross-checking against the Rust /status endpoint
  const networkId = getNetworkId();
  const coinPubKey = ShieldedCoinPublicKey.fromHexString(synced.shielded.coinPublicKey.toHexString());
  const encPubKey = ShieldedEncryptionPublicKey.fromHexString(synced.shielded.encryptionPublicKey.toHexString());
  const shieldedAddress = MidnightBech32m.encode(networkId, new ShieldedAddress(coinPubKey, encPubKey)).toString();
  console.error(`shielded_address:   ${shieldedAddress}`);
  console.error(`unshielded_address: ${unshieldedKeystore.getBech32Address()}`);

  console.error('--- SDK-discovered SHIELDED state ---');

  // Shielded balances per token
  const shBalances = synced.shielded.balances ?? {};
  const shBalanceEntries = Object.entries(shBalances);
  console.error(`shielded balance entries: ${shBalanceEntries.length}`);
  for (const [tokenHex, bal] of shBalanceEntries) {
    console.error(`  token=${tokenHex}  balance=${bal}`);
  }

  // Available shielded coins (the actual ground truth — what we'd be able
  // to spend if our Rust LedgerContext had the same view)
  const shCoins = synced.shielded.availableCoins ?? [];
  console.error(`shielded availableCoins count: ${shCoins.length}`);
  for (const [i, c] of shCoins.entries()) {
    console.error(
      `  [${i}] ${JSON.stringify(c, (_k, v) => {
        if (typeof v === 'bigint') return v.toString();
        if (v && typeof v.toHexString === 'function') return v.toHexString();
        return v;
      })}`,
    );
  }

  // Pending outputs (unconfirmed coins from txs we recently submitted)
  const pendingOuts = synced.shielded.pendingOutputs ?? [];
  console.error(`shielded pendingOutputs count: ${pendingOuts.length}`);

  console.error('--- end SHIELDED state ---');

  return { shieldedAddress, shBalances, shCoinsCount: shCoins.length };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const seed = resolveSeed(args);
  console.error(`seed loaded (${seed.length} bytes)`);

  // Run the raw indexer dump first so we capture what's on the wire,
  // independent of the SDK's wallet-state machinery.
  try {
    await dumpRawShieldedEvents(args);
  } catch (e) {
    console.error(`raw event dump failed (non-fatal): ${e?.message ?? e}`);
  }

  const keys = deriveKeys(seed);
  const { wallet, unshieldedKeystore } = await buildWallet(args, keys);

  const result = await dumpSyncedShieldedState(args, wallet, unshieldedKeystore);

  // Final JSON summary on stdout — easy to capture in shell pipelines.
  // stderr keeps the human-readable narrative.
  // BigInt isn't JSON-serializable; the replacer converts to string.
  console.log(JSON.stringify({
    shielded_address: result.shieldedAddress,
    shielded_balances: result.shBalances,
    shielded_coins_count: result.shCoinsCount,
  }, (_k, v) => (typeof v === 'bigint' ? v.toString() : v), 2));

  await wallet.stop?.();
}

main().catch((err) => {
  console.error(`\nERROR: ${err?.stack ?? err}`);
  process.exit(1);
});
