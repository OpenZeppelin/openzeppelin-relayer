/// Core JSON-RPC 2.0 types and implementations.
///
/// This module contains the fundamental data structures for JSON-RPC 2.0 requests and responses,
/// including proper ID handling according to the JSON-RPC specification.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Represents a JSON-RPC 2.0 ID value.
/// According to the spec, the ID can be a String or Number.
/// When used in `Option<JsonRpcId>`: Some(id) = actual ID, None = explicit null.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// String identifier
    String(String),
    /// Numeric identifier (should not contain fractional parts per spec)
    Number(i64),
}

impl JsonRpcId {
    /// Creates a JsonRpcId from a number
    pub fn number(n: i64) -> Self {
        JsonRpcId::Number(n)
    }

    /// Creates a JsonRpcId from a string
    pub fn string<S: Into<String>>(s: S) -> Self {
        JsonRpcId::String(s.into())
    }

    /// Attempts to extract a numeric value from the ID
    pub fn as_number(&self) -> Option<i64> {
        match self {
            JsonRpcId::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Attempts to extract a string value from the ID
    pub fn as_string(&self) -> Option<&str> {
        match self {
            JsonRpcId::String(s) => Some(s),
            _ => None,
        }
    }
}

impl From<i64> for JsonRpcId {
    fn from(n: i64) -> Self {
        JsonRpcId::Number(n)
    }
}

impl From<u64> for JsonRpcId {
    fn from(n: u64) -> Self {
        JsonRpcId::Number(n as i64)
    }
}

impl From<String> for JsonRpcId {
    fn from(s: String) -> Self {
        JsonRpcId::String(s)
    }
}

impl From<&str> for JsonRpcId {
    fn from(s: &str) -> Self {
        JsonRpcId::String(s.to_string())
    }
}

/// JSON-RPC 2.0 Request structure.
///
/// Represents a JSON-RPC request with proper ID handling:
/// - `Some(JsonRpcId)` = request with ID
/// - `None` = explicit null ID or notification
#[derive(Serialize, Deserialize, ToSchema)]
pub struct JsonRpcRequest<T> {
    pub jsonrpc: String,
    #[serde(flatten)]
    pub params: T,
    #[serde(default)]
    pub id: Option<JsonRpcId>,
}

/// JSON-RPC 2.0 Response structure.
///
/// Represents a JSON-RPC response that can contain either a result or an error.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<JsonRpcError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub id: Option<JsonRpcId>,
}

impl<T> JsonRpcResponse<T> {
    /// Creates a new successful JSON-RPC response with the given result and id.
    ///
    /// # Arguments
    /// * `id` - The request identifier (can be None for null)
    /// * `result` - The result value to include in the response
    ///
    /// # Returns
    /// A new JsonRpcResponse with the specified result
    pub fn result(id: Option<JsonRpcId>, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    /// Creates a new error JSON-RPC response.
    ///
    /// # Arguments
    /// * `code` - The error code
    /// * `message` - The error message
    /// * `description` - The error description
    ///
    /// # Returns
    /// A new JsonRpcResponse with the specified error
    pub fn error(code: i32, message: &str, description: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                description: description.to_string(),
            }),
            id: None,
        }
    }
}

