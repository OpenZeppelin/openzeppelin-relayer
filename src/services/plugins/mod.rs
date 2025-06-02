//! Plugins service module for handling plugins execution and interaction with relayer

use std::collections::HashMap;

struct PluginService {
    plugins: HashMap<String, Box<dyn PluginTrait>>,
}

impl PluginService {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    pub fn load_all(&mut self) -> Result<(), String> {
        // TODO: load all plugins from config
        Ok(())
    }
}

impl Default for PluginService {
    fn default() -> Self {
        Self::new()
    }
}
