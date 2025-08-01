import { runPlugin, PluginAPI } from '../../lib/plugin';
import { SorobanRpc } from '@stellar/stellar-sdk';
import { SequencePool } from './pool';
import { loadConfig, getNetworkPassphrase } from './config';
import { LaunchtubeResponse, RpcClient, SequenceAccount } from './types';
import { validateAndParseRequest } from './validation';
import { extractFunctionAndAuth } from './extraction';
import { checkAuthAndSimDecision } from './authCheck';
import { simulateAndBuild, validateExistingTransaction } from './simulation';
import { calculateFee } from './fee';

// Initialize dependencies
const config = loadConfig();
const pool = new SequencePool(config.sequenceRelayerIds);
const rpc: RpcClient = new SorobanRpc.Server(config.rpcUrl);

async function launchtube(api: PluginAPI, params: any): Promise<LaunchtubeResponse> {
  let sequenceAccount: SequenceAccount | undefined;

  try {
    // 1. Validate and parse input into structured request
    const request = validateAndParseRequest(params);

    // 2. Get sequence account from pool
    const poolAccount = await pool.acquire();
    const sequenceRelayer = api.useRelayer(poolAccount.relayerId);
    const sequenceInfo = await sequenceRelayer.getRelayer();
    if (!sequenceInfo.data) {
      throw new Error('No sequence info found');
    }
    const sequenceStatus = await sequenceRelayer.getRelayerStatus();

    // Create complete sequence account with all required info
    sequenceAccount = {
      relayerId: poolAccount.relayerId,
      address: sequenceInfo.data.address,
      sequence: (sequenceStatus.data as any).sequence,
    };

    // 3. Extract function and auth from either XDR or func+auth
    const networkPassphrase = getNetworkPassphrase(config.network);
    const extracted = extractFunctionAndAuth(request, networkPassphrase);

    // 4. Check auth entries and determine if we should simulate
    const authCheck = checkAuthAndSimDecision(request, extracted, sequenceAccount);

    // 5. Get the final transaction
    //    - If simulating: build new tx with sequence account and simulate
    //    - If not simulating: validate the existing transaction
    const finalTransaction = authCheck.shouldSimulate
      ? await simulateAndBuild(extracted, sequenceAccount, sequenceRelayer, rpc, networkPassphrase)
      : validateExistingTransaction(extracted.inputTx!); // We know inputTx exists if not simulating

    // 6. Calculate fee and submit with fee bump
    const fundRelayer = api.useRelayer(config.fundRelayerId);
    const fee = calculateFee(finalTransaction);

    console.log('Transaction details:', {
      hasSorobanData: !!finalTransaction.toEnvelope().v1()?.tx().ext().sorobanData(),
      fee: fee.toString(),
      hasSignatures: finalTransaction.signatures.length > 0,
      operationType: finalTransaction.operations[0]?.type,
      source: finalTransaction.source,
    });

    const result = await fundRelayer.sendTransaction({
      network: config.network,
      transaction_xdr: finalTransaction.toXDR(),
      fee_bump: true,
      max_fee: parseInt(fee.toString()),
    });

    return {
      transactionId: result.id,
      status: result.status,
      hash: result.hash,
    };
  } catch (error: any) {
    // Always release sequence account on error
    if (sequenceAccount) {
      pool.release(sequenceAccount);
    }
    throw error;
  } finally {
    // Release sequence account when done
    if (sequenceAccount) {
      pool.release(sequenceAccount);
    }
  }
}

// Error-catching wrapper
async function launchtubeWrapper(api: PluginAPI, params: any): Promise<any> {
  try {
    return await launchtube(api, params);
  } catch (error: any) {
    console.error(`Plugin error: ${error.message || error}`);
    return {
      error: error.message || String(error),
      transactionId: null,
      status: 'failed',
    };
  }
}

runPlugin(launchtubeWrapper);
