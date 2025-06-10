//! Plugins service module for handling plugins execution and interaction with relayer
use crate::models::PluginCallRequest;

#[derive(Default)]
pub struct PluginService {}

impl PluginService {
    pub fn call_plugin(
        &self,
        _path: &str,
        _plugin_call_request: PluginCallRequest,
    ) -> Result<String, String> {
        unimplemented!()
    }
}
