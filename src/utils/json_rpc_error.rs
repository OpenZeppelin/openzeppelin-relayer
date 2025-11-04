//! JSON-RPC error codes from JSON-RPC 2.0 and EIP-1474

/// Standard JSON-RPC 2.0 error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardJsonRpcError {
    /// Invalid JSON was received by the server
    ParseError = -32700,
    /// The JSON sent is not a valid Request object
    InvalidRequest = -32600,
    /// The method does not exist / is not available
    MethodNotFound = -32601,
    /// Invalid method parameter(s)
    InvalidParams = -32602,
    /// Internal JSON-RPC error
    InternalError = -32603,
}

/// Ethereum-specific JSON-RPC error codes (EIP-1474)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EthereumJsonRpcError {
    /// Invalid input
    InvalidInput = -32000,
    /// Resource not found
    ResourceNotFound = -32001,
    /// Resource unavailable
    ResourceUnavailable = -32002,
    /// Transaction rejected
    TransactionRejected = -32003,
    /// Method not supported
    MethodNotSupported = -32004,
    /// Request limit exceeded
    LimitExceeded = -32005,
    /// JSON-RPC version not supported
    JsonRpcVersionNotSupported = -32006,
}

impl StandardJsonRpcError {
    pub const fn code(self) -> i64 {
        self as i64
    }

    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            -32700 => Some(Self::ParseError),
            -32600 => Some(Self::InvalidRequest),
            -32601 => Some(Self::MethodNotFound),
            -32602 => Some(Self::InvalidParams),
            -32603 => Some(Self::InternalError),
            _ => None,
        }
    }
}

impl EthereumJsonRpcError {
    pub const fn code(self) -> i64 {
        self as i64
    }

    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            -32000 => Some(Self::InvalidInput),
            -32001 => Some(Self::ResourceNotFound),
            -32002 => Some(Self::ResourceUnavailable),
            -32003 => Some(Self::TransactionRejected),
            -32004 => Some(Self::MethodNotSupported),
            -32005 => Some(Self::LimitExceeded),
            -32006 => Some(Self::JsonRpcVersionNotSupported),
            _ => None,
        }
    }
}

