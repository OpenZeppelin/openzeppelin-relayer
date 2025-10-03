import { Buffer } from 'node:buffer';
/**
 * LaunchTube Stellar account bootstrap script.
 *
 * The script manages LaunchTube relayer slots that follow the naming pattern
 * `lt-seq-0001`, `lt-seq-0002`, ... up to the target count provided via
 * `--total`. For each slot it ensures:
 *   1. A plain signer exists (stored under `<slot>-signer`).
 *   2. A Stellar relayer exists, pointing at that signer.
 *   3. The associated Stellar account is funded on-chain with the configured
 *      starting balance.
 *
 * When a signer or relayer is missing, the tool recreates it. Funding
 * transactions are submitted through the designated funding relayer and the
 * script waits until the relayer reports the transaction as confirmed. The
 * only stateful data retained by the script is printed on stdout (public key,
 * Stellar seed, and signer hex key) so make sure to capture the output if you
 * plan to persist secrets.
 *
 * Usage (example):
 *   pnpm ts-node create-sequence-accounts.ts \
 *     --total 200 \
 *     --base-url http://localhost:8080 \
 *     --api-key <relayer-api-key> \
 *     --funding-relayer stellar-funder \
 *     --plugin-id launchtube \
 *     --plugin-admin-secret <management-secret> \
 *     --network testnet \
 *     [--fix] [--dry-run]
 *
 * Required flags:
 *   --total <number>
 *       Number of LaunchTube relayer slots to create or validate.
 *   --base-url <url>
 *       Base URL of the OpenZeppelin Relayer API (e.g. http://localhost:8080).
 *   --api-key <token>
 *       API token with permissions to manage signers and relayers.
 *   --funding-relayer <relayer-id>
 *       ID of the Stellar relayer that will fund new accounts.
 *   --plugin-id <string>
 *       LaunchTube plugin identifier that should receive the updated sequence
 *       account list after provisioning.
 *   --plugin-admin-secret <string>
 *       Secret required by the LaunchTube management API when calling
 *       `setSequenceAccounts`.
 *
 * Optional flags:
 *   --network <public|mainnet|testnet|futurenet>
 *       Network identifier used for relayer creation and account funding
 *       (default: testnet).
 *   --horizon <url>
 *       Override the Horizon endpoint used to fetch ledger metadata and account
 *       information (defaults are inferred from --network).
 *   --prefix <string>
 *       Slot prefix (default: lt-seq-). The script pads the numeric suffix with
 *       `--padding` digits.
 *   --padding <number>
 *       Zero-padding for the numeric suffix (default: 4). For example,
 *       padding=4 produces `lt-seq-0001`.
 *   --starting-balance <decimal>
 *       XLM amount credited to each new account (default: 5). Must use up to 7
 *       decimal places.
 *   --timeout <seconds>
 *       Horizon transaction timeout used when building funding transactions
 *       (default: 180 seconds).
 *   --fix
 *       Validate every slot up to --total. Missing signers or relayers are
 *       recreated and accounts that are absent on-chain are funded. Existing
 *       accounts are left untouched.
 *   --dry-run
 *       Print the actions that would be executed and exit without mutating any
 *       state. Use together with --fix to audit the current setup.
 *
 * Typical workflow:
 *   1. Run once without --fix to provision new slots.
 *   2. If the run is interrupted, rerun with --fix to heal partially created
 *      slots. The plugin list is always updated at the end of the run using the
 *      provided plugin credentials.
 *   3. Repeat with a higher --total as you need additional relayer capacity.
 */
import {
  Configuration,
  PluginsApi,
  RelayerNetworkType,
  RelayersApi,
  SignerTypeRequest,
  SignersApi,
  StellarTransactionRequest,
} from '@openzeppelin/relayer-sdk';
import { Account, BASE_FEE, Horizon, Keypair, Networks, Operation, TransactionBuilder } from 'stellar-sdk';

const { Server } = Horizon;
type HorizonAccount = Awaited<ReturnType<InstanceType<typeof Server>['loadAccount']>>;

interface CliOptions {
  total: number;
  prefix: string;
  padding: number;
  startingBalance: string;
  timeoutSeconds: number;
  dryRun: boolean;
  fix: boolean;
  horizonUrl?: string;
  basePath?: string;
  accessToken?: string;
  fundingRelayerId?: string;
  network: string;
  pluginId: string;
  pluginAdminSecret: string;
}

