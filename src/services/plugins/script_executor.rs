use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::Command;

use super::PluginError;

#[derive(Debug, Serialize, Deserialize)]
pub struct ScriptResult {
    pub output: String,
    pub error: String,
}

pub struct ScriptExecutor;

impl ScriptExecutor {
    pub async fn execute_typescript(
        script_path: String,
        socket_path: String,
    ) -> Result<ScriptResult, PluginError> {
        let output = Command::new("ts-node")
            .arg(script_path)
            .arg(socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| PluginError::SocketError(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        Ok(ScriptResult {
            output: stdout.to_string(),
            error: stderr.to_string(),
        })
    }
}
