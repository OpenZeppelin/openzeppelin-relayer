/// Custom OpenZeppelin error codes for extended JSON-RPC functionality.
///
/// These error codes extend the standard JSON-RPC 2.0 error codes with
/// OpenZeppelin-specific error conditions. All codes start from -33000
/// to avoid conflicts with standard JSON-RPC codes (-32000 to -32099)
/// and other common extensions.
pub struct OpenZeppelinErrorCodes;

impl OpenZeppelinErrorCodes {
    /// Request timeout - The request took too long to complete.
    pub const TIMEOUT: i32 = -33000;

    /// Rate limited - Too many requests, client should back off.
    pub const RATE_LIMITED: i32 = -33001;

    /// Bad gateway - The upstream server returned an invalid response.
    pub const BAD_GATEWAY: i32 = -33002;

    /// Request error - Generic request error with additional HTTP status context.
    pub const REQUEST_ERROR: i32 = -33003;

    /// Network configuration error - Issues with network or provider configuration.
    pub const NETWORK_CONFIGURATION: i32 = -33004;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::manual_range_contains)]
    fn test_openzeppelin_error_codes_are_not_in_reserved_range() {
        let codes = vec![
            OpenZeppelinErrorCodes::TIMEOUT,
            OpenZeppelinErrorCodes::RATE_LIMITED,
            OpenZeppelinErrorCodes::BAD_GATEWAY,
            OpenZeppelinErrorCodes::REQUEST_ERROR,
            OpenZeppelinErrorCodes::NETWORK_CONFIGURATION,
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
    fn test_openzeppelin_error_codes_are_unique() {
        let codes = vec![
            OpenZeppelinErrorCodes::TIMEOUT,
            OpenZeppelinErrorCodes::RATE_LIMITED,
            OpenZeppelinErrorCodes::BAD_GATEWAY,
            OpenZeppelinErrorCodes::REQUEST_ERROR,
            OpenZeppelinErrorCodes::NETWORK_CONFIGURATION,
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

    #[test]
    fn test_openzeppelin_error_codes_start_from_33000() {
        let codes = vec![
            OpenZeppelinErrorCodes::TIMEOUT,
            OpenZeppelinErrorCodes::RATE_LIMITED,
            OpenZeppelinErrorCodes::BAD_GATEWAY,
            OpenZeppelinErrorCodes::REQUEST_ERROR,
            OpenZeppelinErrorCodes::NETWORK_CONFIGURATION,
        ];

        for code in codes {
            assert!(
                code <= -33000,
                "All OpenZeppelin codes should be <= -33000, found: {}",
                code
            );
        }
    }
}