interface CreatedAccountSummary {
  name: string;
  relayerId: string;
  signerId: string;
  publicKey: string;
  secretHex?: string;
  accountExisted: boolean;
  funded: boolean;
}

type ResultCodes = {
  transaction?: string;
  operations?: string[];
};

const NETWORK_CONFIG: Record<string, { passphrase: string; horizon: string }> = {
  public: { passphrase: Networks.PUBLIC, horizon: 'https://horizon.stellar.org' },
  mainnet: { passphrase: Networks.PUBLIC, horizon: 'https://horizon.stellar.org' },
  testnet: { passphrase: Networks.TESTNET, horizon: 'https://horizon-testnet.stellar.org' },
  futurenet: {
    passphrase: 'Test SDF Future Network ; October 2022',
    horizon: 'https://horizon-futurenet.stellar.org',
  },
};

const DEFAULT_PREFIX = 'lt-seq-';
const DEFAULT_PADDING = 4;
const DEFAULT_STARTING_BALANCE = '5';
const DEFAULT_TIMEOUT_SECONDS = 180;
const DEFAULT_NETWORK = 'testnet';

async function fetchBaseReserveStroops(server: InstanceType<typeof Server>): Promise<bigint> {
  const ledgerPage = await server.ledgers().order('desc').limit(1).call();
  const latestLedger = ledgerPage?.records?.[0];
  const baseReserve = latestLedger?.base_reserve_in_stroops;
  if (!baseReserve) {
    throw new Error('Horizon ledger response did not include base_reserve_in_stroops.');
  }
  return BigInt(baseReserve);
}

const STROOPS_PER_LUMEN = 10_000_000n;

function lumensToStroops(value: string, label: string): bigint {
  if (!/^\d+(\.\d+)?$/.test(value)) {
    throw new Error(`Invalid decimal value for ${label}: ${value}`);
  }
  const [wholePart, fractionalPart = ''] = value.split('.');
  if (fractionalPart.length > 7) {
    throw new Error(`Invalid decimal value for ${label}: ${value} (maximum 7 decimal places)`);
  }
  const whole = BigInt(wholePart || '0');
  const fractional = BigInt((fractionalPart + '0000000').slice(0, 7));
  return whole * STROOPS_PER_LUMEN + fractional;
}

function formatLumens(stroops: bigint): string {
  const sign = stroops < 0n ? '-' : '';
  const abs = stroops < 0n ? -stroops : stroops;
  const whole = abs / STROOPS_PER_LUMEN;
  const fraction = abs % STROOPS_PER_LUMEN;
  if (fraction === 0n) {
    return `${sign}${whole.toString()}`;
  }
  const fractionStr = fraction.toString().padStart(7, '0').replace(/0+$/, '');
  return `${sign}${whole.toString()}.${fractionStr}`;
}

interface FundingCapacitySummary {
  balance: bigint;
  reserve: bigint;
  available: bigint;
  totalStarting: bigint;
  totalFees: bigint;
  totalRequired: bigint;
}

function summarizeFundingCapacity(
  account: HorizonAccount,
  baseReserveStroops: bigint,
  startingBalance: string,
  accountCount: number
): FundingCapacitySummary {
  const nativeBalance = account.balances.find((balance: any) => balance.asset_type === 'native');
  if (!nativeBalance) {
    throw new Error('Funding account does not hold a native XLM balance.');
  }

  const balanceStroops = lumensToStroops(nativeBalance.balance, 'funding account balance');
  const subentryCount = BigInt(account.subentry_count ?? 0);
  const numSponsoring = BigInt((account as any).num_sponsoring ?? 0);
  const numSponsored = BigInt((account as any).num_sponsored ?? 0);
  let reserveMultiplier = 2n + subentryCount + numSponsoring - numSponsored;
  if (reserveMultiplier < 0n) {
    reserveMultiplier = 0n;
  }
  const reserveStroops = baseReserveStroops * reserveMultiplier;

  const startingBalanceStroops = lumensToStroops(startingBalance, 'starting balance');
  const accounts = BigInt(accountCount);
  const totalStarting = startingBalanceStroops * accounts;
  const totalFees = BigInt(BASE_FEE) * accounts;
  const totalRequired = totalStarting + totalFees;

  const available = balanceStroops - reserveStroops;

  return {
    balance: balanceStroops,
    reserve: reserveStroops,
    available,
    totalStarting,
    totalFees,
    totalRequired,
  };
}

