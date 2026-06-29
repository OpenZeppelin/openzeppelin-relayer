use crate::models::TransactionStatus;
use serde::de::IntoDeserializer;
use serde::{Deserialize, Deserializer};
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
    /// Optional status filter. Accepted case-insensitively (e.g. `submitted`,
    /// `Submitted`, and `SUBMITTED` all match), though the canonical form is
    /// lowercase.
    #[serde(default, deserialize_with = "deserialize_optional_status_ci")]
    pub status: Option<TransactionStatus>,
}

/// Deserializes an optional [`TransactionStatus`] from a query string
/// case-insensitively. `TransactionStatus` serializes as lowercase, so we lower
/// the incoming value before matching — this means `?status=Submitted` works as
/// well as `?status=submitted` rather than returning a 400.
fn deserialize_optional_status_ci<'de, D>(
    deserializer: D,
) -> Result<Option<TransactionStatus>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let lowered = raw.to_ascii_lowercase();
    TransactionStatus::deserialize(lowered.as_str().into_deserializer())
        .map(Some)
        .map_err(|_: serde::de::value::Error| {
            serde::de::Error::custom(format!("unknown transaction status: {raw}"))
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    // `web::Query` is exactly how the `?status=` param is decoded at the route layer,
    // so parsing through it mirrors real request handling.
    fn parse(qs: &str) -> Result<TransactionListQuery, actix_web::error::QueryPayloadError> {
        actix_web::web::Query::<TransactionListQuery>::from_query(qs).map(|q| q.into_inner())
    }

    #[test]
    fn status_is_parsed_case_insensitively() {
        for qs in ["status=submitted", "status=Submitted", "status=SUBMITTED"] {
            let q = parse(qs).unwrap_or_else(|e| panic!("{qs} should parse: {e}"));
            assert_eq!(q.status, Some(TransactionStatus::Submitted), "for {qs}");
        }
    }

    #[test]
    fn status_absent_is_none_with_defaults() {
        let q = parse("").unwrap();
        assert_eq!(q.status, None);
        assert_eq!(q.page, 1);
        assert_eq!(q.per_page, 10);
    }

    #[test]
    fn other_statuses_and_pagination_round_trip() {
        let q = parse("page=2&per_page=5&status=CANCELED").unwrap();
        assert_eq!(q.status, Some(TransactionStatus::Canceled));
        assert_eq!(q.page, 2);
        assert_eq!(q.per_page, 5);
    }

    #[test]
    fn invalid_status_is_rejected() {
        assert!(parse("status=bogus").is_err());
    }
}
