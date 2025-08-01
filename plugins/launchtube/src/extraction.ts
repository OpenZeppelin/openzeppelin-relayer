import { Transaction, Operation, xdr } from '@stellar/stellar-sdk';
import { LaunchtubeRequest, ExtractedData, ValidationError } from './types';

export function extractFunctionAndAuth(request: LaunchtubeRequest, networkPassphrase: string): ExtractedData {
  if (request.type === 'xdr') {
    // Parse the transaction and extract func/auth from it
    const tx = new Transaction(request.xdr, networkPassphrase);

    // Validate transaction structure
    if (tx.operations.length !== 1) {
      throw new ValidationError('Must include only one Soroban operation');
    }

    const operation = tx.operations[0];
    if (operation.type !== 'invokeHostFunction') {
      throw new ValidationError('Must include only one operation of type invokeHostFunction');
    }

    const op = operation as Operation.InvokeHostFunction;

    // Validate function type
    validateHostFunction(op.func);

    return {
      func: op.func,
      auth: op.auth,
      inputTx: tx,
    };
  } else {
    // Parse func and auth from base64
    const func = xdr.HostFunction.fromXDR(request.func, 'base64');
    const auth = request.auth.map((a) => xdr.SorobanAuthorizationEntry.fromXDR(a, 'base64'));

    // Validate function type
    validateHostFunction(func);

    return {
      func,
      auth,
      // No inputTx for func-auth requests
    };
  }
}

function validateHostFunction(func: xdr.HostFunction): void {
  if (
    func.switch() !== xdr.HostFunctionType.hostFunctionTypeInvokeContract() &&
    func.switch() !== xdr.HostFunctionType.hostFunctionTypeCreateContract()
  ) {
    throw new ValidationError(
      'Operation func must be of type `hostFunctionTypeInvokeContract` or `hostFunctionTypeCreateContract`'
    );
  }
}
