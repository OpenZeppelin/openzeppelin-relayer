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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    #[tokio::test]
    async fn test_execute_typescript() {
        let dir = Path::new("tests/utils/plugins/mock_repo");

        let script_path = format!("{}/test_works.ts", dir.display());
        let socket_path = format!("{}/test_works.sock", dir.display());
        let content = "console.log('test');";
        fs::write(script_path.clone(), content).unwrap();

        let result =
            ScriptExecutor::execute_typescript(script_path.clone(), socket_path.clone()).await;

        // These are just in case, there should be an automatic deletion
        let _ = fs::remove_file(script_path.clone());
        let _ = fs::remove_file(socket_path.clone());

        assert!(result.is_ok());
        assert_eq!(result.unwrap().output, "test\n");
    }

    #[tokio::test]
    async fn test_execute_typescript_error() {
        let dir = Path::new("tests/utils/plugins/mock_repo");
        let script_path = dir.join("test_error.ts");
        let socket_path = dir.join("test_error.sock");
        let content = "console.logger('test');";
        fs::write(script_path.clone(), content).unwrap();

        let socket_path = socket_path.to_str().unwrap().to_string();
        let script_path = script_path.to_str().unwrap().to_string();

        let result =
            ScriptExecutor::execute_typescript(script_path.clone(), socket_path.clone()).await;

        // These are just in case, there should be an automatic deletion
        let _ = fs::remove_file(script_path.clone());
        let _ = fs::remove_file(socket_path.clone());

        assert!(result.is_ok());
        assert!(result
            .unwrap()
            .error
            .contains("Property 'logger' does not exist"));
    }
}
