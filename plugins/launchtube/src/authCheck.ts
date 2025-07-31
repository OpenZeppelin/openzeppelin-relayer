import { xdr, Address } from '@stellar/stellar-sdk';
import { LaunchtubeRequest, ExtractedData, AuthCheckResult, SequenceAccount, AuthError } from './types';

export function checkAuthAndSimDecision(
  request: LaunchtubeRequest,
  extracted: ExtractedData,
  sequence: SequenceAccount
): AuthCheckResult {
  const violations: string[] = [];
  let forceNoSimulation = false;

  // Check each auth entry for violations
  for (const authEntry of extracted.auth || []) {
    switch (authEntry.credentials().switch()) {
      case xdr.SorobanCredentialsType.sorobanCredentialsSourceAccount():
        // Source account auth requires the transaction source to sign
        // If we're simulating, we'll rebuild the tx with our sequence account
        // So we must disable simulation to preserve the original signatures
        if (request.sim) {
          forceNoSimulation = true;
        }

        // Also check that the source account isn't our sequence account
        const txSource = extracted.inputTx?.source;
        const opSource = (extracted.inputTx?.operations[0] as any)?.source;

        if (txSource === sequence.address || opSource === sequence.address) {
          violations.push('`sorobanCredentialsSourceAccount` is invalid - cannot use sequence account as source');
        }
        break;

      case xdr.SorobanCredentialsType.sorobanCredentialsAddress():
        // Check that auth isn't trying to use our sequence account
        if (authEntry.credentials().address().address().switch() === xdr.ScAddressType.scAddressTypeAccount()) {
          const pk = authEntry.credentials().address().address().accountId();

          if (
            pk.switch() === xdr.PublicKeyType.publicKeyTypeEd25519() &&
            Address.account(pk.ed25519()).toString() === sequence.address
          ) {
            violations.push('`sorobanCredentialsAddress` is invalid - cannot use sequence account in auth');
          }
        }
        break;

      default:
        violations.push('Invalid credentials type');
    }
  }

  // Throw if we found any violations
  if (violations.length > 0) {
    throw new AuthError(violations.join('; '));
  }

  // Determine if we should simulate
  let shouldSimulate = request.sim && !forceNoSimulation;

  // For func-auth requests, we MUST simulate (no transaction to use)
  if (request.type === 'func-auth') {
    shouldSimulate = true;
  }

  return {
    shouldSimulate,
    violations,
  };
}
