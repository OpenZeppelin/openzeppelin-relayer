use std::collections::HashSet;

use crate::config::ConfigFileError;
use serde::{Deserialize, Serialize};

// TODO: in case we want to support other languages and add
// more flexibility to the plugins folder, we should
// move this to a config file
const PLUGIN_FILE_TYPE: &str = ".ts";
const PLUGIN_LANG: &str = "typescript";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PluginFileConfig {
    pub id: String,
    pub path: String,
    pub timeout: Option<u64>,
}

pub struct PluginsFileConfig {
    pub plugins: Vec<PluginFileConfig>,
}

impl PluginsFileConfig {
    pub fn new(plugins: Vec<PluginFileConfig>) -> Self {
        Self { plugins }
    }

    pub fn validate(&self) -> Result<(), ConfigFileError> {
        let mut ids = HashSet::new();
        for plugin in &self.plugins {
            if !ids.insert(plugin.id.clone()) {
                return Err(ConfigFileError::DuplicateId(plugin.id.clone()));
            }

            if plugin.id.is_empty() {
                return Err(ConfigFileError::MissingField("id".into()));
            }

            if plugin.path.is_empty() {
                return Err(ConfigFileError::MissingField("path".into()));
            }

            // validate timeout
            if let Some(timeout) = plugin.timeout {
                if timeout == 0 {
                    return Err(ConfigFileError::InvalidTimeout(timeout));
                }
            }

            if !plugin.path.ends_with(PLUGIN_FILE_TYPE) {
                return Err(ConfigFileError::InvalidFormat(format!(
                    "Plugin path must be a {} file (ends with '{}')",
                    PLUGIN_LANG, PLUGIN_FILE_TYPE
                )));
            }
        }

        Ok(())
    }
}
