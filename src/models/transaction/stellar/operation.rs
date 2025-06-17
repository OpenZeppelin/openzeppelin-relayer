//! Operation types and conversions for Stellar transactions

use crate::models::transaction::stellar::asset::AssetSpec;
use crate::models::transaction::stellar::host_function::HostFunctionSpec;
use crate::models::SignerError;
use serde::{Deserialize, Serialize};
use soroban_rs::xdr::{
    HostFunction, InvokeHostFunctionOp, MuxedAccount as XdrMuxedAccount, MuxedAccountMed25519,
    Operation, OperationBody, PaymentOp, SorobanAuthorizationEntry, SorobanAuthorizedFunction,
    SorobanAuthorizedInvocation, SorobanCredentials, Uint256, VecM,
};
use std::convert::TryFrom;
use stellar_strkey::ed25519::{MuxedAccount, PublicKey};
use utoipa::ToSchema;

/// Simple auth credential types that can be auto-generated
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SimpleAuthCredential {
    /// Use the transaction source account for authorization
    SourceAccount,
    // Future additions:
    // Address { address: String },
}

/// Authorization specification for Soroban operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AuthSpec {
    /// Simple format - auto-generate auth entries based on credential types
    Simple(Vec<SimpleAuthCredential>),

    /// Advanced format - provide complete XDR auth entries as base64-encoded strings
    Xdr(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OperationSpec {
    Payment {
        destination: String,
        amount: i64,
        asset: AssetSpec,
    },
    InvokeHostFunction {
        #[serde(flatten)]
        host_function_spec: HostFunctionSpec,
        #[serde(skip_serializing_if = "Option::is_none")]
        auth: Option<AuthSpec>,
    },
}

// Helper functions for OperationSpec conversion

/// Parses a destination address into an XDR MuxedAccount
fn parse_destination_address(destination: &str) -> Result<XdrMuxedAccount, SignerError> {
    if let Ok(m) = MuxedAccount::from_string(destination) {
        // accept M... muxed accounts
        Ok(XdrMuxedAccount::MuxedEd25519(MuxedAccountMed25519 {
            id: m.id,
            ed25519: Uint256(m.ed25519),
        }))
    } else {
        // fall-back to plain G... public key
        let pk = PublicKey::from_string(destination)
            .map_err(|e| SignerError::ConversionError(format!("Invalid destination: {}", e)))?;
        Ok(XdrMuxedAccount::Ed25519(Uint256(pk.0)))
    }
}

/// Creates a Soroban authorization entry for source account
fn create_source_account_auth_entry(
    function: SorobanAuthorizedFunction,
) -> SorobanAuthorizationEntry {
    SorobanAuthorizationEntry {
        credentials: SorobanCredentials::SourceAccount,
        root_invocation: SorobanAuthorizedInvocation {
            function,
            sub_invocations: VecM::default(),
        },
    }
}

/// Processes simple auth credentials into authorization entries
fn process_simple_auth_credentials(
    credentials: Vec<SimpleAuthCredential>,
    host_function: &HostFunction,
) -> Result<Vec<SorobanAuthorizationEntry>, SignerError> {
    let mut auth_entries = Vec::new();

    for credential in credentials {
        match credential {
            SimpleAuthCredential::SourceAccount => {
                match host_function {
                    HostFunction::CreateContract(ref create_args) => {
                        let auth_entry = create_source_account_auth_entry(
                            SorobanAuthorizedFunction::CreateContractHostFn(create_args.clone()),
                        );
                        auth_entries.push(auth_entry);
                    }
                    HostFunction::CreateContractV2(ref create_args_v2) => {
                        let auth_entry = create_source_account_auth_entry(
                            SorobanAuthorizedFunction::CreateContractV2HostFn(
                                create_args_v2.clone(),
                            ),
                        );
                        auth_entries.push(auth_entry);
                    }
                    HostFunction::InvokeContract(ref invoke_args) => {
                        let auth_entry = create_source_account_auth_entry(
                            SorobanAuthorizedFunction::ContractFn(
                                soroban_rs::xdr::InvokeContractArgs {
                                    contract_address: invoke_args.contract_address.clone(),
                                    function_name: invoke_args.function_name.clone(),
                                    args: invoke_args.args.clone(),
                                },
                            ),
                        );
                        auth_entries.push(auth_entry);
                    }
                    _ => {
                        // Other operations don't typically need auth
                    }
                }
            }
        }
    }

    Ok(auth_entries)
}

/// Decodes XDR authorization entries from base64 strings
fn decode_xdr_auth_entries(
    xdr_entries: Vec<String>,
) -> Result<Vec<SorobanAuthorizationEntry>, SignerError> {
    use soroban_rs::xdr::{Limits, ReadXdr};

    xdr_entries
        .iter()
        .map(|xdr_str| {
            SorobanAuthorizationEntry::from_xdr_base64(xdr_str, Limits::none())
                .map_err(|e| SignerError::ConversionError(format!("Invalid auth XDR: {}", e)))
        })
        .collect()
}

/// Generates default authorization entries for host functions that require them
fn generate_default_auth_entries(
    host_function: &HostFunction,
) -> Result<Vec<SorobanAuthorizationEntry>, SignerError> {
    match host_function {
        HostFunction::CreateContract(ref create_args) => {
            let auth_entry = create_source_account_auth_entry(
                SorobanAuthorizedFunction::CreateContractHostFn(create_args.clone()),
            );
            Ok(vec![auth_entry])
        }
        HostFunction::CreateContractV2(ref create_args_v2) => {
            let auth_entry = create_source_account_auth_entry(
                SorobanAuthorizedFunction::CreateContractV2HostFn(create_args_v2.clone()),
            );
            Ok(vec![auth_entry])
        }
        HostFunction::InvokeContract(ref invoke_args) => {
            let auth_entry = create_source_account_auth_entry(
                SorobanAuthorizedFunction::ContractFn(invoke_args.clone()),
            );
            Ok(vec![auth_entry])
        }
        _ => Ok(vec![]),
    }
}

/// Converts authorization spec and host function into authorization vector
fn build_auth_vector(
    auth: Option<AuthSpec>,
    host_function: &HostFunction,
) -> Result<VecM<SorobanAuthorizationEntry, { u32::MAX }>, SignerError> {
    let auth_entries = match auth {
        Some(AuthSpec::Simple(credentials)) => {
            process_simple_auth_credentials(credentials, host_function)?
        }
        Some(AuthSpec::Xdr(xdr_entries)) => decode_xdr_auth_entries(xdr_entries)?,
        None => generate_default_auth_entries(host_function)?,
    };

    auth_entries.try_into().map_err(|e| {
        SignerError::ConversionError(format!("Failed to convert auth entries: {:?}", e))
    })
}

/// Converts Payment operation spec to Operation
fn convert_payment_operation(
    destination: String,
    amount: i64,
    asset: AssetSpec,
) -> Result<Operation, SignerError> {
    let dest = parse_destination_address(&destination)?;

    Ok(Operation {
        source_account: None,
        body: OperationBody::Payment(PaymentOp {
            destination: dest,
            asset: asset.try_into()?,
            amount,
        }),
    })
}

/// Converts InvokeHostFunction operation spec to Operation
fn convert_invoke_host_function_operation(
    host_function_spec: HostFunctionSpec,
    auth: Option<AuthSpec>,
) -> Result<Operation, SignerError> {
    // Convert HostFunctionSpec to HostFunction using the dedicated impl
    let host_function = HostFunction::try_from(host_function_spec)?;

    // Build authorization vector
    let auth_vec = build_auth_vector(auth, &host_function)?;

    Ok(Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
            auth: auth_vec,
            host_function,
        }),
    })
}

