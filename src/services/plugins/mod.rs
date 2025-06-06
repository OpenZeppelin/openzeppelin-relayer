//! Plugins service module for handling plugins execution and interaction with relayer
struct PluginService {}

impl PluginService {
    pub fn call_plugin(&self, _plugin_id: &str) -> Result<String, String> {
        unimplemented!()
    }
}

impl Default for PluginService {
    fn default() -> Self {
        Self {}
    }
}
