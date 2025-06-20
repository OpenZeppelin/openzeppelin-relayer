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

    use tempfile::tempdir;

    use super::*;

    static TS_CONFIG: &str = r#"
    {
        "compilerOptions": {
          "target": "es2016",
          "module": "commonjs",
          "esModuleInterop": true,
          "forceConsistentCasingInFileNames": true,
          "strict": true,
          "skipLibCheck": true
        }
      }
"#;

    #[tokio::test]
    async fn test_execute_typescript() {
        let temp_dir = tempdir().unwrap();
        let ts_config = temp_dir.path().join("tsconfig.json");
        let script_path = temp_dir.path().join("test_execute_typescript.ts");
        let socket_path = temp_dir.path().join("test_execute_typescript.sock");

        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        let content = "console.log('test');";
        fs::write(script_path.clone(), content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let result = ScriptExecutor::execute_typescript(
            script_path.display().to_string(),
            socket_path.display().to_string(),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().output, "test\n");
    }

    #[tokio::test]
    async fn test_execute_typescript_error() {
        let temp_dir = tempdir().unwrap();
        let ts_config = temp_dir.path().join("tsconfig.json");
        let script_path = temp_dir.path().join("test_execute_typescript_error.ts");
        let socket_path = temp_dir.path().join("test_execute_typescript_error.sock");

        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        let content = "console.logger('test');";
        fs::write(script_path.clone(), content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let result = ScriptExecutor::execute_typescript(
            script_path.display().to_string(),
            socket_path.display().to_string(),
        )
        .await;

        assert!(result.is_ok());

        let result = result.unwrap();
        assert!(result.error.contains("logger"));
    }
}
