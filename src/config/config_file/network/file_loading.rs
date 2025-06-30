//! Network Configuration File Loading
//!
//! This module provides utilities for loading network configurations from JSON files
//! and directories, supporting both single-file and directory-based configuration layouts.
//!
//! ## Key Features
//!
//! - **Flexible loading**: Single files or entire directories of JSON configuration files
//! - **Automatic discovery**: Scans directories for `.json` files recursively
//! - **Validation**: Pre-loading validation to ensure directory contains valid configurations
//!
//! ## Supported File Structure
//!
//! ```text
//! networks/
//! ├── evm.json          # {"networks": [...]}
//! ├── solana.json       # {"networks": [...]}
//! └── stellar.json      # {"networks": [...]}
//! ```
//!
//! ## Loading Process
//!
//! ### Directory Loading
//! 1. **Discovery**: Scans directory for `.json` files (non-recursive)
//! 2. **Validation**: Checks each file for proper JSON structure
//! 3. **Parsing**: Deserializes each file into network configurations
//! 4. **Aggregation**: Combines all configurations into a single collection
//! 5. **Error handling**: Provides detailed context for any failures
//!
//! ### File Format Requirements
//! Each JSON file must contain a top-level `networks` array:
//! ```json
//! {
//!   "networks": [
//!     {
//!       "type": "evm",
//!       "network": "ethereum-mainnet",
//!       "chain_id": 1,
//!       "required_confirmations": 12,
//!       "symbol": "ETH",
//!       "rpc_urls": ["https://eth.llamarpc.com"]
//!     }
//!   ]
//! }
//! ```
//!
//! ### Error Handling
//! - **File not found**: Directory or individual files don't exist
//! - **Permission errors**: Insufficient permissions to read files
//! - **JSON parse errors**: Malformed JSON with line/column information
//! - **Structure validation**: Missing required fields or wrong data types
//! - **Empty collections**: Directories with no valid configuration files

use super::NetworkFileConfig;
use crate::config::ConfigFileError;
use serde::Deserialize;
use std::fs;
use std::path::Path;

// Helper struct for JSON files in the directory
#[derive(Deserialize, Debug, Clone)]
struct DirectoryNetworkList {
    networks: Vec<NetworkFileConfig>,
}

pub struct NetworkFileLoader;