function isNotFoundError(error: unknown): boolean {
  return isHttpStatus(error, 404);
}

interface SignerRecord {
  id: string;
}

interface RelayerRecord {
  id: string;
  address?: string;
  signer_id: string;
}

async function fetchSignerRecord(signersApi: SignersApi, signerId: string): Promise<SignerRecord | undefined> {
  try {
    const response = await signersApi.getSigner(signerId);
    const data = response.data?.data;
    if (!data) {
      return undefined;
    }
    return { id: data.id };
  } catch (error) {
    if (isNotFoundError(error)) {
      return undefined;
    }
    throw error;
  }
}

async function deleteSignerIfExists(signersApi: SignersApi, signerId: string): Promise<void> {
  try {
    await signersApi.deleteSigner(signerId);
  } catch (error) {
    if (!isNotFoundError(error)) {
      throw error;
    }
  }
}

async function fetchRelayerRecord(relayersApi: RelayersApi, relayerId: string): Promise<RelayerRecord | undefined> {
  try {
    const response = await relayersApi.getRelayer(relayerId);
    const data = response.data?.data;
    if (!data) {
      return undefined;
    }
    return { id: data.id, address: data.address, signer_id: data.signer_id ?? '' };
  } catch (error) {
    if (isNotFoundError(error)) {
      return undefined;
    }
    throw error;
  }
}

async function waitForTransactionConfirmation(
  relayersApi: RelayersApi,
  relayerId: string,
  transactionId: string,
  timeoutMs = 120_000,
  pollIntervalMs = 2_000
): Promise<void> {
  const startedAt = Date.now();
  while (true) {
    const elapsed = Date.now() - startedAt;
    if (elapsed > timeoutMs) {
      throw new Error(`Transaction ${transactionId} for relayer ${relayerId} not confirmed within timeout`);
    }

    try {
      const response = await relayersApi.getTransactionById(relayerId, transactionId);
      const data = response.data?.data;
      if (data && 'status' in data) {
        const status = (data as { status?: string }).status ?? 'unknown';
        const reason = (data as { status_reason?: string | null }).status_reason ?? undefined;
        if (status === 'submitted' || status === 'pending') {
          // Keep waiting
        } else if (status === 'confirmed') {
          return;
        } else {
          throw new Error(
            `Transaction ${transactionId} for relayer ${relayerId} reported status '${status}'${reason ? ` (${reason})` : ''}`
          );
        }
      }
    } catch (error) {
      if (isHttpStatus(error, 404)) {
        // If the relayer API hasn't indexed the tx yet, keep waiting
      } else {
        throw error;
      }
    }

    await new Promise((resolve) => setTimeout(resolve, pollIntervalMs));
  }
}

async function accountExists(server: InstanceType<typeof Server>, publicKey: string): Promise<boolean> {
  try {
    await server.loadAccount(publicKey);
    return true;
  } catch (error) {
    if (isNotFoundError(error)) {
      return false;
    }
    throw error;
  }
}

async function updateLaunchTubePlugin(
  pluginsApi: PluginsApi,
  pluginId: string,
  adminSecret: string,
  relayerIds: string[],
): Promise<void> {
  const payload = {
    params: {
      management: {
        action: 'setSequenceAccounts',
        adminSecret,
        relayerIds,
      },
    },
  };

  try {
    await pluginsApi.callPlugin(pluginId, payload);
    console.log(`[Plugin] Updated ${pluginId} with ${relayerIds.length} sequence account(s).`);
  } catch (error) {
    throw new Error(`Failed to update LaunchTube plugin '${pluginId}': ${extractErrorMessage(error)}`);
  }
}


