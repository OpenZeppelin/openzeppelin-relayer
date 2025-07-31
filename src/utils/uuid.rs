//! UUID utilities.
//!
//! This module provides utilities for generating UUIDs.
use uuid::Uuid;

/// Generate a new UUID.
pub fn generate_uuid() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_uuid() {
        let uuid = generate_uuid();
        assert_eq!(uuid.len(), 36);
    }
}
