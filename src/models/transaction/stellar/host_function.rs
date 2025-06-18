//! Host function types and conversions for Stellar transactions

use crate::models::SignerError;
use serde::{Deserialize, Serialize};
use soroban_rs::xdr::{
    AccountId, ContractExecutable, ContractIdPreimage, ContractIdPreimageFromAddress,
    CreateContractArgs, CreateContractArgsV2, Hash, HostFunction, InvokeContractArgs,
    PublicKey as XdrPublicKey, ScAddress, ScSymbol, ScVal, Uint256, VecM,
};
use std::convert::TryFrom;
use utoipa::ToSchema;

/// Represents different ways to provide WASM code
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum WasmSource {
    Hex { hex: String },
    Base64 { base64: String },
}

/// Represents the source for contract creation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum ContractSource {
    Address { address: String }, // Account address that will own the contract
    Contract { contract: String }, // Existing contract address
}

/// Represents different host function specifications
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostFunctionSpec {
    // Contract invocation
    InvokeContract {
        contract_address: String,
        function_name: String,
        args: Vec<serde_json::Value>,
    },

    // WASM upload
    UploadWasm {
        wasm: WasmSource,
    },

    // Contract creation with explicit fields
    CreateContract {
        source: ContractSource,
        wasm_hash: String, // hex-encoded
        #[serde(skip_serializing_if = "Option::is_none")]
        salt: Option<String>, // hex-encoded, defaults to zeros
        #[serde(skip_serializing_if = "Option::is_none")]
        constructor_args: Option<Vec<serde_json::Value>>,
    },
}

// Helper functions for HostFunctionSpec conversion

/// Converts a WasmSource to bytes
fn wasm_source_to_bytes(wasm: WasmSource) -> Result<Vec<u8>, SignerError> {
    match wasm {
        WasmSource::Hex { hex } => hex::decode(&hex)
            .map_err(|e| SignerError::ConversionError(format!("Invalid hex in wasm: {}", e))),
        WasmSource::Base64 { base64 } => {
            use base64::{engine::general_purpose, Engine as _};
            general_purpose::STANDARD
                .decode(&base64)
                .map_err(|e| SignerError::ConversionError(format!("Invalid base64 in wasm: {}", e)))
        }
    }
}

/// Parses and validates salt bytes from optional hex string
fn parse_salt_bytes(salt: Option<String>) -> Result<[u8; 32], SignerError> {
    if let Some(salt_hex) = salt {
        let bytes = hex::decode(&salt_hex)
            .map_err(|e| SignerError::ConversionError(format!("Invalid salt hex: {}", e)))?;
        if bytes.len() != 32 {
            return Err(SignerError::ConversionError("Salt must be 32 bytes".into()));
        }
        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes);
        Ok(array)
    } else {
        Ok([0u8; 32]) // Default to zeros
    }
}

/// Converts hex string to 32-byte hash
fn parse_wasm_hash(wasm_hash: &str) -> Result<Hash, SignerError> {
    let hash_bytes = hex::decode(wasm_hash)
        .map_err(|e| SignerError::ConversionError(format!("Invalid hex in wasm_hash: {}", e)))?;
    if hash_bytes.len() != 32 {
        return Err(SignerError::ConversionError(format!(
            "Hash must be 32 bytes, got {}",
            hash_bytes.len()
        )));
    }
    let mut hash_array = [0u8; 32];
    hash_array.copy_from_slice(&hash_bytes);
    Ok(Hash(hash_array))
}