/// Check if a JSON-RPC error code represents a retriable error.
pub const fn is_retriable_error_code(code: i64) -> bool {
    code == StandardJsonRpcError::InternalError as i64
        || code == EthereumJsonRpcError::ResourceUnavailable as i64
        || code == EthereumJsonRpcError::LimitExceeded as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_error_code_values() {
        assert_eq!(StandardJsonRpcError::ParseError.code(), -32700);
        assert_eq!(StandardJsonRpcError::InvalidRequest.code(), -32600);
        assert_eq!(StandardJsonRpcError::MethodNotFound.code(), -32601);
        assert_eq!(StandardJsonRpcError::InvalidParams.code(), -32602);
        assert_eq!(StandardJsonRpcError::InternalError.code(), -32603);
    }

    #[test]
    fn test_ethereum_error_code_values() {
        assert_eq!(EthereumJsonRpcError::InvalidInput.code(), -32000);
        assert_eq!(EthereumJsonRpcError::ResourceNotFound.code(), -32001);
        assert_eq!(EthereumJsonRpcError::ResourceUnavailable.code(), -32002);
        assert_eq!(EthereumJsonRpcError::TransactionRejected.code(), -32003);
        assert_eq!(EthereumJsonRpcError::MethodNotSupported.code(), -32004);
        assert_eq!(EthereumJsonRpcError::LimitExceeded.code(), -32005);
        assert_eq!(
            EthereumJsonRpcError::JsonRpcVersionNotSupported.code(),
            -32006
        );
    }

    #[test]
    fn test_standard_error_from_code_valid() {
        assert_eq!(
            StandardJsonRpcError::from_code(-32700),
            Some(StandardJsonRpcError::ParseError)
        );
        assert_eq!(
            StandardJsonRpcError::from_code(-32600),
            Some(StandardJsonRpcError::InvalidRequest)
        );
        assert_eq!(
            StandardJsonRpcError::from_code(-32601),
            Some(StandardJsonRpcError::MethodNotFound)
        );
        assert_eq!(
            StandardJsonRpcError::from_code(-32602),
            Some(StandardJsonRpcError::InvalidParams)
        );
        assert_eq!(
            StandardJsonRpcError::from_code(-32603),
            Some(StandardJsonRpcError::InternalError)
        );
    }

    #[test]
    fn test_standard_error_from_code_invalid() {
        assert_eq!(StandardJsonRpcError::from_code(-32000), None);
        assert_eq!(StandardJsonRpcError::from_code(-32699), None);
        assert_eq!(StandardJsonRpcError::from_code(0), None);
        assert_eq!(StandardJsonRpcError::from_code(500), None);
    }

    #[test]
    fn test_ethereum_error_from_code_valid() {
        assert_eq!(
            EthereumJsonRpcError::from_code(-32000),
            Some(EthereumJsonRpcError::InvalidInput)
        );
        assert_eq!(
            EthereumJsonRpcError::from_code(-32001),
            Some(EthereumJsonRpcError::ResourceNotFound)
        );
        assert_eq!(
            EthereumJsonRpcError::from_code(-32002),
            Some(EthereumJsonRpcError::ResourceUnavailable)
        );
        assert_eq!(
            EthereumJsonRpcError::from_code(-32003),
            Some(EthereumJsonRpcError::TransactionRejected)
        );
        assert_eq!(
            EthereumJsonRpcError::from_code(-32004),
            Some(EthereumJsonRpcError::MethodNotSupported)
        );
        assert_eq!(
            EthereumJsonRpcError::from_code(-32005),
            Some(EthereumJsonRpcError::LimitExceeded)
        );
        assert_eq!(
            EthereumJsonRpcError::from_code(-32006),
            Some(EthereumJsonRpcError::JsonRpcVersionNotSupported)
        );
    }

    #[test]
    fn test_ethereum_error_from_code_invalid() {
        assert_eq!(EthereumJsonRpcError::from_code(-32700), None);
        assert_eq!(EthereumJsonRpcError::from_code(-32007), None);
        assert_eq!(EthereumJsonRpcError::from_code(-31999), None);
        assert_eq!(EthereumJsonRpcError::from_code(0), None);
    }

    #[test]
    fn test_is_retriable_error_code_retriable() {
        assert!(is_retriable_error_code(-32603));
        assert!(is_retriable_error_code(-32002));
        assert!(is_retriable_error_code(-32005));
    }

    #[test]
    fn test_is_retriable_error_code_non_retriable_standard() {
        assert!(!is_retriable_error_code(-32700));
        assert!(!is_retriable_error_code(-32600));
        assert!(!is_retriable_error_code(-32601));
        assert!(!is_retriable_error_code(-32602));
    }

    #[test]
    fn test_is_retriable_error_code_non_retriable_ethereum() {
        assert!(!is_retriable_error_code(-32000));
        assert!(!is_retriable_error_code(-32001));
        assert!(!is_retriable_error_code(-32003));
        assert!(!is_retriable_error_code(-32004));
        assert!(!is_retriable_error_code(-32006));
    }

    #[test]
    fn test_is_retriable_error_code_unknown() {
        assert!(!is_retriable_error_code(0));
        assert!(!is_retriable_error_code(404));
        assert!(!is_retriable_error_code(500));
        assert!(!is_retriable_error_code(-1));
        assert!(!is_retriable_error_code(-32100));
        assert!(!is_retriable_error_code(-32769));
        assert!(!is_retriable_error_code(-31999));
    }

    #[test]
    fn test_standard_error_roundtrip() {
        let errors = [
            StandardJsonRpcError::ParseError,
            StandardJsonRpcError::InvalidRequest,
            StandardJsonRpcError::MethodNotFound,
            StandardJsonRpcError::InvalidParams,
            StandardJsonRpcError::InternalError,
        ];

        for error in errors {
            let code = error.code();
            let reconstructed = StandardJsonRpcError::from_code(code);
            assert_eq!(reconstructed, Some(error));
        }
    }

    #[test]
    fn test_ethereum_error_roundtrip() {
        let errors = [
            EthereumJsonRpcError::InvalidInput,
            EthereumJsonRpcError::ResourceNotFound,
            EthereumJsonRpcError::ResourceUnavailable,
            EthereumJsonRpcError::TransactionRejected,
            EthereumJsonRpcError::MethodNotSupported,
            EthereumJsonRpcError::LimitExceeded,
            EthereumJsonRpcError::JsonRpcVersionNotSupported,
        ];

        for error in errors {
            let code = error.code();
            let reconstructed = EthereumJsonRpcError::from_code(code);
            assert_eq!(reconstructed, Some(error));
        }
    }
}
