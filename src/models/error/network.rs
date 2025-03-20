use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("Invalid network: {0}")]
    InvalidNetwork(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_error_creation() {
        let error = NetworkError::InvalidNetwork("ethereum".to_string());
        assert!(matches!(error, NetworkError::InvalidNetwork(_)));
    }

    #[test]
    fn test_network_error_display() {
        let error = NetworkError::InvalidNetwork("polygon".to_string());
        assert_eq!(error.to_string(), "Invalid network: polygon");
    }
}
