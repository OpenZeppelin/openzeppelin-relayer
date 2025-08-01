import { Transaction, xdr } from '@stellar/stellar-sdk';

export function calculateFee(transaction: Transaction): bigint {
  // Extract resource fee from transaction if it has Soroban data
  let resourceFee = 0n;

  const envelope = transaction.toEnvelope();
  if (envelope.switch() === xdr.EnvelopeType.envelopeTypeTx()) {
    const sorobanData = envelope.v1().tx().ext().sorobanData();
    if (sorobanData) {
      resourceFee = sorobanData.resourceFee().toBigInt();
    }
  }

  // Random base fee between 205 and 605
  const baseFee = getRandomNumber(205, 605);

  // Calculate total fee
  if (resourceFee > 0) {
    return BigInt(baseFee) + resourceFee;
  } else {
    // For non-Soroban transactions
    return BigInt(baseFee) + BigInt(60000);
  }
}

function getRandomNumber(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}