impl TryFrom<OperationSpec> for Operation {
    type Error = SignerError;

    fn try_from(op: OperationSpec) -> Result<Self, Self::Error> {
        match op {
            OperationSpec::Payment {
                destination,
                amount,
                asset,
            } => convert_payment_operation(destination, amount, asset),

            OperationSpec::InvokeHostFunction {
                host_function_spec,
                auth,
            } => convert_invoke_host_function_operation(host_function_spec, auth),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::transaction::stellar::host_function::ContractSource;
    use soroban_rs::xdr::{
        AccountId, ContractExecutable, ContractIdPreimage, ContractIdPreimageFromAddress,
        CreateContractArgs, CreateContractArgsV2, Hash, PublicKey as XdrPublicKey, ScAddress,
    };

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const TEST_CONTRACT: &str = "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA";
    const TEST_MUXED: &str =
        "MAAAAAAAAAAAAAB7BQ2L7E5NBWMXDUCMZSIPOBKRDSBYVLMXGSSKF6YNPIB7Y77ITLVL6";

    mod parse_destination_address_tests {
        use super::*;

        #[test]
        fn test_regular_public_key() {
            let result = parse_destination_address(TEST_PK).unwrap();
            assert!(matches!(result, XdrMuxedAccount::Ed25519(_)));
        }

        #[test]
        fn test_muxed_account() {
            let result = parse_destination_address(TEST_MUXED).unwrap();
            assert!(matches!(result, XdrMuxedAccount::MuxedEd25519(_)));
        }

        #[test]
        fn test_invalid_address() {
            let result = parse_destination_address("INVALID");
            assert!(result.is_err());
        }
    }

    mod create_source_account_auth_entry_tests {
        use super::*;

        #[test]
        fn test_creates_correct_structure() {
            let function = SorobanAuthorizedFunction::CreateContractHostFn(CreateContractArgs {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
            });

            let entry = create_source_account_auth_entry(function.clone());
            assert!(matches!(
                entry.credentials,
                SorobanCredentials::SourceAccount
            ));
            // Can't directly compare functions due to Clone requirement, but structure is validated
        }
    }

    mod process_simple_auth_credentials_tests {
        use super::*;

        #[test]
        fn test_create_contract_auth() {
            let host_function = HostFunction::CreateContract(CreateContractArgs {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
            });

            let credentials = vec![SimpleAuthCredential::SourceAccount];
            let result = process_simple_auth_credentials(credentials, &host_function);

            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }

        #[test]
        fn test_create_contract_v2_auth() {
            let host_function = HostFunction::CreateContractV2(CreateContractArgsV2 {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
                constructor_args: VecM::default(),
            });

            let credentials = vec![SimpleAuthCredential::SourceAccount];
            let result = process_simple_auth_credentials(credentials, &host_function);

            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }

        #[test]
        fn test_invoke_contract_auth() {
            let host_function = HostFunction::InvokeContract(soroban_rs::xdr::InvokeContractArgs {
                contract_address: ScAddress::Contract(Hash([0u8; 32])),
                function_name: soroban_rs::xdr::ScSymbol::try_from(b"test".to_vec()).unwrap(),
                args: VecM::default(),
            });

            let credentials = vec![SimpleAuthCredential::SourceAccount];
            let result = process_simple_auth_credentials(credentials, &host_function);

            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }

        #[test]
        fn test_empty_credentials() {
            let host_function = HostFunction::CreateContract(CreateContractArgs {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
            });

            let credentials = vec![];
            let result = process_simple_auth_credentials(credentials, &host_function);

            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 0);
        }
    }

    mod decode_xdr_auth_entries_tests {
        use super::*;

        #[test]
        fn test_invalid_base64() {
            let xdr_entries = vec!["!!!invalid!!!".to_string()];
            let result = decode_xdr_auth_entries(xdr_entries);
            assert!(result.is_err());
        }

        #[test]
        fn test_malformed_xdr() {
            let xdr_entries = vec!["dGVzdA==".to_string()]; // Valid base64 but not valid XDR
            let result = decode_xdr_auth_entries(xdr_entries);
            assert!(result.is_err());
        }

        #[test]
        fn test_empty_list() {
            let xdr_entries = vec![];
            let result = decode_xdr_auth_entries(xdr_entries);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 0);
        }
    }