/// Builds contract ID preimage from contract source
fn build_contract_preimage(
    source: ContractSource,
    salt: Option<String>,
) -> Result<ContractIdPreimage, SignerError> {
    let salt_bytes = parse_salt_bytes(salt)?;

    match source {
        ContractSource::Address { address } => {
            let public_key =
                stellar_strkey::ed25519::PublicKey::from_string(&address).map_err(|e| {
                    SignerError::ConversionError(format!("Invalid account address: {}", e))
                })?;
            let account_id = AccountId(XdrPublicKey::PublicKeyTypeEd25519(Uint256(public_key.0)));

            Ok(ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                address: ScAddress::Account(account_id),
                salt: Uint256(salt_bytes),
            }))
        }
        ContractSource::Contract { contract } => {
            let contract_id = stellar_strkey::Contract::from_string(&contract).map_err(|e| {
                SignerError::ConversionError(format!("Invalid contract address: {}", e))
            })?;

            Ok(ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                address: ScAddress::Contract(Hash(contract_id.0)),
                salt: Uint256(salt_bytes),
            }))
        }
    }
}

/// Converts InvokeContract spec to HostFunction
fn convert_invoke_contract(
    contract_address: String,
    function_name: String,
    args: Vec<serde_json::Value>,
) -> Result<HostFunction, SignerError> {
    // Parse contract address
    let contract = stellar_strkey::Contract::from_string(&contract_address)
        .map_err(|e| SignerError::ConversionError(format!("Invalid contract address: {}", e)))?;
    let contract_addr = ScAddress::Contract(Hash(contract.0));

    // Convert function name to symbol
    let function_symbol = ScSymbol::try_from(function_name.as_bytes().to_vec())
        .map_err(|e| SignerError::ConversionError(format!("Invalid function name: {}", e)))?;

    // Convert JSON args to ScVals using serde
    let scval_args: Vec<ScVal> = args
        .iter()
        .map(|json| serde_json::from_value(json.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| SignerError::ConversionError(format!("Failed to deserialize ScVal: {}", e)))?;
    let args_vec = VecM::try_from(scval_args)
        .map_err(|e| SignerError::ConversionError(format!("Failed to convert arguments: {}", e)))?;

    Ok(HostFunction::InvokeContract(InvokeContractArgs {
        contract_address: contract_addr,
        function_name: function_symbol,
        args: args_vec,
    }))
}

/// Converts UploadWasm spec to HostFunction
fn convert_upload_wasm(wasm: WasmSource) -> Result<HostFunction, SignerError> {
    let bytes = wasm_source_to_bytes(wasm)?;
    Ok(HostFunction::UploadContractWasm(bytes.try_into().map_err(
        |e| SignerError::ConversionError(format!("Failed to convert wasm bytes: {:?}", e)),
    )?))
}

/// Converts CreateContract spec to HostFunction
fn convert_create_contract(
    source: ContractSource,
    wasm_hash: String,
    salt: Option<String>,
    constructor_args: Option<Vec<serde_json::Value>>,
) -> Result<HostFunction, SignerError> {
    let preimage = build_contract_preimage(source, salt)?;
    let wasm_hash = parse_wasm_hash(&wasm_hash)?;

    // Handle constructor args if provided
    if let Some(args) = constructor_args {
        if !args.is_empty() {
            // Convert JSON args to ScVals using serde
            let scval_args: Vec<ScVal> = args
                .iter()
                .map(|json| serde_json::from_value(json.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    SignerError::ConversionError(format!("Failed to deserialize ScVal: {}", e))
                })?;
            let constructor_args_vec = VecM::try_from(scval_args).map_err(|e| {
                SignerError::ConversionError(format!(
                    "Failed to convert constructor arguments: {}",
                    e
                ))
            })?;

            let create_args_v2 = CreateContractArgsV2 {
                contract_id_preimage: preimage,
                executable: ContractExecutable::Wasm(wasm_hash),
                constructor_args: constructor_args_vec,
            };

            return Ok(HostFunction::CreateContractV2(create_args_v2));
        }
    }

    // No constructor args, use v1
    let create_args = CreateContractArgs {
        contract_id_preimage: preimage,
        executable: ContractExecutable::Wasm(wasm_hash),
    };

    Ok(HostFunction::CreateContract(create_args))
}

impl TryFrom<HostFunctionSpec> for HostFunction {
    type Error = SignerError;

    fn try_from(spec: HostFunctionSpec) -> Result<Self, Self::Error> {
        match spec {
            HostFunctionSpec::InvokeContract {
                contract_address,
                function_name,
                args,
            } => convert_invoke_contract(contract_address, function_name, args),

            HostFunctionSpec::UploadWasm { wasm } => convert_upload_wasm(wasm),

            HostFunctionSpec::CreateContract {
                source,
                wasm_hash,
                salt,
                constructor_args,
            } => convert_create_contract(source, wasm_hash, salt, constructor_args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const TEST_CONTRACT: &str = "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA";

    mod wasm_source_to_bytes_tests {
        use super::*;

        #[test]
        fn test_hex_conversion() {
            let wasm = WasmSource::Hex {
                hex: "deadbeef".to_string(),
            };
            let result = wasm_source_to_bytes(wasm).unwrap();
            assert_eq!(result, vec![0xde, 0xad, 0xbe, 0xef]);
        }

        #[test]
        fn test_base64_conversion() {
            let wasm = WasmSource::Base64 {
                base64: "3q2+7w==".to_string(), // base64 for "deadbeef"
            };
            let result = wasm_source_to_bytes(wasm).unwrap();
            assert_eq!(result, vec![0xde, 0xad, 0xbe, 0xef]);
        }

        #[test]
        fn test_invalid_hex() {
            let wasm = WasmSource::Hex {
                hex: "invalid_hex".to_string(),
            };
            let result = wasm_source_to_bytes(wasm);
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Invalid hex"));
        }

        #[test]
        fn test_invalid_base64() {
            let wasm = WasmSource::Base64 {
                base64: "!!!invalid!!!".to_string(),
            };
            let result = wasm_source_to_bytes(wasm);
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Invalid base64"));
        }
    }

    mod parse_salt_bytes_tests {
        use super::*;

        #[test]
        fn test_valid_32_byte_hex() {
            let salt = Some(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            );
            let result = parse_salt_bytes(salt).unwrap();
            assert_eq!(result.len(), 32);
            assert_eq!(result[0], 0x01);
            assert_eq!(result[1], 0x23);
        }

        #[test]
        fn test_none_returns_zeros() {
            let result = parse_salt_bytes(None).unwrap();
            assert_eq!(result, [0u8; 32]);
        }

        #[test]
        fn test_invalid_hex() {
            let salt = Some("gg".to_string()); // Invalid hex
            let result = parse_salt_bytes(salt);
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Invalid salt hex"));
        }

        #[test]
        fn test_wrong_length() {
            let salt = Some("abcd".to_string()); // Too short
            let result = parse_salt_bytes(salt);
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Salt must be 32 bytes"));
        }
    }

    mod parse_wasm_hash_tests {
        use super::*;

        #[test]
        fn test_valid_32_byte_hex() {
            let hash_hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
            let result = parse_wasm_hash(hash_hex).unwrap();
            assert_eq!(result.0[0], 0x01);
            assert_eq!(result.0[31], 0xef);
        }

        #[test]
        fn test_invalid_hex() {
            let result = parse_wasm_hash("invalid_hex");
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("Invalid hex"));
        }

        #[test]
        fn test_wrong_length() {
            let result = parse_wasm_hash("abcd");
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Hash must be 32 bytes"));
        }
    }

    mod build_contract_preimage_tests {
        use super::*;

        #[test]
        fn test_with_address_source() {
            let source = ContractSource::Address {
                address: TEST_PK.to_string(),
            };
            let result = build_contract_preimage(source, None).unwrap();

            if let ContractIdPreimage::Address(preimage) = result {
                assert!(matches!(preimage.address, ScAddress::Account(_)));
                assert_eq!(preimage.salt.0, [0u8; 32]);
            } else {
                panic!("Expected Address preimage");
            }
        }

        #[test]
        fn test_with_contract_source() {
            let source = ContractSource::Contract {
                contract: TEST_CONTRACT.to_string(),
            };
            let result = build_contract_preimage(source, None).unwrap();

            if let ContractIdPreimage::Address(preimage) = result {
                assert!(matches!(preimage.address, ScAddress::Contract(_)));
                assert_eq!(preimage.salt.0, [0u8; 32]);
            } else {
                panic!("Expected Address preimage");
            }
        }

        #[test]
        fn test_with_custom_salt() {
            let source = ContractSource::Address {
                address: TEST_PK.to_string(),
            };
            let salt = Some(
                "0000000000000000000000000000000000000000000000000000000000000042".to_string(),
            );
            let result = build_contract_preimage(source, salt).unwrap();

            if let ContractIdPreimage::Address(preimage) = result {
                assert_eq!(preimage.salt.0[31], 0x42);
            } else {
                panic!("Expected Address preimage");
            }
        }

        #[test]
        fn test_invalid_address() {
            let source = ContractSource::Address {
                address: "INVALID".to_string(),
            };
            let result = build_contract_preimage(source, None);
            assert!(result.is_err());
        }
    }

    mod convert_invoke_contract_tests {
        use super::*;

        #[test]
        fn test_valid_contract_address() {
            let result =
                convert_invoke_contract(TEST_CONTRACT.to_string(), "hello".to_string(), vec![]);
            assert!(result.is_ok());

            if let HostFunction::InvokeContract(args) = result.unwrap() {
                assert!(matches!(args.contract_address, ScAddress::Contract(_)));
                assert_eq!(args.function_name.to_utf8_string_lossy(), "hello");
                assert_eq!(args.args.len(), 0);
            } else {
                panic!("Expected InvokeContract");
            }
        }

        #[test]
        fn test_function_name_conversion() {
            let result = convert_invoke_contract(
                TEST_CONTRACT.to_string(),
                "transfer_tokens".to_string(),
                vec![],
            );
            assert!(result.is_ok());

            if let HostFunction::InvokeContract(args) = result.unwrap() {
                assert_eq!(args.function_name.to_utf8_string_lossy(), "transfer_tokens");
            } else {
                panic!("Expected InvokeContract");
            }
        }

        #[test]
        fn test_various_arg_types() {
            let args = vec![
                json!({"u64": 1000}),
                json!({"string": "hello"}),
                json!({"address": TEST_PK}),
            ];
            let result =
                convert_invoke_contract(TEST_CONTRACT.to_string(), "test".to_string(), args);
            assert!(result.is_ok());

            if let HostFunction::InvokeContract(invoke_args) = result.unwrap() {
                assert_eq!(invoke_args.args.len(), 3);
            } else {
                panic!("Expected InvokeContract");
            }
        }

        #[test]
        fn test_invalid_contract_address() {
            let result =
                convert_invoke_contract("INVALID".to_string(), "hello".to_string(), vec![]);
            assert!(result.is_err());
        }
    }

    mod convert_upload_wasm_tests {
        use super::*;

        #[test]
        fn test_hex_source() {
            let wasm = WasmSource::Hex {
                hex: "deadbeef".to_string(),
            };
            let result = convert_upload_wasm(wasm);
            assert!(result.is_ok());

            if let HostFunction::UploadContractWasm(bytes) = result.unwrap() {
                assert_eq!(bytes.to_vec(), vec![0xde, 0xad, 0xbe, 0xef]);
            } else {
                panic!("Expected UploadContractWasm");
            }
        }

        #[test]
        fn test_base64_source() {
            let wasm = WasmSource::Base64 {
                base64: "3q2+7w==".to_string(),
            };
            let result = convert_upload_wasm(wasm);
            assert!(result.is_ok());
        }

        #[test]
        fn test_invalid_wasm() {
            let wasm = WasmSource::Hex {
                hex: "invalid".to_string(),
            };
            let result = convert_upload_wasm(wasm);
            assert!(result.is_err());
        }
    }

    mod convert_create_contract_tests {
        use super::*;

        #[test]
        fn test_v1_no_constructor_args() {
            let source = ContractSource::Address {
                address: TEST_PK.to_string(),
            };
            let wasm_hash =
                "0000000000000000000000000000000000000000000000000000000000000001".to_string();
            let result = convert_create_contract(source, wasm_hash, None, None);

            assert!(result.is_ok());
            assert!(matches!(result.unwrap(), HostFunction::CreateContract(_)));
        }

        #[test]
        fn test_v2_with_constructor_args() {
            let source = ContractSource::Address {
                address: TEST_PK.to_string(),
            };
            let wasm_hash =
                "0000000000000000000000000000000000000000000000000000000000000001".to_string();
            let args = Some(vec![json!({"string": "hello"}), json!({"u64": 42})]);
            let result = convert_create_contract(source, wasm_hash, None, args);

            assert!(result.is_ok());
            if let HostFunction::CreateContractV2(args) = result.unwrap() {
                assert_eq!(args.constructor_args.len(), 2);
            } else {
                panic!("Expected CreateContractV2");
            }
        }

        #[test]
        fn test_empty_constructor_args_uses_v1() {
            let source = ContractSource::Address {
                address: TEST_PK.to_string(),
            };
            let wasm_hash =
                "0000000000000000000000000000000000000000000000000000000000000001".to_string();
            let args = Some(vec![]);
            let result = convert_create_contract(source, wasm_hash, None, args);

            assert!(result.is_ok());
            assert!(matches!(result.unwrap(), HostFunction::CreateContract(_)));
        }

        #[test]
        fn test_salt_handling() {
            let source = ContractSource::Address {
                address: TEST_PK.to_string(),
            };
            let wasm_hash =
                "0000000000000000000000000000000000000000000000000000000000000001".to_string();
            let salt = Some(
                "0000000000000000000000000000000000000000000000000000000000000042".to_string(),
            );
            let result = convert_create_contract(source, wasm_hash, salt, None);

            assert!(result.is_ok());
        }
    }

    // Integration tests
    #[test]
    fn test_invoke_contract() {
        let spec = HostFunctionSpec::InvokeContract {
            contract_address: TEST_CONTRACT.to_string(),
            function_name: "hello".to_string(),
            args: vec![json!({"string": "world"})],
        };

        let result = HostFunction::try_from(spec);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), HostFunction::InvokeContract(_)));
    }

    #[test]
    fn test_upload_wasm() {
        let spec = HostFunctionSpec::UploadWasm {
            wasm: WasmSource::Hex {
                hex: "deadbeef".to_string(),
            },
        };

        let result = HostFunction::try_from(spec);
        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            HostFunction::UploadContractWasm(_)
        ));
    }

    #[test]
    fn test_create_contract_v1() {
        let spec = HostFunctionSpec::CreateContract {
            source: ContractSource::Address {
                address: TEST_PK.to_string(),
            },
            wasm_hash: "0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            salt: None,
            constructor_args: None,
        };

        let result = HostFunction::try_from(spec);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), HostFunction::CreateContract(_)));
    }

    #[test]
    fn test_create_contract_v2() {
        let spec = HostFunctionSpec::CreateContract {
            source: ContractSource::Address {
                address: TEST_PK.to_string(),
            },
            wasm_hash: "0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            salt: None,
            constructor_args: Some(vec![json!({"string": "init"})]),
        };

        let result = HostFunction::try_from(spec);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), HostFunction::CreateContractV2(_)));
    }

    #[test]
    fn test_host_function_spec_serde() {
        let spec = HostFunctionSpec::InvokeContract {
            contract_address: TEST_CONTRACT.to_string(),
            function_name: "test".to_string(),
            args: vec![json!({"u64": 42})],
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("invoke_contract"));
        assert!(json.contains(TEST_CONTRACT));

        let deserialized: HostFunctionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deserialized);
    }
}
