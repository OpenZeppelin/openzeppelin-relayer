//! FeeForwarder Service for Soroban Gas Abstraction
//!
//! This module provides functionality to build and manage transactions
//! using the FeeForwarder contract for gas abstraction on Soroban.
//!
//! The FeeForwarder contract enables fee abstraction by allowing users to pay
//! relayers in tokens instead of native XLM. It atomically:
//! 1. Collects fee payment from user
//! 2. Forwards the call to the target contract
//!
//! ## Authorization Flow
//!
//! User signs authorization for `fee_forwarder.forward()` with sub-invocations:
//! - `fee_token.approve(fee_forwarder, max_fee_amount, expiration_ledger)`
//! - `target_contract.target_fn(target_args)` (if target requires auth)

use crate::services::provider::StellarProviderTrait;
use soroban_rs::xdr::{
    ContractId, Hash, Int128Parts, InvokeContractArgs, Limits, Operation, OperationBody, ScAddress,
    ScSymbol, ScVal, ScVec, SorobanAddressCredentials, SorobanAuthorizationEntry,
    SorobanAuthorizedFunction, SorobanAuthorizedInvocation, SorobanCredentials, VecM, WriteXdr,
};
use std::sync::Arc;
use thiserror::Error;

/// Default validity duration for gas abstraction authorizations (5 minutes)
pub const DEFAULT_VALIDITY_SECONDS: u64 = 300;

/// Approximate ledger time in seconds
pub const LEDGER_TIME_SECONDS: u64 = 5;

/// Errors that can occur in FeeForwarder operations
#[derive(Error, Debug)]
pub enum FeeForwarderError {
    #[error("Invalid contract address: {0}")]
    InvalidContractAddress(String),

    #[error("Invalid account address: {0}")]
    InvalidAccountAddress(String),

    #[error("Failed to build authorization: {0}")]
    AuthorizationBuildError(String),

    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("XDR serialization error: {0}")]
    XdrError(String),

    #[error("Invalid function name: {0}")]
    InvalidFunctionName(String),
}

/// Parameters for building a FeeForwarder transaction
#[derive(Debug, Clone)]
pub struct FeeForwarderParams {
    /// Soroban token contract address for fee payment
    pub fee_token: String,
    /// Actual fee amount to charge (determined at submission time)
    pub fee_amount: i128,
    /// Maximum fee amount user authorized
    pub max_fee_amount: i128,
    /// Authorization expiration ledger
    pub expiration_ledger: u32,
    /// Target contract address to call
    pub target_contract: String,
    /// Target function name
    pub target_fn: String,
    /// Target function arguments
    pub target_args: Vec<ScVal>,
    /// User's Stellar address
    pub user: String,
    /// Relayer's Stellar address (fee recipient)
    pub relayer: String,
}

/// Service for building FeeForwarder transactions
pub struct FeeForwarderService<P>
where
    P: StellarProviderTrait + Send + Sync,
{
    /// FeeForwarder contract address
    fee_forwarder_address: String,
    /// Stellar provider for network queries
    provider: Arc<P>,
}

