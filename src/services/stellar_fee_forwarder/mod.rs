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
//! - `fee_token.approve(user, fee_forwarder, max_fee_amount, expiration_ledger)`
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

        // 1. fee_token.approve(user, fee_forwarder, max_fee_amount, expiration_ledger)
        // Note: When called from another contract, approve requires 'from' address as first arg
        let approve_args: ScVec = vec![
            ScVal::Address(user_addr.clone()),
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

        // Build the forward() function arguments for USER (6 parameters)
        // User signs: fee_token, max_fee_amount, expiration_ledger, target_contract, target_fn, target_args
        // User does NOT sign: fee_amount, user, relayer
        let user_auth_args = Self::build_user_auth_args_standalone(params)?;

        // Build the root invocation for fee_forwarder.forward()
        let root_invocation = SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: fee_forwarder_addr,
                function_name: Self::create_symbol("forward")?,
                args: user_auth_args.into(),
            }),
            sub_invocations: sub_invocations.try_into().map_err(|_| {
                FeeForwarderError::AuthorizationBuildError(
                    "Failed to create sub-invocations".to_string(),
                )
            })?,
        };

        // Generate nonce using timestamp combined with randomness for uniqueness
        let nonce = {
            use rand::Rng;
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            let random: i64 = rand::rng().random();
            timestamp ^ random
        };

        // For simulation, signature must be an empty vector (not Void)
        // Void causes Error(Value, UnexpectedType) during auth verification
        let empty_signature = ScVal::Vec(Some(ScVec::default()));

        Ok(SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: user_addr,
                nonce,
                signature_expiration_ledger: params.expiration_ledger,
                signature: empty_signature,
            }),
            root_invocation,
        })
    }

    /// Build a relayer authorization entry without requiring a FeeForwarderService instance
    ///
    /// The FeeForwarder contract requires the relayer to authorize receiving the fee payment.
    /// This creates an auth entry for the relayer's authorization of the `forward` call.
    ///
    /// # Arguments
    ///
    /// * `fee_forwarder_address` - The FeeForwarder contract address
    /// * `params` - FeeForwarder transaction parameters
    ///
    /// # Returns
    ///
    /// A SorobanAuthorizationEntry for the relayer (with empty signature for simulation)
    pub fn build_relayer_auth_entry_standalone(
        fee_forwarder_address: &str,
        params: &FeeForwarderParams,
    ) -> Result<SorobanAuthorizationEntry, FeeForwarderError> {
        let fee_forwarder_addr = Self::parse_contract_address(fee_forwarder_address)?;
        let relayer_addr = Self::parse_account_address(&params.relayer)?;

        // Build the forward() function arguments
        let forward_args = Self::build_forward_args_standalone(fee_forwarder_address, params)?;

        // Build the root invocation for fee_forwarder.forward()
        // Relayer only needs to authorize the forward call itself (no sub-invocations)
        let root_invocation = SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: fee_forwarder_addr,
                function_name: Self::create_symbol("forward")?,
                args: forward_args.into(),
            }),
            sub_invocations: VecM::default(),
        };

        // Generate nonce using timestamp combined with randomness for uniqueness
        let nonce = {
            use rand::Rng;
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            let random: i64 = rand::rng().random();
            timestamp ^ random
        };

        // For simulation, signature must be an empty vector (not Void)
        // Void causes Error(Value, UnexpectedType) during auth verification
        let empty_signature = ScVal::Vec(Some(ScVec::default()));

        Ok(SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: relayer_addr,
                nonce,
                signature_expiration_ledger: params.expiration_ledger,
                signature: empty_signature,
            }),
            root_invocation,
        })
    }

    /// Build forward args for USER authorization (6 parameters)
    ///
    /// User signs: fee_token, max_fee_amount, expiration_ledger, target_contract, target_fn, target_args
    /// User does NOT sign: fee_amount, user, relayer
    fn build_user_auth_args_standalone(
        params: &FeeForwarderParams,
    ) -> Result<ScVec, FeeForwarderError> {
        let fee_token_addr = Self::parse_contract_address(&params.fee_token)?;
        let target_contract_addr = Self::parse_contract_address(&params.target_contract)?;

        let target_args_vec: ScVec = params.target_args.clone().try_into().map_err(|_| {
            FeeForwarderError::AuthorizationBuildError("Failed to create target args".to_string())
        })?;

        // User signs 6 parameters (excludes fee_amount, user, relayer)
        let args: Vec<ScVal> = vec![
            ScVal::Address(fee_token_addr),
            Self::i128_to_scval(params.max_fee_amount),
            ScVal::U32(params.expiration_ledger),
            ScVal::Address(target_contract_addr),
            ScVal::Symbol(Self::create_symbol(&params.target_fn)?),
            ScVal::Vec(Some(target_args_vec)),
        ];

        args.try_into().map_err(|_| {
            FeeForwarderError::AuthorizationBuildError(
                "Failed to create user auth args".to_string(),
            )
        })
    }

    /// Build forward args for RELAYER authorization and invoke operation (9 parameters)
    ///
    /// Relayer signs ALL parameters in the exact order they appear in the function signature:
    /// fee_token, fee_amount, max_fee_amount, expiration_ledger, target_contract, target_fn, target_args, user, relayer
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

        // Relayer signs all 9 parameters
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
    /// - fee_token.approve(user, fee_forwarder, max_fee_amount, expiration_ledger)
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

        // 1. fee_token.approve(user, fee_forwarder, max_fee_amount, expiration_ledger)
        // Note: When called from another contract, approve requires 'from' address as first arg
        let approve_args: ScVec = vec![
            ScVal::Address(user_addr.clone()),
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

        // Build the forward() function arguments for USER (6 parameters)
        // User signs: fee_token, max_fee_amount, expiration_ledger, target_contract, target_fn, target_args
        // User does NOT sign: fee_amount, user, relayer
        let user_auth_args = Self::build_user_auth_args_standalone(params)?;

        // Build the root invocation for fee_forwarder.forward()
        let root_invocation = SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: fee_forwarder_addr,
                function_name: Self::create_symbol("forward")?,
                args: user_auth_args.into(),
            }),
            sub_invocations: sub_invocations.try_into().map_err(|_| {
                FeeForwarderError::AuthorizationBuildError(
                    "Failed to create sub-invocations".to_string(),
                )
            })?,
        };

        // Build the authorization entry
        // For simulation, signature must be an empty vector (not Void)
        // Void causes Error(Value, UnexpectedType) during auth verification
        // User will replace this with their actual signature after signing
        let empty_signature = ScVal::Vec(Some(ScVec::default()));

        Ok(SorobanAuthorizationEntry {
            credentials: SorobanCredentials::Address(SorobanAddressCredentials {
                address: user_addr,
                nonce: self.generate_nonce(),
                signature_expiration_ledger: params.expiration_ledger,
                signature: empty_signature,
            }),
            root_invocation,
        })
    }

    /// Generate a nonce for authorization
    ///
    /// The nonce should be unique per authorization to prevent replay attacks.
    /// Combines timestamp with randomness for improved uniqueness.
    fn generate_nonce(&self) -> i64 {
        use rand::Rng;
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let random: i64 = rand::rng().random();
        timestamp ^ random
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

    // Test constants
    const VALID_CONTRACT_ADDR: &str = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
    const VALID_ACCOUNT_ADDR: &str = "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
    const VALID_ACCOUNT_ADDR_2: &str = "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";

    fn create_test_params() -> FeeForwarderParams {
        FeeForwarderParams {
            fee_token: VALID_CONTRACT_ADDR.to_string(),
            fee_amount: 1_000_000,
            max_fee_amount: 2_000_000,
            expiration_ledger: 100000,
            target_contract: VALID_CONTRACT_ADDR.to_string(),
            target_fn: "transfer".to_string(),
            target_args: vec![ScVal::U32(42)],
            user: VALID_ACCOUNT_ADDR.to_string(),
            relayer: VALID_ACCOUNT_ADDR_2.to_string(),
        }
    }

    // ==================== parse_contract_address tests ====================

    #[test]
    fn test_parse_contract_address_valid() {
        let result =
            FeeForwarderService::<StellarProvider>::parse_contract_address(VALID_CONTRACT_ADDR);
        assert!(result.is_ok());
        match result.unwrap() {
            ScAddress::Contract(_) => {}
            _ => panic!("Expected Contract address"),
        }
    }

    #[test]
    fn test_parse_contract_address_invalid() {
        let addr = "INVALID";
        let result = FeeForwarderService::<StellarProvider>::parse_contract_address(addr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FeeForwarderError::InvalidContractAddress(_)));
    }

    #[test]
    fn test_parse_contract_address_with_account_address() {
        // Account addresses (G...) should fail when parsed as contract addresses
        let result =
            FeeForwarderService::<StellarProvider>::parse_contract_address(VALID_ACCOUNT_ADDR);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_contract_address_empty() {
        let result = FeeForwarderService::<StellarProvider>::parse_contract_address("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_contract_address_public() {
        let result = FeeForwarderService::<StellarProvider>::parse_contract_address_public(
            VALID_CONTRACT_ADDR,
        );
        assert!(result.is_ok());
    }

    // ==================== parse_account_address tests ====================

    #[test]
    fn test_parse_account_address_valid() {
        let result =
            FeeForwarderService::<StellarProvider>::parse_account_address(VALID_ACCOUNT_ADDR);
        assert!(result.is_ok());
        match result.unwrap() {
            ScAddress::Account(_) => {}
            _ => panic!("Expected Account address"),
        }
    }

    #[test]
    fn test_parse_account_address_valid_2() {
        let result =
            FeeForwarderService::<StellarProvider>::parse_account_address(VALID_ACCOUNT_ADDR_2);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_account_address_invalid_format() {
        let addr = "GABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890ABCDEFGH";
        let result = FeeForwarderService::<StellarProvider>::parse_account_address(addr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FeeForwarderError::InvalidAccountAddress(_)));
    }

    #[test]
    fn test_parse_account_address_with_contract_address() {
        // Contract addresses (C...) should fail when parsed as account addresses
        let result =
            FeeForwarderService::<StellarProvider>::parse_account_address(VALID_CONTRACT_ADDR);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_account_address_empty() {
        let result = FeeForwarderService::<StellarProvider>::parse_account_address("");
        assert!(result.is_err());
    }

    // ==================== i128_to_scval tests ====================

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
    fn test_i128_to_scval_zero() {
        let amount: i128 = 0;
        let scval = FeeForwarderService::<StellarProvider>::i128_to_scval(amount);
        match scval {
            ScVal::I128(parts) => {
                assert_eq!(parts.hi, 0);
                assert_eq!(parts.lo, 0);
            }
            _ => panic!("Expected I128"),
        }
    }

    #[test]
    fn test_i128_to_scval_negative() {
        let amount: i128 = -1_000_000;
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
    fn test_i128_to_scval_max() {
        let amount: i128 = i128::MAX;
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
    fn test_i128_to_scval_min() {
        let amount: i128 = i128::MIN;
        let scval = FeeForwarderService::<StellarProvider>::i128_to_scval(amount);
        match scval {
            ScVal::I128(parts) => {
                let recovered = ((parts.hi as i128) << 64) | (parts.lo as i128);
                assert_eq!(amount, recovered);
            }
            _ => panic!("Expected I128"),
        }
    }

    // ==================== create_symbol tests ====================

    #[test]
    fn test_create_symbol() {
        let result = FeeForwarderService::<StellarProvider>::create_symbol("forward");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().to_utf8_string_lossy(), "forward");
    }

    #[test]
    fn test_create_symbol_approve() {
        let result = FeeForwarderService::<StellarProvider>::create_symbol("approve");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().to_utf8_string_lossy(), "approve");
    }

    #[test]
    fn test_create_symbol_transfer() {
        let result = FeeForwarderService::<StellarProvider>::create_symbol("transfer");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().to_utf8_string_lossy(), "transfer");
    }

    #[test]
    fn test_create_symbol_empty() {
        let result = FeeForwarderService::<StellarProvider>::create_symbol("");
        assert!(result.is_ok());
    }

    // ==================== build_user_auth_entry_standalone tests ====================

    #[test]
    fn test_build_user_auth_entry_standalone_without_target_auth() {
        let params = create_test_params();
        let result = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            false,
        );
        assert!(result.is_ok());

        let auth_entry = result.unwrap();
        match &auth_entry.credentials {
            SorobanCredentials::Address(creds) => {
                assert_eq!(creds.signature_expiration_ledger, params.expiration_ledger);
                // Signature should be empty vec for simulation
                match &creds.signature {
                    ScVal::Vec(Some(v)) => assert!(v.is_empty()),
                    _ => panic!("Expected empty Vec signature"),
                }
            }
            _ => panic!("Expected Address credentials"),
        }

        // Should have 1 sub-invocation (approve only)
        assert_eq!(auth_entry.root_invocation.sub_invocations.len(), 1);
    }

    #[test]
    fn test_build_user_auth_entry_standalone_with_target_auth() {
        let params = create_test_params();
        let result = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            true,
        );
        assert!(result.is_ok());

        let auth_entry = result.unwrap();
        // Should have 2 sub-invocations (approve + target)
        assert_eq!(auth_entry.root_invocation.sub_invocations.len(), 2);
    }

    #[test]
    fn test_build_user_auth_entry_standalone_invalid_fee_forwarder() {
        let params = create_test_params();
        let result = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            "INVALID", &params, false,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FeeForwarderError::InvalidContractAddress(_)
        ));
    }

    #[test]
    fn test_build_user_auth_entry_standalone_invalid_fee_token() {
        let mut params = create_test_params();
        params.fee_token = "INVALID".to_string();
        let result = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_build_user_auth_entry_standalone_invalid_user() {
        let mut params = create_test_params();
        params.user = "INVALID".to_string();
        let result = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            false,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FeeForwarderError::InvalidAccountAddress(_)
        ));
    }

    #[test]
    fn test_build_user_auth_entry_standalone_invalid_relayer() {
        let mut params = create_test_params();
        params.relayer = "INVALID".to_string();
        let result = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            false,
        );
        assert!(result.is_err());
    }

    // ==================== build_relayer_auth_entry_standalone tests ====================

    #[test]
    fn test_build_relayer_auth_entry_standalone() {
        let params = create_test_params();
        let result = FeeForwarderService::<StellarProvider>::build_relayer_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
        );
        assert!(result.is_ok());

        let auth_entry = result.unwrap();
        match &auth_entry.credentials {
            SorobanCredentials::Address(creds) => {
                assert_eq!(creds.signature_expiration_ledger, params.expiration_ledger);
                // Relayer has no sub-invocations
            }
            _ => panic!("Expected Address credentials"),
        }

        // Relayer should have no sub-invocations
        assert!(auth_entry.root_invocation.sub_invocations.is_empty());
    }

    #[test]
    fn test_build_relayer_auth_entry_standalone_invalid_fee_forwarder() {
        let params = create_test_params();
        let result = FeeForwarderService::<StellarProvider>::build_relayer_auth_entry_standalone(
            "INVALID", &params,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_build_relayer_auth_entry_standalone_invalid_relayer() {
        let mut params = create_test_params();
        params.relayer = "INVALID".to_string();
        let result = FeeForwarderService::<StellarProvider>::build_relayer_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
        );
        assert!(result.is_err());
    }

    // ==================== build_forward_args tests ====================

    #[test]
    fn test_build_forward_args_for_invoke_contract_args() {
        let params = create_test_params();
        let result =
            FeeForwarderService::<StellarProvider>::build_forward_args_for_invoke_contract_args(
                VALID_CONTRACT_ADDR,
                &params,
            );
        assert!(result.is_ok());

        let args = result.unwrap();
        // Should have 9 arguments
        assert_eq!(args.len(), 9);
    }

    #[test]
    fn test_build_forward_args_standalone_invalid_fee_token() {
        let mut params = create_test_params();
        params.fee_token = "INVALID".to_string();
        let result =
            FeeForwarderService::<StellarProvider>::build_forward_args_for_invoke_contract_args(
                VALID_CONTRACT_ADDR,
                &params,
            );
        assert!(result.is_err());
    }

    #[test]
    fn test_build_forward_args_standalone_invalid_target_contract() {
        let mut params = create_test_params();
        params.target_contract = "INVALID".to_string();
        let result =
            FeeForwarderService::<StellarProvider>::build_forward_args_for_invoke_contract_args(
                VALID_CONTRACT_ADDR,
                &params,
            );
        assert!(result.is_err());
    }

    // ==================== serialize/deserialize auth entry tests ====================

    #[test]
    fn test_serialize_and_deserialize_auth_entry() {
        let params = create_test_params();
        let auth_entry = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            false,
        )
        .unwrap();

        let serialized = FeeForwarderService::<StellarProvider>::serialize_auth_entry(&auth_entry);
        assert!(serialized.is_ok());

        let xdr_string = serialized.unwrap();
        assert!(!xdr_string.is_empty());

        let deserialized =
            FeeForwarderService::<StellarProvider>::deserialize_auth_entry(&xdr_string);
        assert!(deserialized.is_ok());
    }

    #[test]
    fn test_deserialize_auth_entry_invalid_xdr() {
        let result =
            FeeForwarderService::<StellarProvider>::deserialize_auth_entry("not-valid-base64-xdr!");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            FeeForwarderError::XdrError(_)
        ));
    }

    #[test]
    fn test_deserialize_auth_entry_empty() {
        let result = FeeForwarderService::<StellarProvider>::deserialize_auth_entry("");
        assert!(result.is_err());
    }

    // ==================== build_invoke_operation_standalone tests ====================

    #[test]
    fn test_build_invoke_operation_standalone() {
        let params = create_test_params();
        let user_auth = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            false,
        )
        .unwrap();
        let relayer_auth =
            FeeForwarderService::<StellarProvider>::build_relayer_auth_entry_standalone(
                VALID_CONTRACT_ADDR,
                &params,
            )
            .unwrap();

        let result = FeeForwarderService::<StellarProvider>::build_invoke_operation_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            vec![user_auth, relayer_auth],
        );
        assert!(result.is_ok());

        let operation = result.unwrap();
        assert!(operation.source_account.is_none());
        match operation.body {
            OperationBody::InvokeHostFunction(op) => {
                assert_eq!(op.auth.len(), 2);
            }
            _ => panic!("Expected InvokeHostFunction operation"),
        }
    }

    #[test]
    fn test_build_invoke_operation_standalone_invalid_fee_forwarder() {
        let params = create_test_params();
        let result = FeeForwarderService::<StellarProvider>::build_invoke_operation_standalone(
            "INVALID",
            &params,
            vec![],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_build_invoke_operation_standalone_empty_auth() {
        let params = create_test_params();
        let result = FeeForwarderService::<StellarProvider>::build_invoke_operation_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            vec![],
        );
        assert!(result.is_ok());
    }

    // ==================== FeeForwarderParams tests ====================

    #[test]
    fn test_fee_forwarder_params_clone() {
        let params = create_test_params();
        let cloned = params.clone();
        assert_eq!(params.fee_token, cloned.fee_token);
        assert_eq!(params.fee_amount, cloned.fee_amount);
        assert_eq!(params.max_fee_amount, cloned.max_fee_amount);
        assert_eq!(params.expiration_ledger, cloned.expiration_ledger);
        assert_eq!(params.target_contract, cloned.target_contract);
        assert_eq!(params.target_fn, cloned.target_fn);
        assert_eq!(params.user, cloned.user);
        assert_eq!(params.relayer, cloned.relayer);
    }

    #[test]
    fn test_fee_forwarder_params_with_empty_target_args() {
        let mut params = create_test_params();
        params.target_args = vec![];

        let result = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            true,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_fee_forwarder_params_with_multiple_target_args() {
        let mut params = create_test_params();
        params.target_args = vec![
            ScVal::U32(1),
            ScVal::U32(2),
            ScVal::U32(3),
            ScVal::Bool(true),
        ];

        let result = FeeForwarderService::<StellarProvider>::build_user_auth_entry_standalone(
            VALID_CONTRACT_ADDR,
            &params,
            true,
        );
        assert!(result.is_ok());
    }

    // ==================== Error display tests ====================

    #[test]
    fn test_error_display_invalid_contract_address() {
        let err = FeeForwarderError::InvalidContractAddress("test".to_string());
        assert!(err.to_string().contains("Invalid contract address"));
    }

    #[test]
    fn test_error_display_invalid_account_address() {
        let err = FeeForwarderError::InvalidAccountAddress("test".to_string());
        assert!(err.to_string().contains("Invalid account address"));
    }

    #[test]
    fn test_error_display_authorization_build_error() {
        let err = FeeForwarderError::AuthorizationBuildError("test".to_string());
        assert!(err.to_string().contains("Failed to build authorization"));
    }

    #[test]
    fn test_error_display_provider_error() {
        let err = FeeForwarderError::ProviderError("test".to_string());
        assert!(err.to_string().contains("Provider error"));
    }

    #[test]
    fn test_error_display_xdr_error() {
        let err = FeeForwarderError::XdrError("test".to_string());
        assert!(err.to_string().contains("XDR serialization error"));
    }

    #[test]
    fn test_error_display_invalid_function_name() {
        let err = FeeForwarderError::InvalidFunctionName("test".to_string());
        assert!(err.to_string().contains("Invalid function name"));
    }

    // ==================== Constants tests ====================

    #[test]
    fn test_default_validity_seconds() {
        assert_eq!(DEFAULT_VALIDITY_SECONDS, 300);
    }

    #[test]
    fn test_ledger_time_seconds() {
        assert_eq!(LEDGER_TIME_SECONDS, 5);
    }
}
