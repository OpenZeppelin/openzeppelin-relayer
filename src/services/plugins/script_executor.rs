//! This module is responsible for executing a typescript script.
//!
//! 1. Checks if `ts-node` is installed.
//! 2. Executes the script using the `ts-node` command.
//! 3. Returns the output and errors of the script.
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::Command;
use utoipa::ToSchema;

use super::PluginError;

#[derive(Serialize, Deserialize, Debug, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Log,
    Info,
    Error,
    Warn,
    Debug,
    Result,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, ToSchema)]
pub struct ScriptResult {
    pub logs: Vec<LogEntry>,
    pub error: String,
    pub trace: Vec<serde_json::Value>,
    pub return_value: String,
}

pub struct ScriptExecutor;

impl ScriptExecutor {
    pub async fn execute_typescript(
        script_path: String,
        socket_path: String,
        script_params: String,
    ) -> Result<ScriptResult, PluginError> {
        if Command::new("ts-node")
            .arg("--version")
            .output()
            .await
            .is_err()
        {
            return Err(PluginError::SocketError(
                "ts-node is not installed or not in PATH. Please install it with: npm install -g ts-node".to_string()
            ));
        }

        let output = Command::new("ts-node")
            .arg(script_path)
            .arg(socket_path)
            .arg(script_params)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| PluginError::SocketError(format!("Failed to execute script: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let (logs, return_value) =
            Self::parse_logs(stdout.lines().map(|l| l.to_string()).collect())?;

        Ok(ScriptResult {
            logs,
            return_value,
            error: stderr.to_string(),
            trace: Vec::new(),
        })
    }

    fn parse_logs(logs: Vec<String>) -> Result<(Vec<LogEntry>, String), PluginError> {
        let mut result = Vec::new();
        let mut return_value = String::new();

        for log in logs {
            let log: LogEntry = serde_json::from_str(&log).map_err(|e| {
                PluginError::PluginExecutionError(format!("Failed to parse log: {}", e))
            })?;

            if log.level == LogLevel::Result {
                return_value = log.message;
            } else {
                result.push(log);
            }
        }

        Ok((result, return_value))
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

        let content = r#"
            console.log(JSON.stringify({ level: 'log', message: 'test' }));
            console.log(JSON.stringify({ level: 'info', message: 'test-info' }));
            console.log(JSON.stringify({ level: 'result', message: 'test-result' }));
        "#;
        fs::write(script_path.clone(), content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let result = ScriptExecutor::execute_typescript(
            script_path.display().to_string(),
            socket_path.display().to_string(),
            "{}".to_string(),
        )
        .await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.logs[0].level, LogLevel::Log);
        assert_eq!(result.logs[0].message, "test");
        assert_eq!(result.logs[1].level, LogLevel::Info);
        assert_eq!(result.logs[1].message, "test-info");
        assert_eq!(result.return_value, "test-result");
    }

    #[tokio::test]
    async fn test_execute_typescript_error() {
        let temp_dir = tempdir().unwrap();
        let ts_config = temp_dir.path().join("tsconfig.json");
        let script_path = temp_dir.path().join("test_execute_typescript_error.ts");
        let socket_path = temp_dir.path().join("test_execute_typescript_error.sock");

        let content = "console.logger('test');";
        fs::write(script_path.clone(), content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let result = ScriptExecutor::execute_typescript(
            script_path.display().to_string(),
            socket_path.display().to_string(),
            "{}".to_string(),
        )
        .await;

        assert!(result.is_ok());

        let result = result.unwrap();
        assert!(result.error.contains("logger"));
    }

    #[tokio::test]
    async fn test_parse_logs_error() {
        let temp_dir = tempdir().unwrap();
        let ts_config = temp_dir.path().join("tsconfig.json");
        let script_path = temp_dir.path().join("test_execute_typescript.ts");
        let socket_path = temp_dir.path().join("test_execute_typescript.sock");

        let invalid_content = r#"
            console.log({ level: 'log', message: 'test' });
            console.log({ level: 'info', message: 'test-info' });
            console.log({ level: 'result', message: 'test-result' });
        "#;
        fs::write(script_path.clone(), invalid_content).unwrap();
        fs::write(ts_config.clone(), TS_CONFIG.as_bytes()).unwrap();

        let result = ScriptExecutor::execute_typescript(
            script_path.display().to_string(),
            socket_path.display().to_string(),
            "{}".to_string(),
        )
        .await;

        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("Failed to parse log"));
    }
}
