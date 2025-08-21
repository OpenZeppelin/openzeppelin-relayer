//!
//! Utility functions for formatting token amounts for display.
//!
//! Provides helpers for converting raw token values to human-readable strings with decimal places.

/// Formats a token amount with the specified number of decimal places
pub fn format_token_amount(amount: u128, decimals: u32) -> String {
    format!(
        "{:.*}",
        decimals as usize,
        amount as f64 / 10f64.powi(decimals as i32)
    )
}

/// Converts a token amount from one decimal precision to another
///
/// # Arguments
/// * `amount` - The token amount in the source precision
/// * `from_decimals` - The number of decimals in the source precision
/// * `to_decimals` - The number of decimals in the target precision
///
/// # Returns
/// The converted amount in the target precision
///
/// # Example
/// ```
/// // Convert 1 DUST (6 decimals) to 18 decimals precision
/// let wei_amount = convert_token_decimals(1_000_000, 6, 18);
/// assert_eq!(wei_amount, 1_000_000_000_000_000_000);
///
/// // Convert 1 ETH (18 decimals) to 6 decimals precision
/// let usdc_amount = convert_token_decimals(1_000_000_000_000_000_000, 18, 6);
/// assert_eq!(usdc_amount, 1_000_000);
/// ```
pub fn convert_token_decimals(amount: u128, from_decimals: u32, to_decimals: u32) -> u128 {
    if from_decimals == to_decimals {
        return amount;
    }

    if from_decimals > to_decimals {
        // Reduce precision
        let divisor = 10u128.pow(from_decimals - to_decimals);
        amount / divisor
    } else {
        // Increase precision
        let multiplier = 10u128.pow(to_decimals - from_decimals);
        amount * multiplier
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_token_amount() {
        // Test ETH (18 decimals)
        assert_eq!(
            format_token_amount(1_000_000_000_000_000_000, 18),
            "1.000000000000000000"
        );
        assert_eq!(
            format_token_amount(1_500_000_000_000_000_000, 18),
            "1.500000000000000000"
        );

        // Test USDC (6 decimals)
        assert_eq!(format_token_amount(1_000_000, 6), "1.000000");
        assert_eq!(format_token_amount(1_500_000, 6), "1.500000");

        // Test DUST (6 decimals)
        assert_eq!(format_token_amount(1_000_000, 6), "1.000000");
        assert_eq!(format_token_amount(999_999, 6), "0.999999");

        // Test edge cases
        assert_eq!(format_token_amount(0, 6), "0.000000");
        assert_eq!(format_token_amount(1, 6), "0.000001");
    }

    #[test]
    fn test_convert_token_decimals() {
        // Same decimals - no conversion
        assert_eq!(convert_token_decimals(1_000_000, 6, 6), 1_000_000);

        // Convert from lower to higher decimals
        assert_eq!(
            convert_token_decimals(1_000_000, 6, 18),
            1_000_000_000_000_000_000
        );
        assert_eq!(convert_token_decimals(1, 6, 18), 1_000_000_000_000);
        assert_eq!(convert_token_decimals(1_500_000, 6, 9), 1_500_000_000);

        // Convert from higher to lower decimals
        assert_eq!(
            convert_token_decimals(1_000_000_000_000_000_000, 18, 6),
            1_000_000
        );
        assert_eq!(convert_token_decimals(1_000_000_000_000, 18, 6), 1); // 0.000001
        assert_eq!(convert_token_decimals(999_999_999_999, 18, 6), 0); // Loss of precision
        assert_eq!(convert_token_decimals(1_500_000_000, 9, 6), 1_500_000);

        // Edge cases
        assert_eq!(convert_token_decimals(0, 6, 18), 0);
        assert_eq!(convert_token_decimals(0, 18, 6), 0);

        // DUST to mDUST conversion (6 decimals to 3 decimals)
        assert_eq!(convert_token_decimals(1_000_000, 6, 3), 1_000);
        assert_eq!(convert_token_decimals(1_500_000, 6, 3), 1_500);
        assert_eq!(convert_token_decimals(999_999, 6, 3), 999);
        assert_eq!(convert_token_decimals(999, 6, 3), 0); // Loss of precision
    }
}
