use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct PaginationMeta {
    pub current_page: u32,
    pub per_page: u32,
    pub total_items: u64,
}

#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<PaginationMeta>,
}

#[allow(dead_code)]
impl<T> ApiResponse<T> {
    pub fn new(data: Option<T>, error: Option<String>, pagination: Option<PaginationMeta>) -> Self {
        Self {
            success: error.is_none(),
            data,
            error,
            pagination,
        }
    }

    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            pagination: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
            pagination: None,
        }
    }

    pub fn no_data() -> Self {
        Self {
            success: true,
            data: None,
            error: None,
            pagination: None,
        }
    }

    pub fn paginated(data: T, meta: PaginationMeta) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            pagination: Some(meta),
        }
    }
}
