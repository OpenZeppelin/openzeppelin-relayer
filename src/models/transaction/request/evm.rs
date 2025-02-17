use crate::models::ApiError;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Fastest,
    Fast,
    Average,
    Slow,
}
impl EvmTransactionRequest {
    pub fn validate_evm_transaction_request(
        request: &EvmTransactionRequest,
    ) -> Result<(), ApiError> {
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
}
