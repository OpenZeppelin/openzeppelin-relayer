/// Converts minutes to milliseconds
pub const fn minutes_ms(minutes: i64) -> i64 {
    minutes * 60 * 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minutes_ms() {
        assert_eq!(minutes_ms(1), 60_000);
        assert_eq!(minutes_ms(5), 300_000);
        assert_eq!(minutes_ms(10), 600_000);
    }
}
