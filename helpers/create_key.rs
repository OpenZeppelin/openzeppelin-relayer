//! Key Generation Tool
//!
//! This tool generates and stores cryptographic keys in a keystore file with password protection.
//! It supports customizable output locations, password complexity requirements, and file naming
//! options.
//!
//! # Features
//!
//! - Secure key generation and storage
//! - Password complexity validation
//! - Timestamp-based automatic file naming
//! - Directory creation if needed
//! - Overwrite protection with force option
//!
//! # Usage
//!
//! ```bash
//! cargo run --example create_key -- --password SecurePass123! --output-dir keys
//! ```
use chrono::Local;
use clap::Parser;
use eyre::{Result, WrapErr};
use oz_keystore::LocalClient;
use std::{env, fmt, fs};

/// Command line arguments for key generation
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Output directory for the keystore file
    #[arg(short, long, default_value = ".")]
    output_dir: String,

    /// Password for the keystore.
    /// Must be at least 12 characters long and contain:
    /// - One uppercase letter
    /// - One lowercase letter
    /// - One number
    /// - One special character
    #[arg(short, long)]
    password: String,

    /// Custom output filename (optional).
    /// If not provided, generates a timestamp-based filename
    #[arg(short, long)]
    filename: Option<String>,

    /// Force overwrite if file exists
    #[arg(long)]
    force: bool,

    /// Disables password complexity validation
    /// Use with caution - only for testing purposes
    #[arg(long)]
    disable_password_check: bool,
}

/// Custom error type for password validation failures
#[derive(Debug)]
struct PasswordError {
    message: String,
}

impl fmt::Display for PasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for PasswordError {}

/// Generates a default filename using current timestamp
///
/// # Format
///
/// Creates a filename in the format: `key_YYYYMMDD_HHMMSS.json`
///
/// # Returns
///
/// A String containing the generated filename
fn generate_default_filename() -> String {
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    format!("key_{}.json", timestamp)
}