impl NetworkFileLoader {
    /// Reads and aggregates network configurations from all JSON files in the specified directory.
    ///
    /// # Arguments
    /// * `path` - A path reference to the directory containing network configuration files.
    ///
    /// # Returns
    /// - `Ok(Vec<NetworkFileConfig>)` containing all network configurations loaded from the directory.
    /// - `Err(ConfigFileError)` with detailed context about what went wrong.
    pub fn load_networks_from_directory(
        path: impl AsRef<Path>,
    ) -> Result<Vec<NetworkFileConfig>, ConfigFileError> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(ConfigFileError::InvalidFormat(format!(
                "Path '{}' does not exist",
                path.display()
            )));
        }

        if !path.is_dir() {
            return Err(ConfigFileError::InvalidFormat(format!(
                "Path '{}' is not a directory",
                path.display()
            )));
        }

        // Validate that the directory contains at least one JSON configuration file
        Self::validate_directory_has_configs(path)?;

        let mut aggregated_networks = Vec::new();

        // Read directory entries with better error handling
        let entries = fs::read_dir(path).map_err(|e| {
            ConfigFileError::InvalidFormat(format!(
                "Failed to read directory '{}': {}",
                path.display(),
                e
            ))
        })?;

        for entry_result in entries {
            let entry = entry_result.map_err(|e| {
                ConfigFileError::InvalidFormat(format!(
                    "Failed to read directory entry in '{}': {}",
                    path.display(),
                    e
                ))
            })?;

            let file_path = entry.path();

            // Only process JSON files, skip directories and other file types
            if Self::is_json_file(&file_path) {
                match Self::load_network_file(&file_path) {
                    Ok(mut networks) => {
                        aggregated_networks.append(&mut networks);
                    }
                    Err(e) => {
                        // Provide context about which file failed
                        return Err(ConfigFileError::InvalidFormat(format!(
                            "Failed to load network configuration from file '{}': {}",
                            file_path.display(),
                            e
                        )));
                    }
                }
            }
        }

        Ok(aggregated_networks)
    }

    /// Loads a single network configuration file.
    ///
    /// # Arguments
    /// * `file_path` - Path to the JSON file containing network configurations.
    ///
    /// # Returns
    /// - `Ok(Vec<NetworkFileConfig>)` containing the networks from the file.
    /// - `Err(ConfigFileError)` if the file cannot be read or parsed.
    fn load_network_file(file_path: &Path) -> Result<Vec<NetworkFileConfig>, ConfigFileError> {
        let file_content = fs::read_to_string(file_path)
            .map_err(|e| ConfigFileError::InvalidFormat(format!("Failed to read file: {}", e)))?;

        let dir_network_list: DirectoryNetworkList = serde_json::from_str(&file_content)
            .map_err(|e| ConfigFileError::InvalidFormat(format!("Failed to parse JSON: {}", e)))?;

        Ok(dir_network_list.networks)
    }

    /// Checks if a path represents a JSON file.
    ///
    /// # Arguments
    /// * `path` - The path to check.
    ///
    /// # Returns
    /// - `true` if the path is a file with a `.json` extension.
    /// - `false` otherwise.
    fn is_json_file(path: &Path) -> bool {
        path.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
    }

    /// Validates that a directory contains at least one JSON file.
    ///
    /// # Arguments
    /// * `path` - Path to the directory to validate.
    ///
    /// # Returns
    /// - `Ok(())` if the directory contains at least one JSON file.
    /// - `Err(ConfigFileError)` if no JSON files are found.
    pub fn validate_directory_has_configs(path: impl AsRef<Path>) -> Result<(), ConfigFileError> {
        let path = path.as_ref();

        if !path.is_dir() {
            return Err(ConfigFileError::InvalidFormat(format!(
                "Path '{}' is not a directory",
                path.display()
            )));
        }

        let entries = fs::read_dir(path).map_err(|e| {
            ConfigFileError::InvalidFormat(format!(
                "Failed to read directory '{}': {}",
                path.display(),
                e
            ))
        })?;

        let has_json_files = entries
            .filter_map(|entry| entry.ok())
            .any(|entry| Self::is_json_file(&entry.path()));

        if !has_json_files {
            return Err(ConfigFileError::InvalidFormat(format!(
                "Directory '{}' contains no JSON configuration files",
                path.display()
            )));
        }

        Ok(())
    }

    /// Loads networks from either a list or a directory path.
    ///
    /// This method handles the polymorphic loading behavior where the source
    /// can be either a direct list of networks or a path to a directory.
    ///
    /// # Arguments
    /// * `source` - Either a vector of networks or a path string.
    ///
    /// # Returns
    /// - `Ok(Vec<NetworkFileConfig>)` containing the loaded networks.
    /// - `Err(ConfigFileError)` if loading fails.
    pub fn load_from_source(
        source: NetworksSource,
    ) -> Result<Vec<NetworkFileConfig>, ConfigFileError> {
        match source {
            NetworksSource::List(networks) => Ok(networks),
            NetworksSource::Path(path_str) => Self::load_networks_from_directory(&path_str),
        }
    }
}

/// Represents the source of network configurations for deserialization.
#[derive(Debug, Clone)]
pub enum NetworksSource {
    List(Vec<NetworkFileConfig>),
    Path(String),
}

impl Default for NetworksSource {
    fn default() -> Self {
        NetworksSource::Path("./config/networks".to_string())
    }
}

