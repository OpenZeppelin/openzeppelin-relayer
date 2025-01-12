// use alloy::primitives::{Address};
// use alloy::signers::Signer;
// use alloy::signers::local::PrivateKeySigner;

// pub struct EvmSigner {
//     wallet: LocalWallet,
//     chain_id: u64,
// }

// #[async_trait]
// impl SignerService for EvmSigner {
//     async fn sign_transaction(&self, tx: TransactionRequest) -> Result<Vec<u8>> {
//         let signed = self.wallet
//             .sign_transaction(&tx, self.chain_id)
//             .await
//             .map_err(|e| eyre!("Failed to sign transaction: {}", e))?;

//         Ok(signed.to_vec())
//     }

//     async fn sign_message(&self, message: Vec<u8>) -> Result<Vec<u8>> {
//         let signed = self.wallet
//             .sign_message(&message)
//             .await
//             .map_err(|e| eyre!("Failed to sign message: {}", e))?;

//         Ok(signed.to_vec())
//     }

//     async fn sign_typed_data(&self, data: TypedData) -> Result<Vec<u8>> {
//         let signed = self.wallet
//             .sign_typed_data(&data)
//             .await
//             .map_err(|e| eyre!("Failed to sign typed data: {}", e))?;

//         Ok(signed.to_vec())
//     }
// }

// impl EvmSigner {
//     pub fn new(private_key: &str, chain_id: u64) -> Result<Self> {
//         let wallet = LocalWallet::from_private_key_str(private_key)
//             .map_err(|e| eyre!("Invalid private key: {}", e))?;

//         Ok(Self {
//             wallet,
//             chain_id,
//         })
//     }

//     pub fn address(&self) -> Address {
//         self.wallet.address()
//     }
// }
