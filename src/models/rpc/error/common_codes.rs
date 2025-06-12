/// JSON-RPC 2.0 error codes as defined in the specification.
///
/// These constants represent the standard error codes used in JSON-RPC 2.0 protocol.
/// Each error code has a specific meaning and should be used consistently across
/// all RPC implementations.
pub struct RpcErrorCodes;

impl RpcErrorCodes {
    /// Parse error - Invalid JSON was received by the server.
    pub const PARSE: i32 = -32700;

    /// Invalid Request - The JSON sent is not a valid Request object.
    pub const INVALID_REQUEST: i32 = -32600;

    /// Method not found - The method does not exist / is not available.
    pub const METHOD_NOT_FOUND: i32 = -32601;

    /// Invalid params - Invalid method parameter(s).
    pub const INVALID_PARAMS: i32 = -32602;

    /// Internal error - Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i32 = -32603;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::manual_range_contains)]
    fn test_rpc_error_codes_are_outside_of_reserved_range() {
        let codes = vec![
            RpcErrorCodes::PARSE,
            RpcErrorCodes::INVALID_REQUEST,
            RpcErrorCodes::METHOD_NOT_FOUND,
            RpcErrorCodes::INVALID_PARAMS,
            RpcErrorCodes::INTERNAL_ERROR,
        ];

        for code in codes {
            assert!(
                !(code >= -32099 && code <= -32000),
                "Code {} is part of the reserved range for implementation-defined server errors",
                code
            );
        }
    }

    #[test]
    fn test_rpc_error_codes_are_unique() {
        let codes = vec![
            RpcErrorCodes::PARSE,
            RpcErrorCodes::INVALID_REQUEST,
            RpcErrorCodes::METHOD_NOT_FOUND,
            RpcErrorCodes::INVALID_PARAMS,
            RpcErrorCodes::INTERNAL_ERROR,
        ];

        let mut unique_codes = codes.clone();
        unique_codes.sort();
        unique_codes.dedup();

        assert_eq!(
            codes.len(),
            unique_codes.len(),
            "All error codes should be unique"
        );
    }
}