/// JSON-RPC 2.0 Error structure.
///
/// Represents an error in a JSON-RPC response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_json_rpc_id_serialization() {
        // Test Number variant
        let id_number = JsonRpcId::Number(42);
        let serialized = serde_json::to_value(&id_number).unwrap();
        assert_eq!(serialized, json!(42));

        // Test String variant
        let id_string = JsonRpcId::String("test-id".to_string());
        let serialized = serde_json::to_value(&id_string).unwrap();
        assert_eq!(serialized, json!("test-id"));
    }

    #[test]
    fn test_json_rpc_id_deserialization() {
        // Test Number deserialization
        let id: JsonRpcId = serde_json::from_value(json!(123)).unwrap();
        assert_eq!(id, JsonRpcId::Number(123));

        // Test String deserialization
        let id: JsonRpcId = serde_json::from_value(json!("example-id")).unwrap();
        assert_eq!(id, JsonRpcId::String("example-id".to_string()));
    }

    #[test]
    fn test_json_rpc_id_helper_methods() {
        let number_id = JsonRpcId::Number(100);
        assert_eq!(number_id.as_number(), Some(100));
        assert_eq!(number_id.as_string(), None);

        let string_id = JsonRpcId::String("hello".to_string());
        assert_eq!(string_id.as_number(), None);
        assert_eq!(string_id.as_string(), Some("hello"));
    }

    #[test]
    fn test_json_rpc_id_from_implementations() {
        // Test From<i64>
        let id: JsonRpcId = 42i64.into();
        assert_eq!(id, JsonRpcId::Number(42));

        // Test From<u64>
        let id: JsonRpcId = 42u64.into();
        assert_eq!(id, JsonRpcId::Number(42));

        // Test From<String>
        let id: JsonRpcId = "test".to_string().into();
        assert_eq!(id, JsonRpcId::String("test".to_string()));

        // Test From<&str>
        let id: JsonRpcId = "test".into();
        assert_eq!(id, JsonRpcId::String("test".to_string()));
    }

    #[test]
    fn test_json_rpc_request_with_different_id_types() {
        // Test with number ID
        let request_with_number = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: json!({"method": "test"}),
            id: Some(JsonRpcId::Number(1)),
        };
        let serialized = serde_json::to_value(&request_with_number).unwrap();
        assert_eq!(serialized["id"], json!(1));

        // Test with string ID
        let request_with_string = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: json!({"method": "test"}),
            id: Some(JsonRpcId::String("abc123".to_string())),
        };
        let serialized = serde_json::to_value(&request_with_string).unwrap();
        assert_eq!(serialized["id"], json!("abc123"));

        // Test with None (explicit null ID)
        let request_with_null = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            params: json!({"method": "test"}),
            id: None,
        };
        let serialized = serde_json::to_value(&request_with_null).unwrap();
        assert_eq!(serialized["id"], json!(null));
        assert!(serialized.as_object().unwrap().contains_key("id")); // Field is present with null value
    }

    #[test]
    fn test_json_rpc_request_deserialization_with_option() {
        // Test deserializing with number ID
        let json_with_number = json!({
            "jsonrpc": "2.0",
            "method": "test",
            "id": 42
        });
        let request: JsonRpcRequest<serde_json::Value> =
            serde_json::from_value(json_with_number).unwrap();
        assert_eq!(request.id, Some(JsonRpcId::Number(42)));

        // Test deserializing with string ID
        let json_with_string = json!({
            "jsonrpc": "2.0",
            "method": "test",
            "id": "req-123"
        });
        let request: JsonRpcRequest<serde_json::Value> =
            serde_json::from_value(json_with_string).unwrap();
        assert_eq!(request.id, Some(JsonRpcId::String("req-123".to_string())));

        // Test deserializing with explicit null ID
        let json_with_null = json!({
            "jsonrpc": "2.0",
            "method": "test",
            "id": null
        });
        let request: JsonRpcRequest<serde_json::Value> =
            serde_json::from_value(json_with_null).unwrap();
        assert_eq!(request.id, None);

        // Test deserializing without ID field (notification)
        let json_notification = json!({
            "jsonrpc": "2.0",
            "method": "test"
        });
        let request: JsonRpcRequest<serde_json::Value> =
            serde_json::from_value(json_notification).unwrap();
        assert_eq!(request.id, None);
    }

    #[test]
    fn test_option_json_rpc_id_serialization() {
        // Test Some(Number)
        let id_some_number = Some(JsonRpcId::Number(100));
        let serialized = serde_json::to_value(&id_some_number).unwrap();
        assert_eq!(serialized, json!(100));

        // Test Some(String)
        let id_some_string = Some(JsonRpcId::String("test".to_string()));
        let serialized = serde_json::to_value(&id_some_string).unwrap();
        assert_eq!(serialized, json!("test"));

        // Test None
        let id_none: Option<JsonRpcId> = None;
        let serialized = serde_json::to_value(&id_none).unwrap();
        assert_eq!(serialized, json!(null));
    }

    #[test]
    fn test_json_rpc_response_with_option_id() {
        // Test with Some(Number) ID
        let response_with_number =
            JsonRpcResponse::result(Some(JsonRpcId::Number(42)), json!("success"));
        assert_eq!(response_with_number.id, Some(JsonRpcId::Number(42)));

        // Test with Some(String) ID
        let response_with_string = JsonRpcResponse::result(
            Some(JsonRpcId::String("req-123".to_string())),
            json!("success"),
        );
        assert_eq!(
            response_with_string.id,
            Some(JsonRpcId::String("req-123".to_string()))
        );

        // Test with None ID (null)
        let response_with_null = JsonRpcResponse::result(None, json!("success"));
        assert_eq!(response_with_null.id, None);
    }
}
