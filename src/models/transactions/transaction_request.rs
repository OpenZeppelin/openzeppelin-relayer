use serde::{Deserialize, Serialize};
use std::convert::TryFrom;

use crate::{repositories::NetworkType, RelayerApiError};

#[derive(Deserialize, Serialize)]
pub struct EvmTransactionRequest {
    pub to: String,
    pub value: u64,
    pub data: String,
    pub gas_limit: u64,
    pub gas_price: u64,
}

impl TryFrom<serde_json::Value> for EvmTransactionRequest {
    type Error = RelayerApiError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|e| RelayerApiError::BadRequest(e.to_string()))
    }
}

#[derive(Deserialize, Serialize)]
pub struct SolanaTransactionRequest {
    pub from: String,
    pub to: String,
    pub lamports: u64,
    pub recent_blockhash: String,
}

impl TryFrom<serde_json::Value> for SolanaTransactionRequest {
    type Error = RelayerApiError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|e| RelayerApiError::BadRequest(e.to_string()))
    }
}

#[derive(Deserialize, Serialize)]
pub struct StellarTransactionRequest {
    pub source_account: String,
    pub destination_account: String,
    pub amount: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub memo: Option<String>,
    pub fee: u32,
    pub sequence_number: String,
}

impl TryFrom<serde_json::Value> for StellarTransactionRequest {
    type Error = RelayerApiError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|e| RelayerApiError::BadRequest(e.to_string()))
    }
}

pub enum NetworkTransactionRequest {
    Evm(EvmTransactionRequest),
    Solana(SolanaTransactionRequest),
    Stellar(StellarTransactionRequest),
}

impl NetworkTransactionRequest {
    pub fn from_json(
        network_type: &NetworkType,
        json: serde_json::Value,
    ) -> Result<Self, RelayerApiError> {
        match network_type {
            NetworkType::Evm => Ok(Self::Evm(EvmTransactionRequest::try_from(json)?)),
            NetworkType::Solana => Ok(Self::Solana(SolanaTransactionRequest::try_from(json)?)),
            NetworkType::Stellar => Ok(Self::Stellar(StellarTransactionRequest::try_from(json)?)),
        }
    }

    pub fn try_into_evm(self) -> Result<EvmTransactionRequest, RelayerApiError> {
        match self {
            NetworkTransactionRequest::Evm(tx) => Ok(tx),
            _ => Err(RelayerApiError::BadRequest(
                "Expected EVM transaction".to_string(),
            )),
        }
    }

    pub fn try_into_solana(self) -> Result<SolanaTransactionRequest, RelayerApiError> {
        match self {
            NetworkTransactionRequest::Solana(tx) => Ok(tx),
            _ => Err(RelayerApiError::BadRequest(
                "Expected Solana transaction".to_string(),
            )),
        }
    }

    pub fn try_into_stellar(self) -> Result<StellarTransactionRequest, RelayerApiError> {
        match self {
            NetworkTransactionRequest::Stellar(tx) => Ok(tx),
            _ => Err(RelayerApiError::BadRequest(
                "Expected Stellar transaction".to_string(),
            )),
        }
    }
}

// use serde::{Deserialize, Serialize};

// use crate::{repositories::NetworkType, RelayerApiError};

// pub trait NetworkTransactionRequestTrait: Sized {
//     fn from_json(json: serde_json::Value) -> Result<Self, RelayerApiError>;
// }

// #[derive(Deserialize, Serialize)]
// struct EvmTransactionRequest {
//     from: String,
//     to: String,
//     value: u64,
//     gas: u64,
//     gas_price: u64,
// }

// impl NetworkTransactionRequestTrait for EvmTransactionRequest {
//     fn from_json(json: serde_json::Value) -> Result<Self, RelayerApiError> {
//         serde_json::from_value(json).map_err(|e| RelayerApiError::BadRequest(e.to_string()))
//     }
// }

// #[derive(Deserialize, Serialize)]
// struct SolanaTransactionRequest {
//     from: String,
//     to: String,
//     lamports: u64,
//     recent_blockhash: String,
// }

// impl NetworkTransactionRequestTrait for SolanaTransactionRequest {
//     fn from_json(json: serde_json::Value) -> Result<Self, RelayerApiError> {
//         serde_json::from_value(json).map_err(|e| RelayerApiError::BadRequest(e.to_string()))
//     }
// }