function parseArgs(argv: string[]): CliOptions {
  let prefix = DEFAULT_PREFIX;
  let padding = DEFAULT_PADDING;
  let startingBalance = DEFAULT_STARTING_BALANCE;
  let timeoutSeconds = DEFAULT_TIMEOUT_SECONDS;
  let dryRun = false;
  let fix = false;
  let horizonUrl: string | undefined;
  let total: number | undefined;
  let basePath: string | undefined;
  let accessToken: string | undefined;
  let fundingRelayerId: string | undefined;
  let network = DEFAULT_NETWORK;
  let pluginId: string | undefined;
  let pluginAdminSecret: string | undefined;

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    switch (arg) {
      case '--total':
      case '-n': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --total');
        total = parsePositiveInt(value, '--total');
        break;
      }
      case '--prefix': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --prefix');
        prefix = value;
        break;
      }
      case '--padding': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --padding');
        padding = parsePositiveInt(value, '--padding');
        break;
      }
      case '--starting-balance': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --starting-balance');
        validateStartingBalance(value, '--starting-balance');
        startingBalance = value;
        break;
      }
      case '--timeout': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --timeout');
        timeoutSeconds = parsePositiveInt(value, '--timeout');
        break;
      }
      case '--horizon': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --horizon');
        horizonUrl = value;
        break;
      }
      case '--network': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --network');
        network = value.toLowerCase();
        break;
      }
      case '--base-url': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --base-url');
        basePath = value;
        break;
      }
      case '--api-key': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --api-key');
        accessToken = value;
        break;
      }
      case '--funding-relayer': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --funding-relayer');
        fundingRelayerId = value;
        break;
      }
      case '--dry-run':
        dryRun = true;
        break;
      case '--plugin-id': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --plugin-id');
        pluginId = value;
        break;
      }
      case '--plugin-admin-secret': {
        const value = argv[++i];
        if (!value) throw new Error('Missing value for --plugin-admin-secret');
        pluginAdminSecret = value;
        break;
      }
      case '--fix':
        fix = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  if (total === undefined) {
    throw new Error('Target account count missing. Provide --total.');
  }
  if (!pluginId) {
    throw new Error('LaunchTube plugin ID missing. Provide --plugin-id.');
  }
  if (!pluginAdminSecret) {
    throw new Error('LaunchTube admin secret missing. Provide --plugin-admin-secret.');
  }

  const resolvedPluginId = pluginId;
  const resolvedPluginAdminSecret = pluginAdminSecret;

  return {
    total,
    prefix,
    padding,
    startingBalance,
    timeoutSeconds,
    dryRun,
    fix,
    horizonUrl,
    basePath,
    accessToken,
    fundingRelayerId,
    network,
    pluginId: resolvedPluginId,
    pluginAdminSecret: resolvedPluginAdminSecret,
  };
}

function parsePositiveInt(value: string, label: string): number {
  const parsed = Number.parseInt(value, 10);
  if (Number.isNaN(parsed) || parsed <= 0) {
    throw new Error(`Invalid positive integer for ${label}: ${value}`);
  }
  return parsed;
}

function validateStartingBalance(value: string, label: string): void {
  if (!/^\d+(\.\d+)?$/.test(value)) {
    throw new Error(`Invalid decimal value for ${label}: ${value}`);
  }
  const fractional = value.split('.')[1] ?? '';
  if (fractional.length > 7) {
    throw new Error(`Invalid decimal value for ${label}: ${value} (maximum 7 decimal places)`);
  }
}

function requireOption(value: string | undefined, message: string): string {
  if (!value) {
    throw new Error(message);
  }
  return value;
}

function formatAccountName(prefix: string, index: number, padding: number): string {
  return `${prefix}${index.toString().padStart(padding, '0')}`;
}

function parseIndexFromName(name: string, prefix: string): number | undefined {
  if (!name.startsWith(prefix)) return undefined;
  const suffix = name.slice(prefix.length);
  if (!/^\d+$/.test(suffix)) return undefined;
  return Number.parseInt(suffix, 10);
}

function getNetworkPassphrase(network: string, override?: string): string {
  if (override) return override;
  const normalized = network.toLowerCase();
  const config = NETWORK_CONFIG[normalized];
  if (config) {
    return config.passphrase;
  }
  throw new Error(`Unsupported Stellar network '${network}'. Provide STELLAR_NETWORK_PASSPHRASE to override.`);
}

