use crate::models::{ApiError, NetworkType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Fastest,
    Fast,
    Average,
    Slow,
}

#[derive(Deserialize, Serialize, Default)]
pub struct EvmTransactionRequest {
    pub to: Option<String>,
    pub value: u64,
    pub data: Option<String>,
    pub gas_limit: u128,
    pub gas_price: u128,
    pub speed: Option<Speed>,
    pub max_fee_per_gas: Option<u128>,
    pub max_priority_fee_per_gas: Option<u128>,
    pub valid_until: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct SolanaTransactionRequest {
    pub fee_payer: String,
    pub instructions: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct StellarTransactionRequest {
    pub source_account: String,
    pub destination_account: String,
    pub amount: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub memo: Option<String>,
    pub fee: u128,
    pub sequence_number: String,
}

#[derive(Serialize)]
pub enum NetworkTransactionRequest {
    Evm(EvmTransactionRequest),
    Solana(SolanaTransactionRequest),
    Stellar(StellarTransactionRequest),
}

impl NetworkTransactionRequest {
    pub fn from_json(
        network_type: &NetworkType,
        json: serde_json::Value,
    ) -> Result<Self, ApiError> {
        match network_type {
            NetworkType::Evm => Ok(Self::Evm(
                serde_json::from_value(json).map_err(|e| ApiError::BadRequest(e.to_string()))?,
            )),
            NetworkType::Solana => Ok(Self::Solana(
                serde_json::from_value(json).map_err(|e| ApiError::BadRequest(e.to_string()))?,
            )),
            NetworkType::Stellar => Ok(Self::Stellar(
                serde_json::from_value(json).map_err(|e| ApiError::BadRequest(e.to_string()))?,
            )),
        }
    }
    pub fn validate(&self) -> Result<(), ApiError> {
        match self {
            NetworkTransactionRequest::Evm(request) => validate_evm_transaction_request(request),
            _ => Ok(()),
        }
    }
}

fn validate_evm_transaction_request(request: &EvmTransactionRequest) -> Result<(), ApiError> {
    if request.to.is_none() && request.data.is_none() {
        return Err(ApiError::BadRequest(
            "Both txs `to` and `data` fields are missing. At least one of them has to be set."
                .to_string(),
        ));
    }

    if let (Some(max_fee), Some(max_priority_fee)) =
        (request.max_fee_per_gas, request.max_priority_fee_per_gas)
    {
        if max_fee < max_priority_fee {
            return Err(ApiError::BadRequest(
                "maxFeePerGas should be greater or equal to maxPriorityFeePerGas".to_string(),
            ));
        }
    }

    if let Some(valid_until) = &request.valid_until {
        match chrono::DateTime::parse_from_rfc3339(valid_until) {
            Ok(valid_until_dt) => {
                let now = chrono::Utc::now();
                if valid_until_dt < now {
                    return Err(ApiError::BadRequest(
                        "The validUntil time cannot be in the past".to_string(),
                    ));
                }
            }
            Err(_) => {
                return Err(ApiError::BadRequest(
                    "Invalid validUntil datetime format".to_string(),
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_evm_eip1559_fees() {
        let request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("0x123".to_string()),
            data: Some("0x".to_string()),
            max_fee_per_gas: Some(100),
            max_priority_fee_per_gas: Some(50),
            ..Default::default()
        });
        assert!(request.validate().is_ok());

        let invalid_request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("0x123".to_string()),
            data: Some("0x".to_string()),
            max_fee_per_gas: Some(50),
            max_priority_fee_per_gas: Some(100),
            ..Default::default()
        });
        let err = invalid_request.validate().unwrap_err();
        match err {
            ApiError::BadRequest(msg) => {
                assert_eq!(
                    msg,
                    "maxFeePerGas should be greater or equal to maxPriorityFeePerGas"
                );
            }
            _ => panic!("Expected BadRequest error"),
        }
    }

    #[test]
    fn test_validate_evm_empty_fields() {
        // Test both empty to and data fields
        let request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("".to_string()),
            data: Some("".to_string()),
            ..Default::default()
        });
        let err = request.validate().unwrap_err();
        match err {
            ApiError::BadRequest(msg) => {
                assert_eq!(
                    msg,
                    "Both txs `to` and `data` fields are missing. At least one of them has to be \
                     set."
                );
            }
            _ => panic!("Expected BadRequest error"),
        }

        let request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("0x123".to_string()),
            data: Some("".to_string()),
            ..Default::default()
        });
        assert!(request.validate().is_ok());

        let request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("".to_string()),
            data: Some("0x123".to_string()),
            ..Default::default()
        });
        assert!(request.validate().is_ok());
    }

    #[test]
    fn test_validate_evm_valid_until() {
        let future_time = (chrono::Utc::now() + chrono::Duration::hours(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("0x123".to_string()),
            valid_until: Some(future_time),
            ..Default::default()
        });
        assert!(request.validate().is_ok());

        // Test invalid past timestamp
        let past_time = (chrono::Utc::now() - chrono::Duration::hours(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("0x123".to_string()),
            valid_until: Some(past_time),
            ..Default::default()
        });
        let err = request.validate().unwrap_err();
        match err {
            ApiError::BadRequest(msg) => {
                assert_eq!(msg, "The validUntil time cannot be in the past");
            }
            _ => panic!("Expected BadRequest error"),
        }

        // Test invalid datetime format
        let request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("0x123".to_string()),
            valid_until: Some("invalid-datetime".to_string()),
            ..Default::default()
        });
        let err = request.validate().unwrap_err();
        match err {
            ApiError::BadRequest(msg) => {
                assert_eq!(msg, "Invalid validUntil datetime format");
            }
            _ => panic!("Expected BadRequest error"),
        }
    }

    #[test]
    fn test_validate_evm_valid_until_iso8601() {
        let request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("0x123".to_string()),
            valid_until: Some("2024-07-19T23:49:28.754Z".to_string()),
            ..Default::default()
        });

        if chrono::DateTime::parse_from_rfc3339("2024-07-19T23:49:28.754Z").unwrap()
            > chrono::Utc::now()
        {
            assert!(request.validate().is_ok());
        }
    }
}
