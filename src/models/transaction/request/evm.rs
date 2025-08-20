use crate::{
    constants::ZERO_ADDRESS,
    models::{ApiError, RelayerNetworkPolicy, RelayerRepoModel, U256},
    utils::calculate_intrinsic_gas,
};
use serde::{Deserialize, Serialize};
use utoipa::{schema, ToSchema};

#[derive(Deserialize, Serialize, Default, ToSchema)]
pub struct EvmTransactionRequest {
    #[schema(nullable = false)]
    pub to: Option<String>,
    #[schema(value_type = u128, format = "u128")]
    pub value: U256,
    #[schema(nullable = false)]
    pub data: Option<String>,
    pub gas_limit: Option<u64>,
    #[schema(nullable = false)]
    pub gas_price: Option<u128>,
    #[schema(nullable = false)]
    pub speed: Option<Speed>,
    #[schema(nullable = false)]
    pub max_fee_per_gas: Option<u128>,
    #[schema(nullable = false)]
    pub max_priority_fee_per_gas: Option<u128>,
    #[schema(nullable = false)]
    pub valid_until: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Fastest,
    Fast,
    Average,
    #[serde(rename = "safeLow")]
    SafeLow,
}
impl EvmTransactionRequest {
    pub fn validate(&self, relayer: &RelayerRepoModel) -> Result<(), ApiError> {
        validate_target_address(self, relayer)?;
        validate_evm_transaction_request(self, relayer)?;
        validate_price_params(self, relayer)?;
        Ok(())
    }
}

