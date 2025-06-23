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

/// HACK: Temporary fix for stellar-xdr bug where u64/i64 values are expected as numbers
/// but are provided as strings. This recursively converts string values to numbers for:
/// - {"u64":"1000"} to {"u64":1000}
/// - {"i64":"-1000"} to {"i64":-1000}
/// - {"timepoint":"1000"} to {"timepoint":1000}
/// - {"duration":"1000"} to {"duration":1000}
/// - UInt128Parts: {"hi":"1", "lo":"2"} to {"hi":1, "lo":2}
/// - Int128Parts: {"hi":"-1", "lo":"2"} to {"hi":-1, "lo":2}
/// - UInt256Parts: {"hi_hi":"1", "hi_lo":"2", "lo_hi":"3", "lo_lo":"4"} to numbers
/// - Int256Parts: {"hi_hi":"-1", "hi_lo":"2", "lo_hi":"3", "lo_lo":"4"} to numbers
///
/// TODO: Remove this once stellar-xdr properly handles u64/i64 as strings.
/// Track the issue at: https://github.com/stellar/rs-stellar-xdr
fn fix_u64_format(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            // Handle single-field u64/i64 objects
            if map.len() == 1 {
                if let Some(serde_json::Value::String(s)) = map.get("u64") {
                    if let Ok(num) = s.parse::<u64>() {
                        map.insert("u64".to_string(), serde_json::json!(num));
                    }
                } else if let Some(serde_json::Value::String(s)) = map.get("i64") {
                    if let Ok(num) = s.parse::<i64>() {
                        map.insert("i64".to_string(), serde_json::json!(num));
                    }
                } else if let Some(serde_json::Value::String(s)) = map.get("timepoint") {
                    if let Ok(num) = s.parse::<u64>() {
                        map.insert("timepoint".to_string(), serde_json::json!(num));
                    }
                } else if let Some(serde_json::Value::String(s)) = map.get("duration") {
                    if let Ok(num) = s.parse::<u64>() {
                        map.insert("duration".to_string(), serde_json::json!(num));
                    }
                }
            }

            // Handle UInt128Parts (hi: u64, lo: u64)
            if map.contains_key("hi") && map.contains_key("lo") && map.len() == 2 {
                if let Some(serde_json::Value::String(s)) = map.get("hi") {
                    if let Ok(num) = s.parse::<u64>() {
                        map.insert("hi".to_string(), serde_json::json!(num));
                    }
                }
                if let Some(serde_json::Value::String(s)) = map.get("lo") {
                    if let Ok(num) = s.parse::<u64>() {
                        map.insert("lo".to_string(), serde_json::json!(num));
                    }
                }
            }

            // Handle u128 wrapper object
            if map.contains_key("u128") {
                if let Some(serde_json::Value::Object(inner)) = map.get_mut("u128") {
                    // Convert UInt128Parts (hi: u64, lo: u64)
                    if let Some(serde_json::Value::String(s)) = inner.get("hi") {
                        if let Ok(num) = s.parse::<u64>() {
                            inner.insert("hi".to_string(), serde_json::json!(num));
                        }
                    }
                    if let Some(serde_json::Value::String(s)) = inner.get("lo") {
                        if let Ok(num) = s.parse::<u64>() {
                            inner.insert("lo".to_string(), serde_json::json!(num));
                        }
                    }
                }
            }

            // Handle i128 wrapper object
            if map.contains_key("i128") {
                if let Some(serde_json::Value::Object(inner)) = map.get_mut("i128") {
                    // Convert Int128Parts (hi: i64, lo: u64)
                    if let Some(serde_json::Value::String(s)) = inner.get("hi") {
                        if let Ok(num) = s.parse::<i64>() {
                            inner.insert("hi".to_string(), serde_json::json!(num));
                        }
                    }
                    if let Some(serde_json::Value::String(s)) = inner.get("lo") {
                        if let Ok(num) = s.parse::<u64>() {
                            inner.insert("lo".to_string(), serde_json::json!(num));
                        }
                    }
                }
            }

            // Handle u256 wrapper object
            if map.contains_key("u256") {
                if let Some(serde_json::Value::Object(inner)) = map.get_mut("u256") {
                    // Convert UInt256Parts (all u64)
                    for key in ["hi_hi", "hi_lo", "lo_hi", "lo_lo"] {
                        if let Some(serde_json::Value::String(s)) = inner.get(key) {
                            if let Ok(num) = s.parse::<u64>() {
                                inner.insert(key.to_string(), serde_json::json!(num));
                            }
                        }
                    }
                }
            }

            // Handle i256 wrapper object
            if map.contains_key("i256") {
                if let Some(serde_json::Value::Object(inner)) = map.get_mut("i256") {
                    // Convert Int256Parts (hi_hi: i64, others: u64)
                    if let Some(serde_json::Value::String(s)) = inner.get("hi_hi") {
                        if let Ok(num) = s.parse::<i64>() {
                            inner.insert("hi_hi".to_string(), serde_json::json!(num));
                        }
                    }
                    for key in ["hi_lo", "lo_hi", "lo_lo"] {
                        if let Some(serde_json::Value::String(s)) = inner.get(key) {
                            if let Ok(num) = s.parse::<u64>() {
                                inner.insert(key.to_string(), serde_json::json!(num));
                            }
                        }
                    }
                }
            }

            // Also handle direct UInt256Parts (all u64) without wrapper
            if map.contains_key("hi_hi")
                && map.contains_key("hi_lo")
                && map.contains_key("lo_hi")
                && map.contains_key("lo_lo")
                && map.len() == 4
            {
                for key in ["hi_hi", "hi_lo", "lo_hi", "lo_lo"] {
                    if let Some(serde_json::Value::String(s)) = map.get(key) {
                        if let Ok(num) = s.parse::<u64>() {
                            map.insert(key.to_string(), serde_json::json!(num));
                        }
                    }
                }
            }

            // Recursively process nested structures
            for (_, v) in map.iter_mut() {
                fix_u64_format(v);
            }
        }
        serde_json::Value::Array(arr) => {
            // Recursively fix all array elements
            for v in arr.iter_mut() {
                fix_u64_format(v);
            }
        }
        _ => {}
    }
}

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
    // HACK: stellar-xdr expects u64 as number but it should be string
    // Convert {"u64":"1000"} to {"u64":1000} before deserialization
    let scval_args: Vec<ScVal> = args
        .iter()
        .map(|json| {
            let mut modified_json = json.clone();
            fix_u64_format(&mut modified_json);
            serde_json::from_value(modified_json)
        })
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
            // HACK: stellar-xdr expects u64 as number but it should be string
            // Convert {"u64":"1000"} to {"u64":1000} before deserialization
            let scval_args: Vec<ScVal> = args
                .iter()
                .map(|json| {
                    let mut modified_json = json.clone();
                    fix_u64_format(&mut modified_json);
                    serde_json::from_value(modified_json)
                })
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

    #[test]
    fn test_u64_string_to_number_conversion() {
        // Test direct u64 conversion
        let args = vec![
            json!({"u64": "1000"}),
            json!({"i64": "-500"}),
            json!({"timepoint": "123456"}),
            json!({"duration": "7890"}),
        ];

        let result = convert_invoke_contract(TEST_CONTRACT.to_string(), "test".to_string(), args);
        assert!(
            result.is_ok(),
            "Should successfully convert string u64/i64 to numbers"
        );

        // Test nested u128 parts
        let u128_arg = vec![json!({"u128": {"hi": "100", "lo": "200"}})];
        let result =
            convert_invoke_contract(TEST_CONTRACT.to_string(), "test".to_string(), u128_arg);
        assert!(result.is_ok(), "Should successfully convert u128 parts");

        // Test nested i128 parts
        let i128_arg = vec![json!({"i128": {"hi": "-100", "lo": "200"}})];
        let result =
            convert_invoke_contract(TEST_CONTRACT.to_string(), "test".to_string(), i128_arg);
        assert!(result.is_ok(), "Should successfully convert i128 parts");

        // Test nested u256 parts
        let u256_arg =
            vec![json!({"u256": {"hi_hi": "1", "hi_lo": "2", "lo_hi": "3", "lo_lo": "4"}})];
        let result =
            convert_invoke_contract(TEST_CONTRACT.to_string(), "test".to_string(), u256_arg);
        assert!(result.is_ok(), "Should successfully convert u256 parts");

        // Test nested i256 parts
        let i256_arg =
            vec![json!({"i256": {"hi_hi": "-1", "hi_lo": "2", "lo_hi": "3", "lo_lo": "4"}})];
        let result =
            convert_invoke_contract(TEST_CONTRACT.to_string(), "test".to_string(), i256_arg);
        assert!(result.is_ok(), "Should successfully convert i256 parts");
    }

    #[test]
    fn test_host_function_spec_json_format() {
        // Test InvokeContract
        let invoke = HostFunctionSpec::InvokeContract {
            contract_address: TEST_CONTRACT.to_string(),
            function_name: "test".to_string(),
            args: vec![json!({"u64": 42})],
        };
        let invoke_json = serde_json::to_value(&invoke).unwrap();
        assert_eq!(invoke_json["type"], "invoke_contract");
        assert_eq!(invoke_json["contract_address"], TEST_CONTRACT);
        assert_eq!(invoke_json["function_name"], "test");

        // Test UploadWasm
        let upload = HostFunctionSpec::UploadWasm {
            wasm: WasmSource::Hex {
                hex: "deadbeef".to_string(),
            },
        };
        let upload_json = serde_json::to_value(&upload).unwrap();
        assert_eq!(upload_json["type"], "upload_wasm");
        assert!(upload_json["wasm"].is_object());

        // Test CreateContract
        let create = HostFunctionSpec::CreateContract {
            source: ContractSource::Address {
                address: TEST_PK.to_string(),
            },
            wasm_hash: "0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            salt: None,
            constructor_args: None,
        };
        let create_json = serde_json::to_value(&create).unwrap();
        assert_eq!(create_json["type"], "create_contract");
        assert_eq!(create_json["source"]["from"], "address");
        assert_eq!(create_json["source"]["address"], TEST_PK);
    }
}