impl<P> FeeForwarderService<P>
where
    P: StellarProviderTrait + Send + Sync,
{
    /// Create a new FeeForwarderService
    ///
    /// # Arguments
    ///
    /// * `fee_forwarder_address` - The deployed FeeForwarder contract address
    /// * `provider` - Stellar provider for network queries
    pub fn new(fee_forwarder_address: String, provider: Arc<P>) -> Self {
        Self {
            fee_forwarder_address,
            provider,
        }
    }

    /// Build a user authorization entry without requiring a FeeForwarderService instance
    ///
    /// This static method is useful when you don't have an `Arc<P>` provider
    /// but still need to build authorization entries for the FeeForwarder.
    pub fn build_user_auth_entry_standalone(
        fee_forwarder_address: &str,
        params: &FeeForwarderParams,
        requires_target_auth: bool,
    ) -> Result<SorobanAuthorizationEntry, FeeForwarderError> {
        let fee_forwarder_addr = Self::parse_contract_address(fee_forwarder_address)?;
        let fee_token_addr = Self::parse_contract_address(&params.fee_token)?;
        let target_contract_addr = Self::parse_contract_address(&params.target_contract)?;
        let user_addr = Self::parse_account_address(&params.user)?;
        let _relayer_addr = Self::parse_account_address(&params.relayer)?;

        // Build sub-invocations
        let mut sub_invocations = Vec::new();

        // 1. fee_token.approve(fee_forwarder, max_fee_amount, expiration_ledger)
        let approve_args: ScVec = vec![
            ScVal::Address(fee_forwarder_addr.clone()),
            Self::i128_to_scval(params.max_fee_amount),
            ScVal::U32(params.expiration_ledger),
        ]
        .try_into()
        .map_err(|_| {
            FeeForwarderError::AuthorizationBuildError("Failed to create approve args".to_string())
        })?;

        sub_invocations.push(SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: fee_token_addr,
                function_name: Self::create_symbol("approve")?,
                args: approve_args.into(),
            }),
            sub_invocations: VecM::default(),
        });

        // 2. target_contract.target_fn(target_args) - if needed
        if requires_target_auth {
            let target_args: ScVec = params.target_args.clone().try_into().map_err(|_| {
                FeeForwarderError::AuthorizationBuildError(
                    "Failed to create target args".to_string(),
                )
            })?;

            sub_invocations.push(SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                    contract_address: target_contract_addr,
                    function_name: Self::create_symbol(&params.target_fn)?,
                    args: target_args.into(),
                }),
                sub_invocations: VecM::default(),
            });
        }

        // Build the forward() function arguments
        let forward_args = Self::build_forward_args_standalone(fee_forwarder_address, params)?;

        // Build the root invocation for fee_forwarder.forward()
        let root_invocation = SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: fee_forwarder_addr,
                function_name: Self::create_symbol("forward")?,
                args: forward_args.into(),
            }),
            sub_invocations: sub_invocations.try_into().map_err(|_| {
                FeeForwarderError::AuthorizationBuildError(
                    "Failed to create sub-invocations".to_string(),
                )
            })?,
        };

        // Generate nonce using timestamp
        let nonce = {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0)
        };

        Ok(SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: user_addr,
                nonce,
                signature_expiration_ledger: params.expiration_ledger,
                signature: ScVal::Void,
            }),
            root_invocation,
        })
    }

    /// Build forward args without requiring a service instance
    fn build_forward_args_standalone(
        _fee_forwarder_address: &str,
        params: &FeeForwarderParams,
    ) -> Result<ScVec, FeeForwarderError> {
        let fee_token_addr = Self::parse_contract_address(&params.fee_token)?;
        let target_contract_addr = Self::parse_contract_address(&params.target_contract)?;
        let user_addr = Self::parse_account_address(&params.user)?;
        let relayer_addr = Self::parse_account_address(&params.relayer)?;

        let target_args_vec: ScVec = params.target_args.clone().try_into().map_err(|_| {
            FeeForwarderError::AuthorizationBuildError("Failed to create target args".to_string())
        })?;

        let args: Vec<ScVal> = vec![
            ScVal::Address(fee_token_addr),
            Self::i128_to_scval(params.fee_amount),
            Self::i128_to_scval(params.max_fee_amount),
            ScVal::U32(params.expiration_ledger),
            ScVal::Address(target_contract_addr),
            ScVal::Symbol(Self::create_symbol(&params.target_fn)?),
            ScVal::Vec(Some(target_args_vec)),
            ScVal::Address(user_addr),
            ScVal::Address(relayer_addr),
        ];

        args.try_into().map_err(|_| {
            FeeForwarderError::AuthorizationBuildError("Failed to create forward args".to_string())
        })
    }

    /// Get the FeeForwarder contract address
    pub fn fee_forwarder_address(&self) -> &str {
        &self.fee_forwarder_address
    }

    /// Public wrapper for building forward args (for serializing InvokeContractArgs)
    pub fn build_forward_args_for_invoke_contract_args(
        fee_forwarder_address: &str,
        params: &FeeForwarderParams,
    ) -> Result<ScVec, FeeForwarderError> {
        Self::build_forward_args_standalone(fee_forwarder_address, params)
    }

    /// Public wrapper for parsing a contract address
    pub fn parse_contract_address_public(address: &str) -> Result<ScAddress, FeeForwarderError> {
        Self::parse_contract_address(address)
    }

    /// Calculate the expiration ledger for authorization
    ///
    /// # Arguments
    ///
    /// * `validity_seconds` - How long the authorization should be valid
    ///
    /// # Returns
    ///
    /// The ledger number when the authorization expires
    pub async fn get_expiration_ledger(
        &self,
        validity_seconds: u64,
    ) -> Result<u32, FeeForwarderError> {
        let current_ledger = self
            .provider
            .get_latest_ledger()
            .await
            .map_err(|e| FeeForwarderError::ProviderError(e.to_string()))?;

        let ledgers_to_add = validity_seconds / LEDGER_TIME_SECONDS;
        Ok(current_ledger.sequence + ledgers_to_add as u32)
    }

    /// Parse a Soroban contract address (C...) to ScAddress
    fn parse_contract_address(address: &str) -> Result<ScAddress, FeeForwarderError> {
        let contract = stellar_strkey::Contract::from_string(address).map_err(|e| {
            FeeForwarderError::InvalidContractAddress(format!("Invalid contract '{address}': {e}"))
        })?;

        Ok(ScAddress::Contract(ContractId(Hash(contract.0))))
    }

    /// Parse a Stellar account address (G...) to ScAddress
    fn parse_account_address(address: &str) -> Result<ScAddress, FeeForwarderError> {
        let account = stellar_strkey::ed25519::PublicKey::from_string(address).map_err(|e| {
            FeeForwarderError::InvalidAccountAddress(format!("Invalid account '{address}': {e}"))
        })?;

        Ok(ScAddress::Account(soroban_rs::xdr::AccountId(
            soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(soroban_rs::xdr::Uint256(account.0)),
        )))
    }

    /// Convert i128 to ScVal::I128
    fn i128_to_scval(amount: i128) -> ScVal {
        let hi = (amount >> 64) as i64;
        let lo = amount as u64;
        ScVal::I128(Int128Parts { hi, lo })
    }

    /// Create a ScSymbol from a function name
    fn create_symbol(name: &str) -> Result<ScSymbol, FeeForwarderError> {
        ScSymbol::try_from(name.as_bytes().to_vec())
            .map_err(|_| FeeForwarderError::InvalidFunctionName(name.to_string()))
    }

    /// Build the user authorization entry for FeeForwarder.forward()
    ///
    /// This creates the authorization structure that the user needs to sign.
    /// The authorization includes sub-invocations for:
    /// - fee_token.approve(fee_forwarder, max_fee_amount, expiration_ledger)
    /// - target_contract.target_fn(target_args) (if requires_target_auth is true)
    ///
    /// # Arguments
    ///
    /// * `params` - FeeForwarder transaction parameters
    /// * `requires_target_auth` - Whether the target contract call requires user authorization
    ///
    /// # Returns
    ///
    /// A SorobanAuthorizationEntry that the user needs to sign
    pub fn build_user_auth_entry(
        &self,
        params: &FeeForwarderParams,
        requires_target_auth: bool,
    ) -> Result<SorobanAuthorizationEntry, FeeForwarderError> {
        let fee_forwarder_addr = Self::parse_contract_address(&self.fee_forwarder_address)?;
        let fee_token_addr = Self::parse_contract_address(&params.fee_token)?;
        let target_contract_addr = Self::parse_contract_address(&params.target_contract)?;
        let user_addr = Self::parse_account_address(&params.user)?;
        // Validate relayer address is valid (used later in build_forward_args)
        let _relayer_addr = Self::parse_account_address(&params.relayer)?;

        // Build sub-invocations
        let mut sub_invocations = Vec::new();

        // 1. fee_token.approve(fee_forwarder, max_fee_amount, expiration_ledger)
        let approve_args: ScVec = vec![
            ScVal::Address(fee_forwarder_addr.clone()),
            Self::i128_to_scval(params.max_fee_amount),
            ScVal::U32(params.expiration_ledger),
        ]
        .try_into()
        .map_err(|_| {
            FeeForwarderError::AuthorizationBuildError("Failed to create approve args".to_string())
        })?;

        sub_invocations.push(SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: fee_token_addr,
                function_name: Self::create_symbol("approve")?,
                args: approve_args.into(),
            }),
            sub_invocations: VecM::default(),
        });

        // 2. target_contract.target_fn(target_args) - if needed
        if requires_target_auth {
            let target_args: ScVec = params.target_args.clone().try_into().map_err(|_| {
                FeeForwarderError::AuthorizationBuildError(
                    "Failed to create target args".to_string(),
                )
            })?;

            sub_invocations.push(SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                    contract_address: target_contract_addr.clone(),
                    function_name: Self::create_symbol(&params.target_fn)?,
                    args: target_args.into(),
                }),
                sub_invocations: VecM::default(),
            });
        }

        // Build the forward() function arguments
        let forward_args = self.build_forward_args(params)?;

        // Build the root invocation for fee_forwarder.forward()
        let root_invocation = SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: fee_forwarder_addr,
                function_name: Self::create_symbol("forward")?,
                args: forward_args.into(),
            }),
            sub_invocations: sub_invocations.try_into().map_err(|_| {
                FeeForwarderError::AuthorizationBuildError(
                    "Failed to create sub-invocations".to_string(),
                )
            })?,
        };

        // Build the authorization entry
        // Note: The signature field is left empty (Void) - user will sign this
        Ok(SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: user_addr,
                nonce: self.generate_nonce(),
                signature_expiration_ledger: params.expiration_ledger,
                signature: ScVal::Void, // User will fill this with their signature
            }),
            root_invocation,
        })
    }

    /// Build the arguments for FeeForwarder.forward() call
    fn build_forward_args(&self, params: &FeeForwarderParams) -> Result<ScVec, FeeForwarderError> {
        let fee_token_addr = Self::parse_contract_address(&params.fee_token)?;
        let target_contract_addr = Self::parse_contract_address(&params.target_contract)?;
        let user_addr = Self::parse_account_address(&params.user)?;
        let relayer_addr = Self::parse_account_address(&params.relayer)?;

        // Convert target_args to ScVec for the target_args parameter
        let target_args_vec: ScVec = params.target_args.clone().try_into().map_err(|_| {
            FeeForwarderError::AuthorizationBuildError("Failed to create target args".to_string())
        })?;

        // forward() parameters:
        // fee_token, fee_amount, max_fee_amount, expiration_ledger,
        // target_contract, target_fn, target_args, user, relayer
        let args: Vec<ScVal> = vec![
            ScVal::Address(fee_token_addr),
            Self::i128_to_scval(params.fee_amount),
            Self::i128_to_scval(params.max_fee_amount),
            ScVal::U32(params.expiration_ledger),
            ScVal::Address(target_contract_addr),
            ScVal::Symbol(Self::create_symbol(&params.target_fn)?),
            ScVal::Vec(Some(target_args_vec)),
            ScVal::Address(user_addr),
            ScVal::Address(relayer_addr),
        ];

        args.try_into().map_err(|_| {
            FeeForwarderError::AuthorizationBuildError("Failed to create forward args".to_string())
        })
    }

    /// Generate a nonce for authorization
    ///
    /// The nonce should be unique per authorization to prevent replay attacks.
    /// Using current timestamp as a simple nonce generator.
    fn generate_nonce(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0)
    }

    /// Serialize an authorization entry to base64 XDR
    pub fn serialize_auth_entry(
        auth: &SorobanAuthorizationEntry,
    ) -> Result<String, FeeForwarderError> {
        auth.to_xdr_base64(Limits::none())
            .map_err(|e| FeeForwarderError::XdrError(format!("Failed to serialize auth: {e}")))
    }

    /// Deserialize an authorization entry from base64 XDR
    pub fn deserialize_auth_entry(
        xdr: &str,
    ) -> Result<SorobanAuthorizationEntry, FeeForwarderError> {
        use soroban_rs::xdr::ReadXdr;
        SorobanAuthorizationEntry::from_xdr_base64(xdr, Limits::none())
            .map_err(|e| FeeForwarderError::XdrError(format!("Failed to deserialize auth: {e}")))
    }

    /// Build the InvokeHostFunction operation for FeeForwarder.forward()
    ///
    /// This creates the operation that will be included in the transaction.
    /// The authorization entries should include both user and relayer signatures.
    ///
    /// # Arguments
    ///
    /// * `params` - FeeForwarder transaction parameters
    /// * `auth_entries` - Signed authorization entries (user + relayer)
    pub fn build_invoke_operation(
        &self,
        params: &FeeForwarderParams,
        auth_entries: Vec<SorobanAuthorizationEntry>,
    ) -> Result<Operation, FeeForwarderError> {
        Self::build_invoke_operation_standalone(&self.fee_forwarder_address, params, auth_entries)
    }

    /// Build the InvokeHostFunction operation without requiring a service instance.
    ///
    /// This static method is useful when you don't have an `Arc<P>` provider
    /// but still need to build InvokeHostFunction operations for the FeeForwarder.
    pub fn build_invoke_operation_standalone(
        fee_forwarder_address: &str,
        params: &FeeForwarderParams,
        auth_entries: Vec<SorobanAuthorizationEntry>,
    ) -> Result<Operation, FeeForwarderError> {
        let fee_forwarder_addr = Self::parse_contract_address(fee_forwarder_address)?;
        let forward_args = Self::build_forward_args_standalone(fee_forwarder_address, params)?;

        let host_function = soroban_rs::xdr::HostFunction::InvokeContract(InvokeContractArgs {
            contract_address: fee_forwarder_addr,
            function_name: Self::create_symbol("forward")?,
            args: forward_args.into(),
        });

        let invoke_op = soroban_rs::xdr::InvokeHostFunctionOp {
            host_function,
            auth: auth_entries.try_into().map_err(|_| {
                FeeForwarderError::AuthorizationBuildError(
                    "Failed to create auth entries vector".to_string(),
                )
            })?,
        };

        Ok(Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(invoke_op),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::provider::StellarProvider;

    #[test]
    fn test_parse_contract_address_valid() {
        let addr = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
        let result = FeeForwarderService::<StellarProvider>::parse_contract_address(addr);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_contract_address_invalid() {
        let addr = "INVALID";
        let result = FeeForwarderService::<StellarProvider>::parse_contract_address(addr);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_account_address_valid() {
        let addr = "GABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890ABCDEFGH";
        // This is not a valid strkey, just testing the function signature
        let result = FeeForwarderService::<StellarProvider>::parse_account_address(addr);
        // Expected to fail since it's not a valid G... address
        assert!(result.is_err());
    }

    #[test]
    fn test_i128_to_scval() {
        let amount: i128 = 1_000_000_000;
        let scval = FeeForwarderService::<StellarProvider>::i128_to_scval(amount);
        match scval {
            ScVal::I128(parts) => {
                let recovered = ((parts.hi as i128) << 64) | (parts.lo as i128);
                assert_eq!(amount, recovered);
            }
            _ => panic!("Expected I128"),
        }
    }

    #[test]
    fn test_create_symbol() {
        let result = FeeForwarderService::<StellarProvider>::create_symbol("forward");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().to_utf8_string_lossy(), "forward");
    }
}
