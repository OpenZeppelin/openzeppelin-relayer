//! UUID Key Generation Tool
//!
//! This tool generates random UUID key and prints it to the console.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example generate_uuid
//! ```
use eyre::Result;
use uuid::Uuid;

/// Main entry point for uuid key generation tool
fn main() -> Result<()> {
    let uuid = Uuid::new_v4().to_string();

    println!("Generated new uuid: {}", uuid);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_uuid_generation() {
        let uuid_string = Uuid::new_v4().to_string();

        let parsed_uuid = Uuid::from_str(&uuid_string);
        assert!(parsed_uuid.is_ok(), "Generated string is not a valid UUID");

        let uuid = parsed_uuid.unwrap();
        assert_eq!(uuid.get_version_num(), 4, "UUID is not version 4");

        let uuid1 = Uuid::new_v4();
        let uuid2 = Uuid::new_v4();
        assert_ne!(uuid1, uuid2, "Two generated UUIDs should not be equal");
    }
}
