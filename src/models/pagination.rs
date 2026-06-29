use crate::models::TransactionStatus;
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Clone, ToSchema)]
pub struct PaginationQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

/// Pagination + optional status filter for listing transactions.
#[derive(Debug, Deserialize, Clone, ToSchema)]
pub struct TransactionListQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
    pub status: Option<TransactionStatus>,
}

impl From<TransactionListQuery> for PaginationQuery {
    fn from(q: TransactionListQuery) -> Self {
        PaginationQuery {
            page: q.page,
            per_page: q.per_page,
        }
    }
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    10
}
