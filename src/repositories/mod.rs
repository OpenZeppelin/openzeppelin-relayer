//! # Repository Module
//!
//! Implements data persistence layer for the relayer service using Repository pattern.

use crate::models::{PaginationQuery, RepositoryError};
use async_trait::async_trait;
use eyre::Result;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod relayer;
pub use relayer::*;

pub mod transaction;
pub use transaction::*;

mod signer;
pub use signer::*;

pub mod notification;
pub use notification::*;

mod transaction_counter;
pub use transaction_counter::*;

pub mod network;
pub use network::*;

mod plugin;
pub use plugin::*;

pub mod api_key;
pub use api_key::*;

// Redis base utilities for shared functionality
pub mod redis_base;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PaginatedResult<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

pub struct BatchRetrievalResult<T> {
    pub results: Vec<T>,
    pub failed_ids: Vec<String>,
}

/// Result of a batch delete operation
#[derive(Debug, Default)]
pub struct BatchDeleteResult {
    /// Number of entities successfully deleted
    pub deleted_count: usize,
    /// IDs that failed to delete with their error messages
    pub failed: Vec<(String, String)>,
}

/// Request to delete a transaction with pre-extracted data needed for index cleanup.
/// This avoids re-fetching transaction data when the caller already has it.
#[derive(Debug, Clone)]
pub struct TransactionDeleteRequest {
    /// Transaction ID
    pub id: String,
    /// Relayer ID (needed for building index keys)
    pub relayer_id: String,
    /// Nonce if available (needed for nonce index cleanup, EVM-specific)
    pub nonce: Option<u64>,
}

impl TransactionDeleteRequest {
    pub fn new(id: String, relayer_id: String, nonce: Option<u64>) -> Self {
        Self {
            id,
            relayer_id,
            nonce,
        }
    }
}

#[cfg(test)]
use mockall::automock;
use utoipa::ToSchema;

#[async_trait]
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait Repository<T, ID> {
    async fn create(&self, entity: T) -> Result<T, RepositoryError>;
    async fn get_by_id(&self, id: ID) -> Result<T, RepositoryError>;
    async fn list_all(&self) -> Result<Vec<T>, RepositoryError>;
    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<T>, RepositoryError>;
    async fn update(&self, id: ID, entity: T) -> Result<T, RepositoryError>;
    async fn delete_by_id(&self, id: ID) -> Result<(), RepositoryError>;
    async fn count(&self) -> Result<usize, RepositoryError>;

    /// Check if the repository contains any entries.
    async fn has_entries(&self) -> Result<bool, RepositoryError>;

    /// Drop all entries from storage.
    /// This completely clears all data, indexes, and metadata.
    /// Use with caution as this permanently deletes all data.
    async fn drop_all_entries(&self) -> Result<(), RepositoryError>;
}

#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Invalid type: {0}")]
    InvalidType(String),
    #[error("Invalid config: {0}")]
    InvalidConfig(String),
}