// #[derive(Deserialize, Serialize)]
// struct StellarTransactionRequest {
//     source_account: String,
//     destination_account: String,
//     amount: String,
//     asset_code: String,
//     asset_issuer: Option<String>,
//     memo: Option<String>,
//     fee: u32,
//     sequence_number: String,
// }

// impl NetworkTransactionRequestTrait for StellarTransactionRequest {
//     fn from_json(json: serde_json::Value) -> Result<Self, RelayerApiError> {
//         serde_json::from_value(json).map_err(|e| RelayerApiError::BadRequest(e.to_string()))
//     }
// }

// #[derive(Deserialize, Serialize)]
// struct MidnightTransactionRequest {
//     sender: String,
//     receiver: String,
//     amount: u64,
//     fee: u64,
//     timestamp: u64,
//     memo: Option<String>,
// }

// impl NetworkTransactionRequestTrait for MidnightTransactionRequest {
//     fn from_json(json: serde_json::Value) -> Result<Self, RelayerApiError> {
//         serde_json::from_value(json).map_err(|e| RelayerApiError::BadRequest(e.to_string()))
//     }
// }

// pub enum NetworkTransactionRequest {
//     Evm(EvmTransactionRequest),
//     Solana(SolanaTransactionRequest),
//     Stellar(StellarTransactionRequest),
//     Midnight(MidnightTransactionRequest),
// }

// pub fn create_network_transaction_request(
//     network_type: &NetworkType,
//     json: serde_json::Value,
// ) -> Result<NetworkTransactionRequest, RelayerApiError> {
//     match network_type {
//         NetworkType::Evm => {
//             let request = EvmTransactionRequest::from_json(json)?;
//             Ok(NetworkTransactionRequest::Evm(request))
//         }
//         NetworkType::Solana => {
//             let request = SolanaTransactionRequest::from_json(json)?;
//             Ok(NetworkTransactionRequest::Solana(request))
//         }
//         NetworkType::Stellar => {
//             let request = StellarTransactionRequest::from_json(json)?;
//             Ok(NetworkTransactionRequest::Stellar(request))
//         }
//     }
// }

// // Define a trait for a generic transaction
// pub trait Transaction {
//     fn id(&self) -> &str;
//     fn from(&self) -> &str;
//     fn to(&self) -> &str;
//     fn value(&self) -> u64;
//     fn data(&self) -> &[u8];
//     // Add more methods as necessary, such as sign, validate, etc.
// }

// // Define a struct for an Ethereum transaction
// pub struct EthereumTransaction {
//     pub id: String,
//     pub from: String,
//     pub to: String,
//     pub value: u64,
//     pub data: Vec<u8>,
//     // Ethereum-specific fields
//     pub gas_price: u64,
//     pub gas_limit: u64,
//     pub nonce: u64,
//     pub chain_id: u64,
//     pub signature: Option<Vec<u8>>,
// }

// // Implement the Transaction trait for EthereumTransaction
// impl Transaction for EthereumTransaction {
//     pub fn new(
//         id: String,
//         from: String,
//         to: String,
//         value: u64,
//         data: Vec<u8>,
//         gas_price: u64,
//         gas_limit: u64,
//         nonce: u64,
//         chain_id: u64,
//     ) -> Self {
//         EthereumTransaction {
//             id,
//             from,
//             to,
//             value,
//             data,
//             gas_price,
//             gas_limit,
//             nonce,
//             chain_id,
//             signature: None, // Initially unsigned
//         }
//     }

//     fn id(&self) -> &str {
//         &self.id
//     }

//     fn from(&self) -> &str {
//         &self.from
//     }

//     fn to(&self) -> &str {
//         &self.to
//     }

//     fn value(&self) -> u64 {
//         self.value
//     }

//     fn data(&self) -> &[u8] {
//         &self.data
//     }

//     // Implement additional methods specific to Ethereum transactions
// }

// // Define a struct for a Solana transaction
// pub struct SolanaTransaction {
//     // Solana-specific fields
//     // ...
// }

// // Implement the Transaction trait for SolanaTransaction
// impl Transaction for SolanaTransaction {
//     // Implement the methods for Solana transactions
//     // ...
// }

// // ... Implementations for other network transactions ...