    mod generate_default_auth_entries_tests {
        use super::*;

        #[test]
        fn test_create_contract() {
            let host_function = HostFunction::CreateContract(CreateContractArgs {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
            });

            let result = generate_default_auth_entries(&host_function);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }

        #[test]
        fn test_create_contract_v2() {
            let host_function = HostFunction::CreateContractV2(CreateContractArgsV2 {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
                constructor_args: VecM::default(),
            });

            let result = generate_default_auth_entries(&host_function);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }

        #[test]
        fn test_invoke_contract() {
            let host_function = HostFunction::InvokeContract(soroban_rs::xdr::InvokeContractArgs {
                contract_address: ScAddress::Contract(Hash([0u8; 32])),
                function_name: soroban_rs::xdr::ScSymbol::try_from(b"test".to_vec()).unwrap(),
                args: VecM::default(),
            });

            let result = generate_default_auth_entries(&host_function);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }

        #[test]
        fn test_other_operations() {
            let host_function = HostFunction::UploadContractWasm(vec![0u8; 10].try_into().unwrap());

            let result = generate_default_auth_entries(&host_function);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 0);
        }
    }

    mod build_auth_vector_tests {
        use super::*;

        #[test]
        fn test_simple_auth() {
            let host_function = HostFunction::CreateContract(CreateContractArgs {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
            });

            let auth = Some(AuthSpec::Simple(vec![SimpleAuthCredential::SourceAccount]));
            let result = build_auth_vector(auth, &host_function);

            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }

        #[test]
        fn test_xdr_auth_invalid() {
            let host_function = HostFunction::CreateContract(CreateContractArgs {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
            });

            let auth = Some(AuthSpec::Xdr(vec!["invalid".to_string()]));
            let result = build_auth_vector(auth, &host_function);

            assert!(result.is_err());
        }

        #[test]
        fn test_none_default_create_contract() {
            let host_function = HostFunction::CreateContract(CreateContractArgs {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Account(AccountId(XdrPublicKey::PublicKeyTypeEd25519(
                        Uint256([0u8; 32]),
                    ))),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::Wasm(Hash([0u8; 32])),
            });

            let result = build_auth_vector(None, &host_function);

            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }

        #[test]
        fn test_none_default_invoke_contract() {
            let host_function = HostFunction::InvokeContract(soroban_rs::xdr::InvokeContractArgs {
                contract_address: ScAddress::Contract(Hash([0u8; 32])),
                function_name: soroban_rs::xdr::ScSymbol::try_from(b"test".to_vec()).unwrap(),
                args: VecM::default(),
            });

            let result = build_auth_vector(None, &host_function);

            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 1);
        }
    }

    mod convert_payment_operation_tests {
        use super::*;

        #[test]
        fn test_with_native_asset() {
            let result = convert_payment_operation(TEST_PK.to_string(), 1000, AssetSpec::Native);

            assert!(result.is_ok());
            if let Operation {
                body: OperationBody::Payment(op),
                ..
            } = result.unwrap()
            {
                assert_eq!(op.amount, 1000);
                assert!(matches!(op.asset, soroban_rs::xdr::Asset::Native));
            } else {
                panic!("Expected Payment operation");
            }
        }

        #[test]
        fn test_with_credit_asset() {
            let result = convert_payment_operation(
                TEST_PK.to_string(),
                500,
                AssetSpec::Credit4 {
                    code: "USDC".to_string(),
                    issuer: TEST_PK.to_string(),
                },
            );

            assert!(result.is_ok());
        }

        #[test]
        fn test_invalid_destination() {
            let result = convert_payment_operation("INVALID".to_string(), 1000, AssetSpec::Native);

            assert!(result.is_err());
        }

        #[test]
        fn test_invalid_asset() {
            let result = convert_payment_operation(
                TEST_PK.to_string(),
                1000,
                AssetSpec::Credit4 {
                    code: "TOOLONG".to_string(),
                    issuer: TEST_PK.to_string(),
                },
            );

            assert!(result.is_err());
        }
    }

    mod convert_invoke_host_function_operation_tests {
        use super::*;

        #[test]
        fn test_various_host_functions() {
            let spec = HostFunctionSpec::InvokeContract {
                contract_address: TEST_CONTRACT.to_string(),
                function_name: "test".to_string(),
                args: vec![],
            };

            let result = convert_invoke_host_function_operation(spec, None);
            assert!(result.is_ok());
        }

        #[test]
        fn test_auth_handling() {
            let spec = HostFunctionSpec::CreateContract {
                source: ContractSource::Address {
                    address: TEST_PK.to_string(),
                },
                wasm_hash: "0000000000000000000000000000000000000000000000000000000000000001"
                    .to_string(),
                salt: None,
                constructor_args: None,
            };

            let auth = Some(AuthSpec::Simple(vec![SimpleAuthCredential::SourceAccount]));
            let result = convert_invoke_host_function_operation(spec, auth);

            assert!(result.is_ok());
            if let Operation {
                body: OperationBody::InvokeHostFunction(op),
                ..
            } = result.unwrap()
            {
                assert_eq!(op.auth.len(), 1);
            } else {
                panic!("Expected InvokeHostFunction operation");
            }
        }
    }

    // Integration tests
    #[test]
    fn test_payment_operation() {
        let spec = OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000,
            asset: AssetSpec::Native,
        };

        let result = Operation::try_from(spec);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap().body, OperationBody::Payment(_)));
    }

    #[test]
    fn test_invoke_host_function_operation() {
        let spec = OperationSpec::InvokeHostFunction {
            host_function_spec: HostFunctionSpec::InvokeContract {
                contract_address: TEST_CONTRACT.to_string(),
                function_name: "test".to_string(),
                args: vec![],
            },
            auth: None,
        };

        let result = Operation::try_from(spec);
        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap().body,
            OperationBody::InvokeHostFunction(_)
        ));
    }

    #[test]
    fn test_operation_spec_serde() {
        let spec = OperationSpec::Payment {
            destination: TEST_PK.to_string(),
            amount: 1000,
            asset: AssetSpec::Native,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("payment"));
        assert!(json.contains("NATIVE"));

        let deserialized: OperationSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn test_auth_spec_serde() {
        let spec = AuthSpec::Simple(vec![SimpleAuthCredential::SourceAccount]);
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("simple"));
        assert!(json.contains("source_account"));

        let deserialized: AuthSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn test_invoke_contract_with_source_account_auth_integration() {
        // Create a realistic InvokeContract operation with source account auth
        let spec = OperationSpec::InvokeHostFunction {
            host_function_spec: HostFunctionSpec::InvokeContract {
                contract_address: TEST_CONTRACT.to_string(),
                function_name: "transfer".to_string(),
                args: vec![], // In real scenario, this would contain transfer args
            },
            auth: Some(AuthSpec::Simple(vec![SimpleAuthCredential::SourceAccount])),
        };

        // Convert to XDR Operation
        let result = Operation::try_from(spec);
        assert!(result.is_ok());

        let operation = result.unwrap();
        match operation.body {
            OperationBody::InvokeHostFunction(ref invoke_op) => {
                // Verify auth entries were created
                assert_eq!(invoke_op.auth.len(), 1);

                // Verify it's a source account credential
                let auth_entry = &invoke_op.auth[0];
                assert!(matches!(
                    auth_entry.credentials,
                    SorobanCredentials::SourceAccount
                ));

                // Verify the authorized function matches our contract invocation
                match &auth_entry.root_invocation.function {
                    SorobanAuthorizedFunction::ContractFn(invoke_args) => {
                        // The contract address and function name should match
                        assert!(matches!(
                            invoke_args.contract_address,
                            ScAddress::Contract(_)
                        ));
                        assert_eq!(invoke_args.function_name.0.as_slice(), b"transfer");
                    }
                    _ => panic!("Expected ContractFn authorization"),
                }
            }
            _ => panic!("Expected InvokeHostFunction operation"),
        }
    }

    #[test]
    fn test_invoke_contract_with_none_auth_gets_default() {
        // Create InvokeContract operation with NO auth specified
        let spec = OperationSpec::InvokeHostFunction {
            host_function_spec: HostFunctionSpec::InvokeContract {
                contract_address: TEST_CONTRACT.to_string(),
                function_name: "mint".to_string(),
                args: vec![],
            },
            auth: None, // No auth specified - should get default source account
        };

        // Convert to XDR Operation
        let result = Operation::try_from(spec);
        assert!(result.is_ok());

        let operation = result.unwrap();
        match operation.body {
            OperationBody::InvokeHostFunction(ref invoke_op) => {
                // Verify default auth entry was created
                assert_eq!(invoke_op.auth.len(), 1);

                // Verify it's a source account credential
                let auth_entry = &invoke_op.auth[0];
                assert!(matches!(
                    auth_entry.credentials,
                    SorobanCredentials::SourceAccount
                ));

                // Verify the authorized function matches our contract invocation
                match &auth_entry.root_invocation.function {
                    SorobanAuthorizedFunction::ContractFn(invoke_args) => {
                        assert_eq!(invoke_args.function_name.0.as_slice(), b"mint");
                    }
                    _ => panic!("Expected ContractFn authorization"),
                }
            }
            _ => panic!("Expected InvokeHostFunction operation"),
        }
    }
}