function defaultHorizonUrl(network: string): string {
  const normalized = network.toLowerCase();
  const config = NETWORK_CONFIG[normalized];
  if (config) {
    return config.horizon;
  }
  throw new Error(`Unknown network '${network}'. Set STELLAR_HORIZON_URL to continue.`);
}

function isHttpStatus(error: unknown, status: number): boolean {
  if (!error || typeof error !== 'object') return false;
  const response = (error as { response?: { status?: number } }).response;
  return response?.status === status;
}

function extractResultCodes(error: unknown): ResultCodes | undefined {
  if (!error || typeof error !== 'object') return undefined;
  const response = (error as { response?: { data?: any } }).response;
  if (!response || typeof response.data !== 'object') return undefined;

  const data = response.data as { details?: any; result_codes?: any };
  const details = data.details ?? {};
  const source = details.result_codes ?? data.result_codes;
  if (!source || typeof source !== 'object') return undefined;

  const transaction = typeof source.transaction === 'string' ? source.transaction : undefined;
  const operations = Array.isArray(source.operations)
    ? source.operations.filter((code: unknown): code is string => typeof code === 'string')
    : undefined;
  return { transaction, operations };
}

function extractErrorMessage(error: unknown): string {
  if (!error) return 'Unknown error';
  if (typeof error === 'string') return error;
  const messages = new Set<string>();

  if (error instanceof Error && error.message) {
    messages.add(error.message);
  }

  if (typeof error === 'object') {
    const anyError = error as any;
    const response = anyError.response;
    if (response) {
      if (typeof response.statusText === 'string') messages.add(response.statusText);
      const data = response.data;
      if (typeof data === 'string') {
        messages.add(data);
      } else if (data && typeof data === 'object') {
        if (typeof data.error === 'string') messages.add(data.error);
        if (typeof data.message === 'string') messages.add(data.message);
        if (typeof data.detail === 'string') messages.add(data.detail);
        if (Array.isArray(data.details)) {
          for (const detail of data.details) {
            if (typeof detail === 'string') messages.add(detail);
            else if (detail && typeof detail === 'object' && typeof detail.message === 'string') {
              messages.add(detail.message);
            }
          }
        }
        if (data.details && typeof data.details === 'object') {
          const nested = data.details;
          if (typeof nested.message === 'string') messages.add(nested.message);
          if (typeof nested.error === 'string') messages.add(nested.error);
        }
      }
    }
  }

  const collected = Array.from(messages).filter(Boolean);
  return collected.length ? collected.join(' | ') : 'Unknown error';
}

function isAccountAlreadyExistsError(error: unknown): boolean {
  const resultCodes = extractResultCodes(error);
  if (resultCodes?.operations?.some((code) => code === 'op_already_exists')) {
    return true;
  }
  const message = extractErrorMessage(error).toLowerCase();
  return message.includes('op_already_exists') || message.includes('account already exists');
}

function isTxBadSeqError(error: unknown): boolean {
  const resultCodes = extractResultCodes(error);
  if (resultCodes?.transaction === 'tx_bad_seq') return true;
  const message = extractErrorMessage(error).toLowerCase();
  return message.includes('tx_bad_seq');
}

async function ensureSigner(signersApi: SignersApi, signerId: string, secretKey: string): Promise<string> {
  try {
    const response = await signersApi.createSigner({
      id: signerId,
      type: SignerTypeRequest.PLAIN,
      config: { key: secretKey },
    });
    const data = response.data?.data;
    if (!data) {
      throw new Error('createSigner returned an empty response');
    }
    return data.id;
  } catch (error) {
    if (isHttpStatus(error, 409)) {
      const existing = await signersApi.getSigner(signerId);
      const data = existing.data?.data;
      if (!data) {
        throw new Error(`Signer ${signerId} already exists but could not be fetched`);
      }
      return data.id;
    }
    throw error;
  }
}

