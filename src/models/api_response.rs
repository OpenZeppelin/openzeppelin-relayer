use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::services::plugins::LogEntry;

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, ToSchema)]
pub struct PaginationMeta {
    pub current_page: u32,
    pub per_page: u32,
    pub total_items: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, ToSchema)]
pub struct PluginMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<LogEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traces: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    #[schema(nullable = false)]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub pagination: Option<PaginationMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub metadata: Option<PluginMetadata>,
}

#[allow(dead_code)]
impl<T> ApiResponse<T> {
    pub fn new(data: Option<T>, error: Option<String>, pagination: Option<PaginationMeta>) -> Self {
        Self {
            success: error.is_none(),
            data,
            error,
            pagination,
            metadata: None,
        }
    }

    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            pagination: None,
            metadata: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
            pagination: None,
            metadata: None,
        }
    }

    pub fn no_data() -> Self {
        Self {
            success: true,
            data: None,
            error: None,
            pagination: None,
            metadata: None,
        }
    }

    pub fn paginated(data: T, meta: PaginationMeta) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            pagination: Some(meta),
            metadata: None,
        }
    }

    pub fn with_metadata(data: T, metadata: PluginMetadata) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            pagination: None,
            metadata: Some(metadata),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_with_data() {
        let data = "test data";
        let response = ApiResponse::new(Some(data), None, None);

        assert!(response.success);
        assert_eq!(response.data, Some(data));
        assert_eq!(response.error, None);
        assert_eq!(response.pagination, None);
    }

    #[test]
    fn test_new_with_error() {
        let error = "test error";
        let response: ApiResponse<()> = ApiResponse::new(None, Some(error.to_string()), None);

        assert!(!response.success);
        assert_eq!(response.data, None);
        assert_eq!(response.error, Some(error.to_string()));
        assert_eq!(response.pagination, None);
    }

    #[test]
    fn test_success() {
        let data = "test data";
        let response = ApiResponse::success(data);

        assert!(response.success);
        assert_eq!(response.data, Some(data));
        assert_eq!(response.error, None);
        assert_eq!(response.pagination, None);
    }

    #[test]
    fn test_error() {
        let error = "test error";
        let response: ApiResponse<()> = ApiResponse::error(error);

        assert!(!response.success);
        assert_eq!(response.data, None);
        assert_eq!(response.error, Some(error.to_string()));
        assert_eq!(response.pagination, None);
    }

    #[test]
    fn test_no_data() {
        let response: ApiResponse<String> = ApiResponse::no_data();

        assert!(response.success);
        assert_eq!(response.data, None);
        assert_eq!(response.error, None);
        assert_eq!(response.pagination, None);
    }

    #[test]
    fn test_paginated() {
        let data = "test data";
        let pagination = PaginationMeta {
            current_page: 1,
            per_page: 10,
            total_items: 100,
        };

        let response = ApiResponse::paginated(data, pagination.clone());

        assert!(response.success);
        assert_eq!(response.data, Some(data));
        assert_eq!(response.error, None);
        assert_eq!(response.pagination, Some(pagination));
    }
}
