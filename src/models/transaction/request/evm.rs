use crate::{
    constants::ZERO_ADDRESS,
    models::{ApiError, RelayerNetworkPolicy, RelayerRepoModel},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Default)]
pub struct EvmTransactionRequest {
    pub from: String,
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
    pub fn validate(&self, relayer: &RelayerRepoModel) -> Result<(), ApiError> {
        validate_target_address(self, relayer)?;
        validate_evm_transaction_request(self)?;
        Ok(())
    }
}

pub fn validate_evm_transaction_request(request: &EvmTransactionRequest) -> Result<(), ApiError> {
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

pub fn validate_target_address(
    request: &EvmTransactionRequest,
    relayer: &RelayerRepoModel,
) -> Result<(), ApiError> {
    if let RelayerNetworkPolicy::Evm(evm_policy) = &relayer.policies {
        if let Some(whitelist) = &evm_policy.whitelist_receivers {
            let target_address = request.to.clone().unwrap_or_default().to_lowercase();
            let mut allowed_addresses: Vec<String> =
                whitelist.iter().map(|addr| addr.to_lowercase()).collect();
            allowed_addresses.push(ZERO_ADDRESS.to_string());
            allowed_addresses.push(relayer.address.to_lowercase());

            if !allowed_addresses.contains(&target_address) {
                return Err(ApiError::BadRequest(
                    "Transaction target address is not whitelisted".to_string(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::models::{NetworkType, RelayerEvmPolicy};

    use super::*;
    use chrono::{Duration, Utc};

    fn create_basic_request() -> EvmTransactionRequest {
        EvmTransactionRequest {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: 0,
            data: Some("0x".to_string()),
            gas_limit: 21000,
            gas_price: 20000000000,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            valid_until: None,
        }
    }

    fn create_test_relayer(paused: bool, system_disabled: bool) -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test_relayer".to_string(),
            name: "Test Relayer".to_string(),
            paused,
            system_disabled,
            network: "test_network".to_string(),
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            signer_id: "test_signer".to_string(),
            address: "0x".to_string(),
            notification_id: None,
        }
    }

    #[test]
    fn test_validate_evm_transaction_request_valid() {
        let request = create_basic_request();
        assert!(validate_evm_transaction_request(&request).is_ok());
    }

    #[test]
    fn test_validate_missing_to_and_data() {
        let mut request = create_basic_request();
        request.to = None;
        request.data = None;

        let result = validate_evm_transaction_request(&request);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_invalid_gas_fees() {
        let mut request = create_basic_request();
        request.max_fee_per_gas = Some(100);
        request.max_priority_fee_per_gas = Some(200);

        let result = validate_evm_transaction_request(&request);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_valid_until_past() {
        let mut request = create_basic_request();
        let past_time = Utc::now() - Duration::hours(1);
        request.valid_until = Some(past_time.to_rfc3339());

        let result = validate_evm_transaction_request(&request);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_valid_until_future() {
        let mut request = create_basic_request();
        let future_time = Utc::now() + Duration::hours(1);
        request.valid_until = Some(future_time.to_rfc3339());

        assert!(validate_evm_transaction_request(&request).is_ok());
    }

    #[test]
    fn test_validate_target_address_whitelisted() {
        let request = create_basic_request();
        let relayer = create_test_relayer(false, false);

        assert!(validate_target_address(&request, &relayer).is_ok());
    }

    #[test]
    fn test_validate_target_address_not_whitelisted() {
        let mut request = create_basic_request();
        request.to = Some("0xNOTWHITELISTED123456789".to_string());

        let mut relayer = create_test_relayer(false, false);

        if let RelayerNetworkPolicy::Evm(ref mut evm_policy) = relayer.policies {
            evm_policy.whitelist_receivers = Some(vec![
                "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            ]);
        }

        let result = validate_target_address(&request, &relayer);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_target_address_zero_address() {
        let mut request = create_basic_request();
        request.to = Some(ZERO_ADDRESS.to_string());
        let relayer = create_test_relayer(false, false);

        assert!(validate_target_address(&request, &relayer).is_ok());
    }

    #[test]
    fn test_validate_target_address_relayer_address() {
        let mut request = create_basic_request();
        let relayer = create_test_relayer(false, false);
        request.to = Some(relayer.address.clone());

        assert!(validate_target_address(&request, &relayer).is_ok());
    }
}
