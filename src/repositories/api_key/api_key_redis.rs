//! Redis-backed implementation of the ApiKeyRepository.

use crate::models::{ApiKeyModel, PaginationQuery, RepositoryError};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{ApiKeyRepositoryTrait, BatchRetrievalResult, PaginatedResult};
use async_trait::async_trait;
use log::{debug, error, warn};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

const API_KEY_PREFIX: &str = "apikey";
const API_KEY_LIST_KEY: &str = "apikey_list";

#[derive(Clone)]
pub struct RedisApiKeyRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisApiKeyRepository {}

impl RedisApiKeyRepository {
    pub fn new(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        if key_prefix.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Redis key prefix cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            client: connection_manager,
            key_prefix,
        })
    }

    /// Generate key for api key data: apikey:{api_key_id}
    fn api_key_key(&self, api_key_id: &str) -> String {
        format!("{}:{}:{}", self.key_prefix, API_KEY_PREFIX, api_key_id)
    }

    /// Generate key for api key list: apikey_list (paginated list of api key IDs)
    fn api_key_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, API_KEY_LIST_KEY)
    }

    async fn get_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<ApiKeyModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No api key IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.api_key_key(id)).collect();

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_api_keys"))?;

        let mut apikeys = Vec::new();
        let mut failed_count = 0;
        let mut failed_ids = Vec::new();
        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => match self.deserialize_entity(&json, &ids[i], "apikey") {
                    Ok(apikey) => apikeys.push(apikey),
                    Err(e) => {
                        failed_count += 1;
                        error!("Failed to deserialize api key {}: {}", ids[i], e);
                        failed_ids.push(ids[i].clone());
                    }
                },
                None => {
                    warn!("Plugin {} not found in batch fetch", ids[i]);
                }
            }
        }

        if failed_count > 0 {
            warn!(
                "Failed to deserialize {} out of {} api keys in batch",
                failed_count,
                ids.len()
            );
        }

        Ok(BatchRetrievalResult {
            results: apikeys,
            failed_ids,
        })
    }
}

impl fmt::Debug for RedisApiKeyRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RedisApiKeyRepository {{ key_prefix: {} }}",
            self.key_prefix
        )
    }
}

#[async_trait]
impl ApiKeyRepositoryTrait for RedisApiKeyRepository {
    async fn create(&self, entity: ApiKeyModel) -> Result<ApiKeyModel, RepositoryError> {
        if entity.id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "API Key ID cannot be empty".to_string(),
            ));
        }

        let key = self.api_key_key(&entity.id);
        let json = self.serialize_entity(&entity, |a| &a.id, "apikey")?;

        let mut conn = self.client.as_ref().clone();

        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "create_api_key_check"))?;

        if existing.is_some() {
            return Err(RepositoryError::ConstraintViolation(format!(
                "API Key with ID {} already exists",
                entity.id
            )));
        }

        // Use atomic pipeline for consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(&key, json);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "create_api_key"))?;

        debug!("Successfully created API Key {}", entity.id);
        Ok(entity)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<ApiKeyModel>, RepositoryError> {
        if query.page == 0 {
            return Err(RepositoryError::InvalidData(
                "Page number must be greater than 0".to_string(),
            ));
        }

        if query.per_page == 0 {
            return Err(RepositoryError::InvalidData(
                "Per page count must be greater than 0".to_string(),
            ));
        }
        let mut conn = self.client.as_ref().clone();
        let api_key_list_key = self.api_key_list_key();

        // Get total count
        let total: u64 = conn
            .scard(&api_key_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_count"))?;

        if total == 0 {
            return Ok(PaginatedResult {
                items: vec![],
                total: 0,
                page: query.page,
                per_page: query.per_page,
            });
        }

        // Get all IDs and paginate in memory
        let all_ids: Vec<String> = conn
            .smembers(&api_key_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_members"))?;

        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(all_ids.len());

        let ids_to_query = &all_ids[start..end];
        let items = self.get_by_ids(ids_to_query).await?;

        Ok(PaginatedResult {
            items: items.results.clone(),
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<ApiKeyModel>, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "API Key ID cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let api_key_key = self.api_key_key(id);

        debug!("Fetching api key with ID: {}", id);

        let json: Option<String> = conn
            .get(&api_key_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_api_key_by_id"))?;

        match json {
            Some(json) => {
                debug!("Found api key with ID: {}", id);
                self.deserialize_entity(&json, id, "apikey")
            }
            None => {
                debug!("Api key with ID {} not found", id);
                Ok(None)
            }
        }
    }

    async fn list_permissions(&self, api_key_id: &str) -> Result<Vec<String>, RepositoryError> {
        let api_key = self.get_by_id(api_key_id).await?;
        match api_key {
            Some(api_key) => Ok(api_key.permissions),
            None => Err(RepositoryError::NotFound(format!(
                "Api key with ID {} not found",
                api_key_id
            ))),
        }
    }

    async fn delete_by_id(&self, id: &str) -> Result<(), RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "API Key ID cannot be empty".to_string(),
            ));
        }

        let key = self.api_key_key(id);
        let api_key_list_key = self.api_key_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Deleting api key with ID: {}", id);

        // Check if api key exists
        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_api_key_check"))?;

        if existing.is_none() {
            return Err(RepositoryError::NotFound(format!(
                "Api key with ID {} not found",
                id
            )));
        }

        // Use atomic pipeline to ensure consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&key);
        pipe.srem(&api_key_list_key, id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_api_key"))?;

        debug!("Successfully deleted api key {}", id);
        Ok(())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let api_key_list_key = self.api_key_list_key();

        let count: u64 = conn
            .scard(&api_key_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "count_api_keys"))?;

        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {}
