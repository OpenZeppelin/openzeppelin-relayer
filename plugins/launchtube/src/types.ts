import { Transaction, SorobanRpc, xdr } from '@stellar/stellar-sdk';

// Clear request types - either XDR or func+auth
export type LaunchtubeRequest =
  | { type: 'xdr'; xdr: string; sim: boolean }
  | { type: 'func-auth'; func: string; auth: string[]; sim: boolean };

// What we extract from either request type
export interface ExtractedData {
  func: xdr.HostFunction;
  auth: xdr.SorobanAuthorizationEntry[] | undefined;
  inputTx?: Transaction; // Only present for XDR requests
}

// Result of auth checking
export interface AuthCheckResult {
  shouldSimulate: boolean;
  violations: string[]; // Any auth violations found
}

// Sequence account from the pool
export interface SequenceAccount {
  relayerId: string;
  address: string;
  sequence: string;
}

// Final response
export interface LaunchtubeResponse {
  transactionId: string | null;
  status: string;
  hash: string | null;
  error?: string;
}

// External dependencies
export interface RpcClient {
  simulateTransaction(tx: Transaction): Promise<SorobanRpc.Api.SimulateTransactionResponse>;
}

// Errors
export class ValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ValidationError';
  }
}

export class AuthError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'AuthError';
  }
}

export class SimulationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SimulationError';
  }
}