impl<'de> serde::Deserialize<'de> for NetworksSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;
        use serde_json::Value;

        // First try to deserialize as a generic Value to determine the type
        let value = Value::deserialize(deserializer)?;

        match value {
            Value::Null => Ok(NetworksSource::default()),
            Value::String(s) => {
                if s.is_empty() {
                    Ok(NetworksSource::default())
                } else {
                    Ok(NetworksSource::Path(s))
                }
            }
            Value::Array(arr) => {
                let networks: Vec<NetworkFileConfig> = serde_json::from_value(Value::Array(arr))
                    .map_err(|e| {
                        de::Error::custom(format!("Failed to deserialize network array: {}", e))
                    })?;
                Ok(NetworksSource::List(networks))
            }
            _ => Err(de::Error::custom("Expected an array, string, or null")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_file::network::test_utils::*;
    use serde_json::json;
    use std::fs::{create_dir, File};
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn test_load_from_single_file() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_data = create_valid_evm_network_json();
        create_temp_file(&dir, "config1.json", &network_data.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());
        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].network_name(), "test-evm");
    }

    #[test]
    fn test_load_from_multiple_files() {
        let dir = tempdir().expect("Failed to create temp dir");
        let evm_data = create_valid_evm_network_json();
        let solana_data = create_valid_solana_network_json();

        create_temp_file(&dir, "evm.json", &evm_data.to_string());
        create_temp_file(&dir, "solana.json", &solana_data.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 2);

        let network_names: Vec<&str> = networks.iter().map(|n| n.network_name()).collect();
        assert!(network_names.contains(&"test-evm"));
        assert!(network_names.contains(&"test-solana"));
    }

    #[test]
    fn test_load_from_directory_multiple_networks_per_file() {
        let dir = tempdir().expect("Failed to create temp dir");

        let multi_network_data = json!({
            "networks": [
                {
                    "type": "evm",
                    "network": "evm-1",
                    "chain_id": 1,
                    "rpc_urls": ["http://localhost:8545"],
                    "symbol": "ETH"
                },
                {
                    "type": "evm",
                    "network": "evm-2",
                    "chain_id": 2,
                    "rpc_urls": ["http://localhost:8546"],
                    "symbol": "ETH2"
                }
            ]
        });

        create_temp_file(&dir, "multi.json", &multi_network_data.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 2);
        assert_eq!(networks[0].network_name(), "evm-1");
        assert_eq!(networks[1].network_name(), "evm-2");
    }

    #[test]
    fn test_load_from_directory_with_mixed_file_types() {
        let dir = tempdir().expect("Failed to create temp dir");

        let network_data = create_valid_evm_network_json();
        create_temp_file(&dir, "config.json", &network_data.to_string());
        create_temp_file(&dir, "readme.txt", "This is not a JSON file");
        create_temp_file(&dir, "config.yaml", "networks: []");

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].network_name(), "test-evm");
    }

    #[test]
    fn test_load_from_directory_with_subdirectories() {
        let dir = tempdir().expect("Failed to create temp dir");

        let network_data = create_valid_evm_network_json();
        create_temp_file(&dir, "config.json", &network_data.to_string());

        // Create a subdirectory - should be ignored
        let subdir_path = dir.path().join("subdir");
        create_dir(&subdir_path).expect("Failed to create subdirectory");

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
    }

    #[test]
    fn test_load_from_nonexistent_directory() {
        let dir = tempdir().expect("Failed to create temp dir");
        let non_existent_path = dir.path().join("non_existent");

        let result = NetworkFileLoader::load_networks_from_directory(&non_existent_path);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_file_instead_of_directory() {
        let dir = tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("not_a_dir.json");
        File::create(&file_path).expect("Failed to create file");

        let result = NetworkFileLoader::load_networks_from_directory(&file_path);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_with_no_json_files() {
        let dir = tempdir().expect("Failed to create temp dir");

        create_temp_file(&dir, "readme.txt", "This is not a JSON file");
        create_temp_file(&dir, "config.yaml", "networks: []");

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_with_invalid_json() {
        let dir = tempdir().expect("Failed to create temp dir");

        create_temp_file(
            &dir,
            "invalid.json",
            r#"{"networks": [{"type": "evm", "network": "broken""#,
        );

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_with_wrong_json_structure() {
        let dir = tempdir().expect("Failed to create temp dir");

        create_temp_file(&dir, "wrong.json", r#"{"foo": "bar"}"#);

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_with_empty_networks_array() {
        let dir = tempdir().expect("Failed to create temp dir");

        create_temp_file(&dir, "empty.json", r#"{"networks": []}"#);

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 0);
    }

    #[test]
    fn test_load_from_directory_with_invalid_network_structure() {
        let dir = tempdir().expect("Failed to create temp dir");

        let invalid_network = create_invalid_network_json();

        create_temp_file(&dir, "invalid_network.json", &invalid_network.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_partial_failure() {
        let dir = tempdir().expect("Failed to create temp dir");

        let valid_data = create_valid_evm_network_json();
        create_temp_file(&dir, "valid.json", &valid_data.to_string());
        create_temp_file(&dir, "invalid.json", r#"{"networks": [malformed"#);

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        // Should fail completely if any file fails
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_is_json_file() {
        let dir = tempdir().expect("Failed to create temp dir");

        let json_file = dir.path().join("config.json");
        File::create(&json_file).expect("Failed to create JSON file");
        assert!(NetworkFileLoader::is_json_file(&json_file));

        let txt_file = dir.path().join("config.txt");
        File::create(&txt_file).expect("Failed to create TXT file");
        assert!(!NetworkFileLoader::is_json_file(&txt_file));

        let json_upper_file = dir.path().join("config.JSON");
        File::create(&json_upper_file).expect("Failed to create JSON file");
        assert!(NetworkFileLoader::is_json_file(&json_upper_file));

        let no_extension_file = dir.path().join("config");
        File::create(&no_extension_file).expect("Failed to create file without extension");
        assert!(!NetworkFileLoader::is_json_file(&no_extension_file));

        // Test with directory
        let subdir = dir.path().join("subdir");
        create_dir(&subdir).expect("Failed to create subdirectory");
        assert!(!NetworkFileLoader::is_json_file(&subdir));
    }

    #[test]
    fn test_validate_directory_has_configs() {
        let dir = tempdir().expect("Failed to create temp dir");

        // Empty directory should fail validation
        let result = NetworkFileLoader::validate_directory_has_configs(dir.path());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));

        // Directory with non-JSON files should fail
        create_temp_file(&dir, "readme.txt", "Not JSON");
        let result = NetworkFileLoader::validate_directory_has_configs(dir.path());
        assert!(result.is_err());

        // Directory with JSON file should pass validation
        create_temp_file(&dir, "config.json", r#"{"networks": []}"#);
        let result = NetworkFileLoader::validate_directory_has_configs(dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_directory_has_configs_with_file_path() {
        let dir = tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("not_a_dir.json");
        File::create(&file_path).expect("Failed to create file");

        let result = NetworkFileLoader::validate_directory_has_configs(&file_path);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_source_with_list() {
        let networks = vec![]; // Empty list for simplicity
        let source = NetworksSource::List(networks.clone());

        let result = NetworkFileLoader::load_from_source(source);

        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }

    #[test]
    fn test_load_from_source_with_path() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_data = create_valid_evm_network_json();
        create_temp_file(&dir, "config.json", &network_data.to_string());

        let path_str = dir
            .path()
            .to_str()
            .expect("Path should be valid UTF-8")
            .to_string();
        let source = NetworksSource::Path(path_str);

        let result = NetworkFileLoader::load_from_source(source);

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].network_name(), "test-evm");
    }

    #[test]
    fn test_load_from_source_with_invalid_path() {
        let source = NetworksSource::Path("/non/existent/path".to_string());

        let result = NetworkFileLoader::load_from_source(source);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_with_unicode_filenames() {
        let dir = tempdir().expect("Failed to create temp dir");

        let network_data = create_valid_evm_network_json();
        create_temp_file(&dir, "配置.json", &network_data.to_string());
        create_temp_file(&dir, "конфиг.json", &network_data.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 2);
    }

    #[test]
    fn test_load_from_directory_with_unicode_content() {
        let dir = tempdir().expect("Failed to create temp dir");

        let unicode_network = json!({
            "networks": [
                {
                    "type": "evm",
                    "network": "测试网络",
                    "chain_id": 1,
                    "rpc_urls": ["http://localhost:8545"],
                    "symbol": "ETH"
                }
            ]
        });

        create_temp_file(&dir, "unicode.json", &unicode_network.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].network_name(), "测试网络");
    }

    #[test]
    fn test_load_from_directory_with_json_extension_but_invalid_content() {
        let dir = tempdir().expect("Failed to create temp dir");

        create_temp_file(&dir, "fake.json", "This is not JSON content at all!");

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_with_large_number_of_files() {
        let dir = tempdir().expect("Failed to create temp dir");

        // Create 100 JSON files
        for i in 0..100 {
            let network_data = json!({
                "networks": [
                    {
                        "type": "evm",
                        "network": format!("test-network-{}", i),
                        "chain_id": i + 1,
                        "rpc_urls": [format!("http://localhost:{}", 8545 + i)],
                        "symbol": "ETH"
                    }
                ]
            });
            create_temp_file(
                &dir,
                &format!("config_{}.json", i),
                &network_data.to_string(),
            );
        }

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 100);
    }

    #[test]
    fn test_networks_source_deserialization() {
        // Test deserializing as list
        let list_json = r#"[{"type": "evm", "network": "test", "chain_id": 1, "rpc_urls": ["http://localhost:8545"], "symbol": "ETH", "required_confirmations": 1}]"#;
        let source: NetworksSource =
            serde_json::from_str(list_json).expect("Failed to deserialize list");

        match source {
            NetworksSource::List(networks) => {
                assert_eq!(networks.len(), 1);
                assert_eq!(networks[0].network_name(), "test");
            }
            NetworksSource::Path(_) => panic!("Expected List variant"),
        }

        // Test deserializing as path
        let path_json = r#""/path/to/configs""#;
        let source: NetworksSource =
            serde_json::from_str(path_json).expect("Failed to deserialize path");

        match source {
            NetworksSource::Path(path) => {
                assert_eq!(path, "/path/to/configs");
            }
            NetworksSource::List(_) => panic!("Expected Path variant"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_load_from_directory_with_permission_issues() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_data = create_valid_evm_network_json();
        create_temp_file(&dir, "config.json", &network_data.to_string());

        // Remove read permissions from the directory
        let mut perms = std::fs::metadata(dir.path())
            .expect("Failed to get metadata")
            .permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(dir.path(), perms).expect("Failed to set permissions");

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        // Restore permissions for cleanup
        let mut perms = std::fs::metadata(dir.path())
            .expect("Failed to get metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dir.path(), perms).expect("Failed to restore permissions");

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_validate_directory_has_configs_with_nonexistent_directory() {
        let dir = tempdir().expect("Failed to create temp dir");
        let non_existent_path = dir.path().join("non_existent");

        let result = NetworkFileLoader::validate_directory_has_configs(&non_existent_path);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_is_json_file_with_nonexistent_file() {
        let dir = tempdir().expect("Failed to create temp dir");
        let non_existent_file = dir.path().join("nonexistent.json");

        // Should return false for nonexistent files since is_file() returns false
        assert!(!NetworkFileLoader::is_json_file(&non_existent_file));
    }

    #[cfg(unix)]
    #[test]
    fn test_load_from_directory_with_file_permission_issues() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_data = create_valid_evm_network_json();
        create_temp_file(&dir, "config.json", &network_data.to_string());

        // Remove read permissions from the file (not the directory)
        let file_path = dir.path().join("config.json");
        let mut perms = std::fs::metadata(&file_path)
            .expect("Failed to get file metadata")
            .permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&file_path, perms).expect("Failed to set file permissions");

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_empty_directory() {
        let dir = tempdir().expect("Failed to create temp dir");

        // Empty directory (no files at all) should fail validation
        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_with_json_containing_extra_fields() {
        let dir = tempdir().expect("Failed to create temp dir");

        // JSON with extra fields in the network config should fail due to deny_unknown_fields
        let network_with_extra_fields = json!({
            "networks": [
                {
                    "type": "evm",
                    "network": "test-with-extra",
                    "chain_id": 1,
                    "rpc_urls": ["http://localhost:8545"],
                    "symbol": "ETH",
                    "extra_field": "should_cause_error"
                }
            ]
        });

        create_temp_file(
            &dir,
            "extra_fields.json",
            &network_with_extra_fields.to_string(),
        );

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        // Should fail because EVM networks have deny_unknown_fields
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_load_from_directory_with_json_containing_extra_top_level_fields() {
        let dir = tempdir().expect("Failed to create temp dir");

        // JSON with extra fields at the top level should be ignored
        let network_with_extra_top_level = json!({
            "networks": [
                {
                    "type": "evm",
                    "network": "test-with-extra-top",
                    "chain_id": 1,
                    "rpc_urls": ["http://localhost:8545"],
                    "symbol": "ETH",
                    "required_confirmations": 1
                }
            ],
            "extra_top_level": "ignored",
            "another_extra": 42
        });

        create_temp_file(
            &dir,
            "extra_top_level.json",
            &network_with_extra_top_level.to_string(),
        );

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        // Should succeed because extra top-level fields are ignored by DirectoryNetworkList
        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].network_name(), "test-with-extra-top");
    }

    #[test]
    fn test_load_from_directory_with_very_large_json() {
        let dir = tempdir().expect("Failed to create temp dir");

        let mut networks_array = Vec::new();
        for i in 0..1000 {
            networks_array.push(json!({
                "type": "evm",
                "network": format!("large-test-{}", i),
                "chain_id": i + 1,
                "rpc_urls": [format!("http://localhost:{}", 8545 + i)],
                "symbol": "ETH"
            }));
        }

        let large_json = json!({
            "networks": networks_array
        });

        create_temp_file(&dir, "large.json", &large_json.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1000);
    }

    #[test]
    fn test_load_from_directory_with_deeply_nested_json() {
        let dir = tempdir().expect("Failed to create temp dir");

        let complex_network = json!({
            "networks": [
                {
                    "type": "evm",
                    "network": "complex-nested",
                    "chain_id": 1,
                    "rpc_urls": ["http://localhost:8545"],
                    "symbol": "ETH",
                    "tags": ["mainnet", "production", "high-security"]
                }
            ]
        });

        create_temp_file(&dir, "complex.json", &complex_network.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].network_name(), "complex-nested");
    }

    #[test]
    fn test_load_from_directory_with_null_values() {
        let dir = tempdir().expect("Failed to create temp dir");

        // Test JSON with null values in optional fields
        let network_with_nulls = json!({
            "networks": [
                {
                    "type": "evm",
                    "network": "test-nulls",
                    "chain_id": 1,
                    "rpc_urls": ["http://localhost:8545"],
                    "symbol": "ETH",
                    "tags": null,
                    "features": null
                }
            ]
        });

        create_temp_file(&dir, "nulls.json", &network_with_nulls.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].network_name(), "test-nulls");
    }

    #[test]
    fn test_load_from_directory_with_special_characters_in_content() {
        let dir = tempdir().expect("Failed to create temp dir");

        let special_chars_network = json!({
            "networks": [
                {
                    "type": "evm",
                    "network": "test-special-chars-\n\t\r\"\\",
                    "chain_id": 1,
                    "rpc_urls": ["http://localhost:8545"],
                    "symbol": "ETH"
                }
            ]
        });

        create_temp_file(&dir, "special.json", &special_chars_network.to_string());

        let result = NetworkFileLoader::load_networks_from_directory(dir.path());

        assert!(result.is_ok());
        let networks = result.unwrap();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].network_name(), "test-special-chars-\n\t\r\"\\");
    }

    #[cfg(unix)]
    #[test]
    fn test_load_from_directory_with_symbolic_links() {
        let dir = tempdir().expect("Failed to create temp dir");
        let network_data = create_valid_evm_network_json();

        create_temp_file(&dir, "regular.json", &network_data.to_string());

        // Create a symbolic link to the JSON file
        let regular_path = dir.path().join("regular.json");
        let symlink_path = dir.path().join("symlink.json");

        if std::os::unix::fs::symlink(&regular_path, &symlink_path).is_ok() {
            let result = NetworkFileLoader::load_networks_from_directory(dir.path());

            assert!(result.is_ok());
            let networks = result.unwrap();
            // Should load both the regular file and the symlink (2 networks total)
            assert_eq!(networks.len(), 2);
        }
    }

    #[test]
    fn test_load_from_source_with_list_containing_networks() {
        // Test load_from_source with actual network data in the list
        let evm_network_json = create_valid_evm_network_json();
        let networks: Vec<NetworkFileConfig> =
            serde_json::from_value(evm_network_json["networks"].clone())
                .expect("Failed to deserialize networks");

        let source = NetworksSource::List(networks.clone());
        let result = NetworkFileLoader::load_from_source(source);

        assert!(result.is_ok());
        let loaded_networks = result.unwrap();
        assert_eq!(loaded_networks.len(), 1);
        assert_eq!(loaded_networks[0].network_name(), "test-evm");
    }

    #[test]
    fn test_directory_network_list_deserialization() {
        // Test DirectoryNetworkList deserialization directly
        let json_str = r#"{"networks": []}"#;
        let result: Result<DirectoryNetworkList, _> = serde_json::from_str(json_str);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().networks.len(), 0);

        // Test with invalid structure
        let invalid_json = r#"{"not_networks": []}"#;
        let result: Result<DirectoryNetworkList, _> = serde_json::from_str(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_networks_source_clone_and_debug() {
        // Test that NetworksSource implements Clone and Debug properly
        let source = NetworksSource::Path("/test/path".to_string());
        let cloned = source.clone();

        match (source, cloned) {
            (NetworksSource::Path(path1), NetworksSource::Path(path2)) => {
                assert_eq!(path1, path2);
            }
            _ => panic!("Clone didn't preserve variant"),
        }

        // Test Debug formatting
        let source = NetworksSource::List(vec![]);
        let debug_str = format!("{:?}", source);
        assert!(debug_str.contains("List"));
    }

    #[test]
    fn test_is_json_file_edge_cases() {
        let dir = tempdir().expect("Failed to create temp dir");

        // Test file with .json in the middle of the name but different extension
        let misleading_file = dir.path().join("config.json.backup");
        File::create(&misleading_file).expect("Failed to create misleading file");
        assert!(!NetworkFileLoader::is_json_file(&misleading_file));

        // Test file with multiple dots
        let multi_dot_file = dir.path().join("config.test.json");
        File::create(&multi_dot_file).expect("Failed to create multi-dot file");
        assert!(NetworkFileLoader::is_json_file(&multi_dot_file));

        // Test file with mixed case in middle
        let mixed_case_file = dir.path().join("config.Json");
        File::create(&mixed_case_file).expect("Failed to create mixed case file");
        assert!(NetworkFileLoader::is_json_file(&mixed_case_file));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_directory_has_configs_with_permission_issues() {
        let dir = tempdir().expect("Failed to create temp dir");
        create_temp_file(&dir, "config.json", r#"{"networks": []}"#);

        // Remove read permissions from the directory
        let mut perms = std::fs::metadata(dir.path())
            .expect("Failed to get metadata")
            .permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(dir.path(), perms).expect("Failed to set permissions");

        let result = NetworkFileLoader::validate_directory_has_configs(dir.path());

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigFileError::InvalidFormat(_)
        ));
    }

    #[test]
    fn test_networks_source_default() {
        let default_source = NetworksSource::default();
        match default_source {
            NetworksSource::Path(path) => {
                assert_eq!(path, "./config/networks");
            }
            _ => panic!("Default should be a Path variant"),
        }
    }

    #[test]
    fn test_networks_source_deserialize_null() {
        let json = r#"null"#;
        let result: Result<NetworksSource, _> = serde_json::from_str(json);
        assert!(result.is_ok());

        match result.unwrap() {
            NetworksSource::Path(path) => {
                assert_eq!(path, "./config/networks");
            }
            _ => panic!("Expected default Path variant"),
        }
    }

    #[test]
    fn test_networks_source_deserialize_empty_string() {
        let json = r#""""#;
        let result: Result<NetworksSource, _> = serde_json::from_str(json);
        assert!(result.is_ok());

        match result.unwrap() {
            NetworksSource::Path(path) => {
                assert_eq!(path, "./config/networks");
            }
            _ => panic!("Expected default Path variant"),
        }
    }

    #[test]
    fn test_networks_source_deserialize_valid_path() {
        let json = r#""/custom/path""#;
        let result: Result<NetworksSource, _> = serde_json::from_str(json);
        assert!(result.is_ok());

        match result.unwrap() {
            NetworksSource::Path(path) => {
                assert_eq!(path, "/custom/path");
            }
            _ => panic!("Expected Path variant"),
        }
    }

    #[test]
    fn test_networks_source_deserialize_array() {
        let json = r#"[{"type": "evm", "network": "test", "chain_id": 1, "rpc_urls": ["http://localhost:8545"], "symbol": "ETH", "required_confirmations": 1}]"#;
        let result: Result<NetworksSource, _> = serde_json::from_str(json);
        assert!(result.is_ok());

        match result.unwrap() {
            NetworksSource::List(networks) => {
                assert_eq!(networks.len(), 1);
                assert_eq!(networks[0].network_name(), "test");
            }
            _ => panic!("Expected List variant"),
        }
    }

    #[test]
    fn test_networks_source_deserialize_invalid_type() {
        let json = r#"42"#;
        let result: Result<NetworksSource, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
