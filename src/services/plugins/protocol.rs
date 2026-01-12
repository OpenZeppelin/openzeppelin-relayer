//! Protocol types for pool server communication.
//!
//! Defines the JSON-line protocol messages exchanged between
//! the Rust pool executor and Node.js pool server via Unix socket.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{LogEntry, LogLevel};

/// Request messages sent to the pool server
#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PoolRequest {
    Execute {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "pluginId")]
        plugin_id: String,
        #[serde(rename = "compiledCode", skip_serializing_if = "Option::is_none")]
        compiled_code: Option<String>,
        #[serde(rename = "pluginPath", skip_serializing_if = "Option::is_none")]
        plugin_path: Option<String>,
        params: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, Vec<String>>>,
        #[serde(rename = "socketPath")]
        socket_path: String,
        #[serde(rename = "httpRequestId", skip_serializing_if = "Option::is_none")]
        http_request_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        route: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        config: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        method: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<serde_json::Value>,
    },
    Precompile {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "pluginId")]
        plugin_id: String,
        #[serde(rename = "pluginPath", skip_serializing_if = "Option::is_none")]
        plugin_path: Option<String>,
        #[serde(rename = "sourceCode", skip_serializing_if = "Option::is_none")]
        source_code: Option<String>,
    },
    Cache {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "pluginId")]
        plugin_id: String,
        #[serde(rename = "compiledCode")]
        compiled_code: String,
    },
    Invalidate {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "pluginId")]
        plugin_id: String,
    },
    Stats {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Health {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    Shutdown {
        #[serde(rename = "taskId")]
        task_id: String,
    },
}

/// Response from the pool server
#[derive(Deserialize, Debug)]
pub struct PoolResponse {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<PoolError>,
    pub logs: Option<Vec<PoolLogEntry>>,
}

/// Error details from the pool server
#[derive(Deserialize, Debug)]
pub struct PoolError {
    pub message: String,
    pub code: Option<String>,
    pub status: Option<u16>,
    pub details: Option<serde_json::Value>,
}

/// Log entry from plugin execution
#[derive(Deserialize, Debug)]
pub struct PoolLogEntry {
    pub level: String,
    pub message: String,
}

impl From<PoolLogEntry> for LogEntry {
    fn from(entry: PoolLogEntry) -> Self {
        let level = match entry.level.as_str() {
            "error" => LogLevel::Error,
            "warn" => LogLevel::Warn,
            "info" => LogLevel::Info,
            "debug" => LogLevel::Debug,
            "result" => LogLevel::Result,
            _ => LogLevel::Log,
        };
        LogEntry {
            level,
            message: entry.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_request_execute_serialization() {
        let request = PoolRequest::Execute {
            task_id: "test-123".to_string(),
            plugin_id: "my-plugin".to_string(),
            compiled_code: Some("console.log('hello')".to_string()),
            plugin_path: None,
            params: serde_json::json!({"key": "value"}),
            headers: None,
            socket_path: "/tmp/test.sock".to_string(),
            http_request_id: Some("req-456".to_string()),
            timeout: Some(30000),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"type\":\"execute\""));
        assert!(json.contains("\"taskId\":\"test-123\""));
        assert!(json.contains("\"pluginId\":\"my-plugin\""));
    }

    #[test]
    fn test_pool_request_precompile_serialization() {
        let request = PoolRequest::Precompile {
            task_id: "precompile-123".to_string(),
            plugin_id: "test-plugin".to_string(),
            plugin_path: Some("/plugins/test.ts".to_string()),
            source_code: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"type\":\"precompile\""));
        assert!(json.contains("\"taskId\":\"precompile-123\""));
        assert!(json.contains("\"pluginPath\":\"/plugins/test.ts\""));
    }

    #[test]
    fn test_pool_request_cache_serialization() {
        let request = PoolRequest::Cache {
            task_id: "cache-123".to_string(),
            plugin_id: "test-plugin".to_string(),
            compiled_code: "compiled code here".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"type\":\"cache\""));
        assert!(json.contains("\"compiledCode\":\"compiled code here\""));
    }

    #[test]
    fn test_pool_request_invalidate_serialization() {
        let request = PoolRequest::Invalidate {
            task_id: "inv-123".to_string(),
            plugin_id: "test-plugin".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"type\":\"invalidate\""));
    }

    #[test]
    fn test_pool_request_stats_serialization() {
        let request = PoolRequest::Stats {
            task_id: "stats-123".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"type\":\"stats\""));
    }

    #[test]
    fn test_pool_request_health_serialization() {
        let request = PoolRequest::Health {
            task_id: "health-123".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"type\":\"health\""));
    }

    #[test]
    fn test_pool_request_shutdown_serialization() {
        let request = PoolRequest::Shutdown {
            task_id: "shutdown-123".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"type\":\"shutdown\""));
    }

    #[test]
    fn test_pool_response_success_deserialization() {
        let json = r#"{
            "taskId": "test-123",
            "success": true,
            "result": {"data": "hello"},
            "logs": [{"level": "info", "message": "test log"}]
        }"#;

        let response: PoolResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.task_id, "test-123");
        assert!(response.success);
        assert!(response.error.is_none());
        assert!(response
            .logs
            .as_ref()
            .map(|l| !l.is_empty())
            .unwrap_or(false));
    }

    #[test]
    fn test_pool_response_error_deserialization() {
        let json = r#"{
            "taskId": "test-123",
            "success": false,
            "error": {
                "message": "Plugin failed",
                "code": "EXEC_ERROR",
                "status": 500
            },
            "logs": []
        }"#;

        let response: PoolResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.task_id, "test-123");
        assert!(!response.success);
        assert!(response.error.is_some());
        let err = response.error.unwrap();
        assert_eq!(err.message, "Plugin failed");
        assert_eq!(err.code, Some("EXEC_ERROR".to_string()));
        assert_eq!(err.status, Some(500));
    }

    #[test]
    fn test_pool_log_entry_conversion() {
        let pool_entry = PoolLogEntry {
            level: "error".to_string(),
            message: "test error".to_string(),
        };

        let log_entry: LogEntry = pool_entry.into();
        assert!(matches!(log_entry.level, LogLevel::Error));
        assert_eq!(log_entry.message, "test error");
    }

    #[test]
    fn test_pool_log_entry_level_conversion() {
        let levels = vec![
            ("log", LogLevel::Log),
            ("info", LogLevel::Info),
            ("warn", LogLevel::Warn),
            ("error", LogLevel::Error),
            ("debug", LogLevel::Debug),
            ("unknown", LogLevel::Log),
        ];

        for (input, expected) in levels {
            let pool_entry = PoolLogEntry {
                level: input.to_string(),
                message: "test".to_string(),
            };
            let log_entry: LogEntry = pool_entry.into();
            assert!(
                matches!(log_entry.level, ref e if std::mem::discriminant(e) == std::mem::discriminant(&expected)),
                "Expected {:?} for input '{}', got {:?}",
                expected,
                input,
                log_entry.level
            );
        }
    }
}