pub fn validate_evm_transaction_request(
    request: &EvmTransactionRequest,
    relayer: &RelayerRepoModel,
) -> Result<(), ApiError> {
    if request.to.is_none() && request.data.is_none() {
        return Err(ApiError::BadRequest(
            "Both txs `to` and `data` fields are missing. At least one of them has to be set."
                .to_string(),
        ));
    }

    // Validate gas_limit based on gas_limit_estimation policy
    if let RelayerNetworkPolicy::Evm(evm_policy) = &relayer.policies {
        // If gas_limit_estimation is disabled (Some(false)), gas_limit must be provided
        if evm_policy.gas_limit_estimation == Some(false) && request.gas_limit.is_none() {
            return Err(ApiError::BadRequest(
                "gas_limit is required when gas_limit_estimation policy is disabled".to_string(),
            ));
        }
    }

    // Validate intrinsic gas if gas_limit is provided
    if let Some(gas_limit) = request.gas_limit {
        let intrinsic_gas = calculate_intrinsic_gas(request);
        if gas_limit < intrinsic_gas {
            return Err(ApiError::BadRequest(format!(
                "gas_limit is too low, intrinsic gas is {} and gas_limit is {}",
                intrinsic_gas, gas_limit
            )));
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

pub fn validate_price_params(
    request: &EvmTransactionRequest,
    relayer: &RelayerRepoModel,
) -> Result<(), ApiError> {
    let is_eip1559 =
        request.max_fee_per_gas.is_some() || request.max_priority_fee_per_gas.is_some();
    let is_legacy = request.gas_price.is_some();
    let is_speed = request.speed.is_some();

    // count how many transaction types are present
    let transaction_types = [is_eip1559, is_legacy, is_speed]
        .iter()
        .filter(|&&x| x)
        .count();

    // validate that only one transaction type is present
    if transaction_types == 0 {
        return Err(ApiError::BadRequest(
            "Transaction must specify either gasPrice, speed, or EIP1559 parameters".to_string(),
        ));
    }

    if transaction_types > 1 {
        return Err(ApiError::BadRequest(
            "Cannot mix different transaction types. Use either gasPrice, speed, or EIP1559 \
             parameters"
                .to_string(),
        ));
    }

    // validate specific fields based on the type
    if is_eip1559 {
        // for eip1559, both fields must be present
        match (request.max_fee_per_gas, request.max_priority_fee_per_gas) {
            (Some(_), None) | (None, Some(_)) => {
                return Err(ApiError::BadRequest(
                    "EIP1559 transactions require both maxFeePerGas and maxPriorityFeePerGas"
                        .to_string(),
                ));
            }
            (Some(max_fee), Some(max_priority_fee)) => {
                if max_fee < max_priority_fee {
                    return Err(ApiError::BadRequest(
                        "maxFeePerGas must be greater than or equal to maxPriorityFeePerGas"
                            .to_string(),
                    ));
                }
            }
            _ => unreachable!(),
        }
    }

    if is_legacy {
        if let RelayerNetworkPolicy::Evm(evm_policy) = &relayer.policies {
            if let Some(gas_price_cap) = evm_policy.gas_price_cap {
                if request.gas_price.unwrap_or(0) > gas_price_cap {
                    return Err(ApiError::BadRequest("Gas price is too high".to_string()));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::models::{NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RpcConfig};

    use super::*;
    use chrono::{Duration, Utc};

    fn create_basic_request() -> EvmTransactionRequest {
        EvmTransactionRequest {
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(0),
            data: Some("0x".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(0),
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
            custom_rpc_urls: Some(vec![RpcConfig::new("https://test-rpc-url".to_string())]),
        }
    }

    #[test]
    fn test_validate_evm_transaction_request_valid() {
        let request = create_basic_request();
        assert!(
            validate_evm_transaction_request(&request, &create_test_relayer(false, false)).is_ok()
        );
    }

    #[test]
    fn test_validate_missing_to_and_data() {
        let mut request = create_basic_request();
        request.to = None;
        request.data = None;

        let result = validate_evm_transaction_request(&request, &create_test_relayer(false, false));
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_valid_until_past() {
        let mut request = create_basic_request();
        let past_time = Utc::now() - Duration::hours(1);
        request.valid_until = Some(past_time.to_rfc3339());

        let result = validate_evm_transaction_request(&request, &create_test_relayer(false, false));
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_valid_until_future() {
        let mut request = create_basic_request();
        let future_time = Utc::now() + Duration::hours(1);
        request.valid_until = Some(future_time.to_rfc3339());

        assert!(
            validate_evm_transaction_request(&request, &create_test_relayer(false, false)).is_ok()
        );
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

    #[test]
    fn test_validate_evm_transaction_request_gas_limit_too_low() {
        let mut request = create_basic_request();
        request.gas_limit = Some(20000);
        let result = validate_evm_transaction_request(&request, &create_test_relayer(false, false));
        assert!(result.is_err());

        if let Err(ApiError::BadRequest(msg)) = result {
            assert_eq!(
                msg,
                "gas_limit is too low, intrinsic gas is 21000 and gas_limit is 20000".to_string()
            );
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[test]
    fn test_validate_legacy_transaction() {
        let request = create_basic_request();
        assert!(
            validate_evm_transaction_request(&request, &create_test_relayer(false, false)).is_ok()
        );
    }

    #[test]
    fn test_validate_eip1559_transaction() {
        let mut request = create_basic_request();
        request.max_fee_per_gas = Some(30000000000);
        request.max_priority_fee_per_gas = Some(20000000000);

        assert!(
            validate_evm_transaction_request(&request, &create_test_relayer(false, false)).is_ok()
        );
    }

    #[test]
    fn test_validate_eip1559_invalid_fees() {
        let mut request = create_basic_request();
        request.max_fee_per_gas = Some(20000000000);
        request.max_priority_fee_per_gas = Some(30000000000); // max_fee_per_gas should be greater than max_priority_fee_per_gas
        let relayer = create_test_relayer(false, false);
        let result = validate_price_params(&request, &relayer);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }
    #[test]
    fn test_validate_speed_transaction() {
        let mut request = create_basic_request();
        request.speed = Some(Speed::Fast);

        assert!(
            validate_evm_transaction_request(&request, &create_test_relayer(false, false)).is_ok()
        );
    }

    #[test]
    fn test_validate_missing_required_fields() {
        let mut request = create_basic_request();
        request.to = None;
        request.data = None;

        let result = validate_evm_transaction_request(&request, &create_test_relayer(false, false));
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_invalid_valid_until_format() {
        let mut request = create_basic_request();
        request.valid_until = Some("invalid-date-format".to_string());

        let result = validate_evm_transaction_request(&request, &create_test_relayer(false, false));
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_whitelisted_address() {
        let request = create_basic_request();
        let mut relayer = create_test_relayer(false, false);

        if let RelayerNetworkPolicy::Evm(ref mut evm_policy) = relayer.policies {
            evm_policy.whitelist_receivers = Some(vec![
                "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            ]);
        }

        assert!(validate_target_address(&request, &relayer).is_ok());
    }

    #[test]
    fn test_validate_non_whitelisted_address() {
        let mut request = create_basic_request();
        request.to = Some("0x1234567890123456789012345678901234567890".to_string());
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
    fn test_validate_mixed_transaction_types() {
        let mut request = create_basic_request();
        request.gas_price = Some(20000000000);
        request.max_fee_per_gas = Some(30000000000);

        let relayer = create_test_relayer(false, false);
        let result = validate_price_params(&request, &relayer);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_incomplete_eip1559() {
        let mut request = create_basic_request();
        request.max_fee_per_gas = Some(30000000000);
        // Falta max_priority_fee_per_gas

        let relayer = create_test_relayer(false, false);
        let result = validate_price_params(&request, &relayer);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_invalid_eip1559_fees() {
        let mut request = create_basic_request();
        request.max_fee_per_gas = Some(20000000000);
        request.max_priority_fee_per_gas = Some(30000000000); // Mayor que max_fee
        let relayer = create_test_relayer(false, false);
        let result = validate_price_params(&request, &relayer);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_speed_with_gas_price() {
        let mut request = create_basic_request();
        request.speed = Some(Speed::Fast);
        request.gas_price = Some(20000000000);
        let relayer = create_test_relayer(false, false);
        let result = validate_price_params(&request, &relayer);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_gas_price_cap() {
        let mut request = create_basic_request();
        request.gas_price = Some(20000000000);
        let mut relayer = create_test_relayer(false, false);
        if let RelayerNetworkPolicy::Evm(ref mut evm_policy) = relayer.policies {
            evm_policy.gas_price_cap = Some(10000000000);
        }
        let result = validate_price_params(&request, &relayer);
        assert!(result.is_err());
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn test_validate_gas_limit_optional_when_estimation_enabled() {
        let mut request = create_basic_request();
        request.gas_limit = None; // No gas_limit provided

        // Create relayer with gas_limit_estimation enabled (default)
        let relayer = create_test_relayer(false, false);
        // Default RelayerEvmPolicy has gas_limit_estimation = Some(true)

        let result = validate_evm_transaction_request(&request, &relayer);
        assert!(
            result.is_ok(),
            "gas_limit should be optional when gas_limit_estimation is enabled"
        );
    }

    #[test]
    fn test_validate_gas_limit_optional_when_estimation_explicitly_enabled() {
        let mut request = create_basic_request();
        request.gas_limit = None; // No gas_limit provided

        // Create relayer with gas_limit_estimation explicitly enabled
        let mut relayer = create_test_relayer(false, false);
        if let RelayerNetworkPolicy::Evm(ref mut evm_policy) = relayer.policies {
            evm_policy.gas_limit_estimation = Some(true);
        }

        let result = validate_evm_transaction_request(&request, &relayer);
        assert!(
            result.is_ok(),
            "gas_limit should be optional when gas_limit_estimation is explicitly enabled"
        );
    }

    #[test]
    fn test_validate_gas_limit_required_when_estimation_disabled() {
        let mut request = create_basic_request();
        request.gas_limit = None; // No gas_limit provided

        // Create relayer with gas_limit_estimation disabled
        let mut relayer = create_test_relayer(false, false);
        if let RelayerNetworkPolicy::Evm(ref mut evm_policy) = relayer.policies {
            evm_policy.gas_limit_estimation = Some(false);
        }

        let result = validate_evm_transaction_request(&request, &relayer);
        assert!(
            result.is_err(),
            "gas_limit should be required when gas_limit_estimation is disabled"
        );

        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(
                msg.contains("gas_limit is required when gas_limit_estimation policy is disabled"),
                "Expected specific error message, got: {}",
                msg
            );
        } else {
            panic!("Expected BadRequest error");
        }
    }

    #[test]
    fn test_validate_gas_limit_provided_when_estimation_disabled() {
        let mut request = create_basic_request();
        request.gas_limit = Some(21000); // gas_limit provided

        // Create relayer with gas_limit_estimation disabled
        let mut relayer = create_test_relayer(false, false);
        if let RelayerNetworkPolicy::Evm(ref mut evm_policy) = relayer.policies {
            evm_policy.gas_limit_estimation = Some(false);
        }

        let result = validate_evm_transaction_request(&request, &relayer);
        assert!(
            result.is_ok(),
            "validation should pass when gas_limit is provided and estimation is disabled"
        );
    }

    #[test]
    fn test_validate_gas_limit_provided_when_estimation_enabled() {
        let mut request = create_basic_request();
        request.gas_limit = Some(21000); // gas_limit provided

        // Create relayer with gas_limit_estimation enabled
        let mut relayer = create_test_relayer(false, false);
        if let RelayerNetworkPolicy::Evm(ref mut evm_policy) = relayer.policies {
            evm_policy.gas_limit_estimation = Some(true);
        }

        let result = validate_evm_transaction_request(&request, &relayer);
        assert!(
            result.is_ok(),
            "validation should pass when gas_limit is provided even when estimation is enabled"
        );
    }
}
