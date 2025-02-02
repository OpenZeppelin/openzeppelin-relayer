//! # Repository Module
//!
//! Implements data persistence layer for the relayer service using Repository pattern.

use crate::models::{PaginationQuery, RepositoryError};
use async_trait::async_trait;
use eyre::Result;

mod relayer;
pub use relayer::*;

mod transaction;
use serde::Serialize;
use thiserror::Error;
pub use transaction::*;

mod signer;
pub use signer::*;

mod notification;
pub use notification::*;

mod transaction_counter;
pub use transaction_counter::*;

#[derive(Debug)]
pub struct PaginatedResult<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

#[async_trait]
#[allow(dead_code)]
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
}

#[derive(Error, Debug, Serialize)]
pub enum TransactionCounterError {
    #[error("No sequence found for relayer {relayer_id} and address {address}")]
    SequenceNotFound { relayer_id: String, address: String },
    #[error("Counter not found for {0}")]
    NotFound(String),
}

#[allow(dead_code)]
pub trait TransactionCounterTrait {
    fn get(&self, relayer_id: &str, address: &str) -> Result<Option<u64>, TransactionCounterError>;

    fn get_and_increment(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, TransactionCounterError>;

    fn decrement(&self, relayer_id: &str, address: &str) -> Result<u64, TransactionCounterError>;

    fn set(
        &self,
        relayer_id: &str,
        address: &str,
        value: u64,
    ) -> Result<(), TransactionCounterError>;
}
