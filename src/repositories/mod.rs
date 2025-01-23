//! # Repository Module
//!
//! Implements data persistence layer for the relayer service using Repository pattern.

use crate::models::{PaginationQuery, RepositoryError};
use async_trait::async_trait;
use eyre::Result;

mod relayer;
pub use relayer::*;

mod transaction;
pub use transaction::*;

mod signer;
pub use signer::*;

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
