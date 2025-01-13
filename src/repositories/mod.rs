use crate::models::RepositoryError;
use async_trait::async_trait;
use eyre::Result;

mod relayer;
mod transaction;

pub use relayer::*;
pub use transaction::*;

#[async_trait]
#[allow(dead_code)]
pub trait Repository<T, ID> {
    async fn create(&self, entity: T) -> Result<T, RepositoryError>;
    async fn get_by_id(&self, id: ID) -> Result<T, RepositoryError>;
    async fn list_all(&self) -> Result<Vec<T>, RepositoryError>;
    async fn update(&self, id: ID, entity: T) -> Result<T, RepositoryError>;
    async fn delete_by_id(&self, id: ID) -> Result<(), RepositoryError>;
    async fn count(&self) -> Result<usize, RepositoryError>;
}
