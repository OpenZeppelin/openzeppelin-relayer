import { Transaction, TransactionBuilder, Operation, Account, SorobanRpc, xdr } from '@stellar/stellar-sdk';
import { ExtractedData, SequenceAccount, RpcClient, SimulationError, ValidationError } from './types';
import { Relayer } from '../../lib/plugin';

export async function simulateAndBuild(
  extracted: ExtractedData,
  sequence: SequenceAccount,
  sequenceRelayer: Relayer,
  rpc: RpcClient,
  networkPassphrase: string
): Promise<Transaction> {
  const now = Math.floor(Date.now() / 1000);

  // Build transaction for simulation using sequence account
  const transaction = new TransactionBuilder(new Account(sequence.address, sequence.sequence), {
    fee: '100', // Will be updated after simulation
    networkPassphrase: networkPassphrase,
    timebounds: extracted.inputTx?.timeBounds || {
      minTime: 0,
      maxTime: now + 30,
    },
    memo: extracted.inputTx?.memo,
  })
    .addOperation(
      Operation.invokeHostFunction({
        func: extracted.func,
        auth: extracted.auth,
        source: (extracted.inputTx?.operations[0] as any)?.source,
      })
    )
    .build();

  // Simulate the transaction
  console.log('Simulating transaction...');
  const simResult = await rpc.simulateTransaction(transaction);

  if (SorobanRpc.Api.isSimulationError(simResult)) {
    throw new SimulationError(`Simulation failed: ${simResult.error}`);
  }

  if (SorobanRpc.Api.isSimulationRestore(simResult)) {
    throw new SimulationError('Restore flow not yet supported');
  }

  const successResult = simResult as SorobanRpc.Api.SimulateTransactionSuccessResponse;

  // Verify auth matches what we provided
  validateSimulatedAuth(extracted.auth, successResult.result?.auth);

  // Get transaction data and build final transaction
  const transactionData = successResult.transactionData;
  if (!transactionData) {
    throw new SimulationError('No transaction data from simulation');
  }

  const sorobanData = transactionData.build();
  const resourceFee = sorobanData.resourceFee().toBigInt();

  // Clone transaction with simulation results
  const finalTransaction = TransactionBuilder.cloneFrom(transaction, {
    fee: resourceFee.toString(),
    sorobanData,
  }).build();

  // Sign with sequence account
  const signResult = await sequenceRelayer.signTransaction({
    unsignedXdr: finalTransaction.toXDR(),
  });

  return new Transaction(signResult.signedXdr, networkPassphrase);
}

export function validateExistingTransaction(tx: Transaction): Transaction {
  // Validate transaction constraints for non-simulated path
  const envelope = tx.toEnvelope();
  if (envelope.switch() !== xdr.EnvelopeType.envelopeTypeTx()) {
    throw new ValidationError('Invalid transaction envelope type');
  }

  const sorobanData = envelope.v1().tx().ext().sorobanData();
  if (sorobanData) {
    const resourceFee = sorobanData.resourceFee().toBigInt();

    if (BigInt(tx.fee) > resourceFee + 201n) {
      throw new ValidationError('Transaction fee must be equal to the resource fee');
    }
  }

  const now = Math.floor(Date.now() / 1000);
  if (tx.timeBounds?.maxTime && Number(tx.timeBounds.maxTime) - now > 30) {
    throw new ValidationError(
      'Transaction `timeBounds.maxTime` too far into the future. Must be no greater than 30 seconds'
    );
  }

  return tx;
}

function validateSimulatedAuth(
  providedAuth: xdr.SorobanAuthorizationEntry[] | undefined,
  simulatedAuth: xdr.SorobanAuthorizationEntry[] | undefined
): void {
  if (providedAuth && providedAuth.length > 0) {
    if (!simulatedAuth || simulatedAuth.length === 0) {
      throw new ValidationError('Auth invalid - simulation returned no auth');
    }

    // Check arrays have same auth entries (order doesn't matter)
    const providedAuthXdr = providedAuth.map((a) => a.toXDR('base64'));
    const simulatedAuthXdr = simulatedAuth.map((a) => a.toXDR('base64'));

    const authMatches =
      providedAuthXdr.length === simulatedAuthXdr.length &&
      providedAuthXdr.every((a) => simulatedAuthXdr.includes(a)) &&
      simulatedAuthXdr.every((a) => providedAuthXdr.includes(a));

    if (!authMatches) {
      throw new ValidationError('Auth invalid - simulation returned different auth');
    }
  }
}