async function ensureRelayer(
  relayersApi: RelayersApi,
  relayerId: string,
  relayerName: string,
  network: string,
  signerId: string
): Promise<{ id: string; address?: string }> {
  try {
    const response = await relayersApi.createRelayer({
      id: relayerId,
      name: relayerName,
      network,
      network_type: RelayerNetworkType.STELLAR,
      signer_id: signerId,
      paused: false,
    });
    const data = response.data?.data;
    if (!data) {
      throw new Error('createRelayer returned an empty response');
    }
    return { id: data.id, address: data.address };
  } catch (error) {
    if (isHttpStatus(error, 409)) {
      const existing = await relayersApi.getRelayer(relayerId);
      const data = existing.data?.data;
      if (!data) {
        throw new Error(`Relayer ${relayerId} already exists but could not be fetched`);
      }
      if (data.signer_id !== signerId) {
        throw new Error(
          `Relayer ${relayerId} already exists but is bound to signer ${data.signer_id}, expected ${signerId}.`
        );
      }
      return { id: data.id, address: data.address };
    }
    throw error;
  }
}

async function collectExistingIndices(relayersApi: RelayersApi, prefix: string, network: string): Promise<Set<number>> {
  const indices = new Set<number>();
  const perPage = 100;
  let page = 1;
  let processed = 0;
  let totalItems = Number.POSITIVE_INFINITY;

  while (processed < totalItems) {
    const response = await relayersApi.listRelayers(page, perPage);
    const payload = response.data;
    const data = payload.data ?? [];

    for (const relayer of data) {
      if (relayer.network_type !== RelayerNetworkType.STELLAR) continue;
      if (relayer.network.toLowerCase() !== network) continue;
      const index = parseIndexFromName(relayer.name, prefix);
      if (index !== undefined) {
        indices.add(index);
      }
    }

    const pagination = payload.pagination;
    if (pagination) {
      totalItems = pagination.total_items;
      processed += data.length;
      if (processed >= totalItems || data.length === 0) {
        break;
      }
    } else {
      break;
    }

    page += 1;
  }

  return indices;
}

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2));
  const basePath = requireOption(options.basePath, 'Relayer base URL (--base-url) is required.');
  const accessToken = requireOption(options.accessToken, 'Relayer API key (--api-key) is required.');
  const fundingRelayerId = requireOption(
    options.fundingRelayerId,
    'Funding relayer ID (--funding-relayer) is required.'
  );
  const network = options.network;
  const networkPassphrase = getNetworkPassphrase(network);
  const horizonUrl = options.horizonUrl ?? defaultHorizonUrl(network);

  const configuration = new Configuration({ basePath, accessToken });
  const relayersApi = new RelayersApi(configuration);
  const signersApi = new SignersApi(configuration);
  const pluginsApi = new PluginsApi(configuration);

  const fundingRelayerResponse = await relayersApi.getRelayer(fundingRelayerId);
  const fundingRelayer = fundingRelayerResponse.data?.data;
  if (!fundingRelayer) {
    throw new Error(`Funding relayer ${fundingRelayerId} not found`);
  }
  const fundingAddress = fundingRelayer.address;
  if (!fundingAddress) {
    throw new Error(`Funding relayer ${fundingRelayerId} does not expose an address`);
  }

  console.log(`Funding relayer: ${fundingRelayerId} (${fundingAddress})`);
  console.log(`Stellar network: ${network}`);
  console.log(`Horizon URL: ${horizonUrl}`);
  console.log(`Starting balance: ${options.startingBalance} XLM`);
  console.log(`Target accounts: ${options.total}`);
  if (options.dryRun) {
    console.log('Dry run enabled: no signers, relayers, or transactions will be created.');
  }

  const existingIndices = await collectExistingIndices(relayersApi, options.prefix, network);
  console.log(`Found ${existingIndices.size} existing relayers with prefix '${options.prefix}'.`);

  const allIndices = Array.from({ length: options.total }, (_, idx) => idx + 1);
  const missingIndices = allIndices.filter((index) => !existingIndices.has(index));
  const indicesToProcess = options.fix ? allIndices : missingIndices;

  if (indicesToProcess.length === 0) {
    console.log('No missing accounts detected. Nothing to do.');
    if (!options.fix) {
      return;
    }
  }

  if (options.fix) {
    console.log(`Fix mode enabled: validating ${indicesToProcess.length} account(s).`);
  } else {
    console.log(`Preparing to create ${indicesToProcess.length} new account(s).`);
  }
  console.log(`Starting balance per account: ${options.startingBalance} XLM`);

  if (options.dryRun) {
    console.log('Dry run summary:');
    for (const index of indicesToProcess) {
      const name = formatAccountName(options.prefix, index, options.padding);
      console.log(
        options.fix
          ? `- would verify relayer '${name}' and signer '${name}-signer', funding if required.`
          : `- would create relayer '${name}' and signer '${name}-signer'.`
      );
    }
    return;
  }

  const server = new Server(horizonUrl);
  const fundingAccountResponse = await server.loadAccount(fundingAddress);
  const baseReserveStroops = await fetchBaseReserveStroops(server);
  const targetCount = options.fix ? missingIndices.length : indicesToProcess.length;
  const fundingSummary = summarizeFundingCapacity(
    fundingAccountResponse,
    baseReserveStroops,
    options.startingBalance,
    targetCount
  );

  console.log(
    `[Funding] Native balance: ${formatLumens(fundingSummary.balance)} XLM (reserve: ${formatLumens(
      fundingSummary.reserve
    )} XLM).`
  );
  console.log(`[Funding] Available for new accounts: ${formatLumens(fundingSummary.available)} XLM`);
  console.log(
    `[Funding] Required for ${targetCount} account(s): ${formatLumens(
      fundingSummary.totalRequired
    )} XLM (starting balances ${formatLumens(fundingSummary.totalStarting)} + fees ${formatLumens(
      fundingSummary.totalFees
    )}).`
  );

  if (fundingSummary.available < fundingSummary.totalRequired) {
    const deficit = fundingSummary.totalRequired - fundingSummary.available;
    throw new Error(
      `Funding relayer ${fundingRelayerId} needs an additional ${formatLumens(deficit)} XLM to fund ${
        targetCount
      } account(s). Top up the account or reduce the account count.`
    );
  }

  let currentSequence = fundingAccountResponse.sequenceNumber();
  console.log(`Funding account sequence: ${currentSequence}`);

  const createdAccounts: CreatedAccountSummary[] = [];

  for (const index of indicesToProcess) {
    const accountName = formatAccountName(options.prefix, index, options.padding);
    const signerId = `${accountName}-signer`;
    const relayerId = accountName;

    const existingSigner = await fetchSignerRecord(signersApi, signerId);
    const existingRelayer = await fetchRelayerRecord(relayersApi, relayerId);

    const signerExistedBefore = Boolean(existingSigner);
    const relayerExistedBefore = Boolean(existingRelayer);

    let signerRecord = existingSigner;
    let relayerRecord = existingRelayer;
    let keypair: Keypair | undefined;
    let secretHex: string | undefined;
    let signerCreated = false;
    let relayerCreated = false;

    if (!signerRecord) {
      keypair = Keypair.random();
      secretHex = Buffer.from(keypair.rawSecretKey()).toString('hex');
      const signerIdentifier = await ensureSigner(signersApi, signerId, secretHex);
      signerRecord = { id: signerIdentifier };
      signerCreated = true;
    }

    if (!relayerRecord) {
      if (!keypair) {
        if (signerRecord) {
          console.log(`[${accountName}] Relayer missing; recreating signer to restore configuration.`);
          await deleteSignerIfExists(signersApi, signerId);
          keypair = Keypair.random();
          secretHex = Buffer.from(keypair.rawSecretKey()).toString('hex');
          const signerIdentifier = await ensureSigner(signersApi, signerId, secretHex);
          signerRecord = { id: signerIdentifier };
          signerCreated = true;
        } else {
          keypair = Keypair.random();
          secretHex = Buffer.from(keypair.rawSecretKey()).toString('hex');
          const signerIdentifier = await ensureSigner(signersApi, signerId, secretHex);
          signerRecord = { id: signerIdentifier };
          signerCreated = true;
        }
      }

      const relayerInfo = await ensureRelayer(
        relayersApi,
        relayerId,
        accountName,
        network,
        signerRecord.id
      );
      relayerRecord = { id: relayerInfo.id, address: relayerInfo.address, signer_id: signerRecord.id };
      relayerCreated = true;
    }

    if (!signerRecord) {
      throw new Error(`[${accountName}] Unable to ensure signer state.`);
    }
    if (!relayerRecord) {
      throw new Error(
        `[${accountName}] Unable to determine account address. Consider deleting the relayer and signer manually before rerunning.`
      );
    }

    const publicKey = relayerRecord.address ?? keypair?.publicKey();
    if (!publicKey) {
      throw new Error(
        `[${accountName}] Unable to determine account address. Consider deleting the relayer and signer manually before rerunning.`
      );
    }

    const signerIdentifier = signerRecord.id;
    const relayerIdentifier = relayerRecord.id;

    if (signerCreated || relayerCreated) {
      console.log(
        `[${accountName}] Signer ${signerIdentifier} and relayer ${relayerIdentifier} ready. Account address: ${publicKey}`
      );
    } else {
      console.log(`[${accountName}] Validating existing signer ${signerIdentifier} and relayer ${relayerIdentifier}.`);
    }

    const accountAlreadyExists = await accountExists(server, publicKey);

    let funded = false;
    if (accountAlreadyExists) {
      console.log(`[${accountName}] Account already exists on network. Skipping funding.`);
    } else {
      let submitted = false;
      let attempt = 0;
      while (!submitted && attempt < 2) {
        attempt += 1;
        const account = new Account(fundingAddress, currentSequence);
        const transaction = new TransactionBuilder(account, {
          fee: BASE_FEE.toString(),
          networkPassphrase,
        })
          .addOperation(
            Operation.createAccount({
              destination: publicKey,
              startingBalance: options.startingBalance,
            })
          )
          .setTimeout(options.timeoutSeconds)
          .build();

        const transactionXdr = transaction.toXDR();
        currentSequence = account.sequenceNumber();

        const request: StellarTransactionRequest = {
          network,
          transaction_xdr: transactionXdr,
        };

        try {
          const response = await relayersApi.sendTransaction(fundingRelayerId, request);
          const payload = response.data?.data;
          const status = payload && 'status' in payload ? (payload as { status?: string }).status : undefined;
          const hash = payload && 'hash' in payload ? (payload as { hash?: string }).hash : undefined;
          console.log(
            `[${accountName}] Submitted funding transaction${hash ? ` (hash: ${hash})` : ''}${
              status ? ` with status ${status}` : ''
            }.`
          );
          if (payload && 'id' in payload) {
            const id = (payload as { id?: string }).id;
            if (id) {
              await waitForTransactionConfirmation(relayersApi, fundingRelayerId, id);
              console.log(`[${accountName}] Funding transaction ${id} confirmed.`);
            }
          }
          submitted = true;
          funded = true;
        } catch (error) {
          console.error(`[${accountName}] Funding transaction failed: ${extractErrorMessage(error)}`);
          const refreshed = await server.loadAccount(fundingAddress);
          currentSequence = refreshed.sequenceNumber();

          if (isAccountAlreadyExistsError(error)) {
            console.warn(`[${accountName}] Account appears to already exist. Skipping funding.`);
            submitted = true;
          } else if (attempt < 2 && isTxBadSeqError(error)) {
            console.warn(`[${accountName}] Sequence mismatch detected. Retrying with refreshed sequence.`);
          } else {
            throw error;
          }
        }
      }
    }

    createdAccounts.push({
      name: accountName,
      relayerId: relayerIdentifier,
      signerId: signerIdentifier,
      publicKey,
      secretHex,
      accountExisted: accountAlreadyExists,
      funded,
    });
  }

  console.log(options.fix ? '\nAccount audit summary:' : '\nAccount creation summary:');
  for (const entry of createdAccounts) {
    const secretInfo = entry.secretHex ? `, secret_hex=${entry.secretHex}` : '';
    const existedLabel = entry.accountExisted ? 'yes' : 'no';
    const fundedLabel = entry.funded ? 'yes' : 'no';
    console.log(
      `- ${entry.name}: relayer=${entry.relayerId}, signer=${entry.signerId}, public=${entry.publicKey}, existed=${existedLabel}, funded=${fundedLabel}${secretInfo}`
    );
  }

  const relayerIdsForPlugin = allIndices.map((index) =>
    formatAccountName(options.prefix, index, options.padding)
  );
  await updateLaunchTubePlugin(pluginsApi, options.pluginId, options.pluginAdminSecret, relayerIdsForPlugin);
}

main().catch((error) => {
  console.error(`\nScript failed: ${extractErrorMessage(error)}`);
  if (error instanceof Error && error.stack) {
    console.error(error.stack);
  }
  process.exitCode = 1;
});