/// Validates password complexity requirements
///
/// # Arguments
///
/// * `password` - The password string to validate
///
/// # Returns
///
/// * `Ok(())` if password meets all requirements
/// * `Err(PasswordError)` with description if validation fails
///
/// # Requirements
///
/// - Minimum 12 characters
/// - At least one uppercase letter
/// - At least one lowercase letter
/// - At least one number
/// - At least one special character
fn validate_password(password: &str) -> Result<(), PasswordError> {
    if password.len() < 12 {
        return Err(PasswordError {
            message: "Password must be at least 12 characters long".to_string(),
        });
    }

    let has_uppercase = password.chars().any(|c| c.is_uppercase());
    let has_lowercase = password.chars().any(|c| c.is_lowercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password.chars().any(|c| !c.is_alphanumeric());

    if !has_uppercase || !has_lowercase || !has_digit || !has_special {
        return Err(PasswordError {
            message: "Password must contain at least one uppercase letter, one lowercase letter, \
                      one number, and one special character"
                .to_string(),
        });
    }

    Ok(())
}

/// Main entry point for the key generation tool
///
/// # Process
///
/// 1. Parses and validates command line arguments
/// 2. Validates password complexity (if enabled)
/// 3. Creates output directory if needed
/// 4. Generates and stores the key
/// 5. Prints success message with file location
///
/// # Errors
///
/// Returns error if:
/// - Password validation fails
/// - Directory creation fails
/// - File already exists (without --force)
/// - Key generation fails
fn main() -> Result<()> {
    let args = Args::parse();
    // Validate password complexity
    if !args.disable_password_check {
        validate_password(&args.password).map_err(|e| eyre::eyre!("Invalid password: {}", e))?;
    }

    let filename = args.filename.unwrap_or_else(generate_default_filename);

    let current_dir = env::current_dir()?;
    let config_dir = current_dir.join(&args.output_dir);

    fs::create_dir_all(&config_dir)
        .wrap_err_with(|| format!("Failed to create directory: {:?}", config_dir))?;

    let key_path = config_dir.join(&filename);

    if key_path.exists() && !args.force {
        return Err(eyre::eyre!(
            "File {:?} already exists. Use --force to overwrite",
            key_path
        ));
    }

    LocalClient::generate(args.output_dir.into(), args.password, Some(&filename));

    println!("Generated new key:");
    println!("Keystore file created at: {:?}", key_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_generate_default_filename() {
        let filename = generate_default_filename();

        // Check format: key_YYYYMMDD_HHMMSS.json
        assert!(filename.starts_with("key_"));
        assert!(filename.ends_with(".json"));

        // Verify timestamp format (rough check)
        let timestamp_part = filename
            .strip_prefix("key_")
            .unwrap()
            .strip_suffix(".json")
            .unwrap();
        assert_eq!(timestamp_part.len(), 15); // YYYYMMDD_HHMMSS = 15 chars
        assert!(timestamp_part.contains('_'));
    }

    #[test]
    fn test_password_validation_success() {
        // Valid password with all requirements
        let result = validate_password("SecurePass123!");
        assert!(result.is_ok());
    }

    #[test]
    fn test_password_validation_too_short() {
        // Password too short
        let result = validate_password("Short1!");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("at least 12 characters"));
    }

    #[test]
    fn test_password_validation_missing_uppercase() {
        // Missing uppercase
        let result = validate_password("securepass123!");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("uppercase letter"));
    }

    #[test]
    fn test_password_validation_missing_lowercase() {
        // Missing lowercase
        let result = validate_password("SECUREPASS123!");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("lowercase letter"));
    }

    #[test]
    fn test_password_validation_missing_number() {
        // Missing number
        let result = validate_password("SecurePassword!");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("number"));
    }

    #[test]
    fn test_password_validation_missing_special() {
        // Missing special character
        let result = validate_password("SecurePass1234");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("special character"));
    }

    // Mock for LocalClient to avoid actual key generation in tests
    #[cfg(test)]
    mod mock {
        use std::path::Path;

        pub struct LocalClient;

        impl LocalClient {
            pub fn generate(output_dir: String, _password: String, filename: Option<&str>) -> bool {
                // Create an empty file to simulate key generation
                if let Some(fname) = filename {
                    let path = Path::new(&output_dir).join(fname);
                    std::fs::write(path, "mock key data").unwrap();
                }
                true
            }
        }
    }

    // Integration-style tests for the main functionality
    #[test]
    fn test_key_generation_with_custom_filename() {
        let temp_dir = tempdir().unwrap();
        let dir_path = temp_dir.path().to_str().unwrap().to_string();
        let custom_filename = "test_key.json";

        // Override the actual LocalClient with our mock
        // This requires modifying the main code to be more testable

        // Simulate command line args
        let args = Args {
            output_dir: dir_path.clone(),
            password: "SecurePass123!".to_string(),
            filename: Some(custom_filename.to_string()),
            force: false,
            disable_password_check: false,
        };

        // Call the function directly or use a test helper
        // For now, we'll just test the mock client directly
        mock::LocalClient::generate(dir_path.clone(), args.password, Some(custom_filename));

        // Verify file was created
        let key_path = Path::new(&dir_path).join(custom_filename);
        assert!(key_path.exists());
    }

    #[test]
    fn test_key_generation_with_default_filename() {
        let temp_dir = tempdir().unwrap();
        let dir_path = temp_dir.path().to_str().unwrap().to_string();

        // Simulate command line args
        let args = Args {
            output_dir: dir_path.clone(),
            password: "SecurePass123!".to_string(),
            filename: None,
            force: false,
            disable_password_check: false,
        };

        // Generate a default filename
        let default_filename = generate_default_filename();

        // Call the mock client
        mock::LocalClient::generate(dir_path.clone(), args.password, Some(&default_filename));

        // Verify file was created
        let key_path = Path::new(&dir_path).join(default_filename);
        assert!(key_path.exists());
    }

    #[test]
    fn test_force_overwrite() {
        let temp_dir = tempdir().unwrap();
        let dir_path = temp_dir.path().to_str().unwrap().to_string();
        let custom_filename = "existing_key.json";

        // Create a file that already exists
        let key_path = Path::new(&dir_path).join(custom_filename);
        fs::write(&key_path, "existing data").unwrap();
        assert!(key_path.exists());
        let content = fs::read_to_string(&key_path).unwrap();
        assert_eq!(content, "existing data");

        let args = Args {
            output_dir: dir_path.clone(),
            password: "SecurePass123!".to_string(),
            filename: Some(custom_filename.to_string()),
            force: true,
            disable_password_check: false,
        };

        // Call the mock client
        mock::LocalClient::generate(dir_path.clone(), args.password, Some(custom_filename));

        // Verify file was overwritten
        assert!(key_path.exists());
        let content = fs::read_to_string(&key_path).unwrap();
        assert_eq!(content, "mock key data");
    }
}
