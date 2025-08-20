//! Redis-backed implementation of the NotificationRepository.

use crate::models::{NotificationRepoModel, PaginationQuery, RepositoryError};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{BatchRetrievalResult, PaginatedResult, Repository};
use async_trait::async_trait;
use log::{debug, error, warn};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

const NOTIFICATION_PREFIX: &str = "notification";
const NOTIFICATION_LIST_KEY: &str = "notification_list";

#[derive(Clone)]
pub struct RedisNotificationRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisNotificationRepository {}

impl RedisNotificationRepository {
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

    /// Generate key for notification data: notification:{notification_id}
    fn notification_key(&self, notification_id: &str) -> String {
        format!(
            "{}:{}:{}",
            self.key_prefix, NOTIFICATION_PREFIX, notification_id
        )
    }

    /// Generate key for notification list: notification_list (set of all notification IDs)
    fn notification_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, NOTIFICATION_LIST_KEY)
    }

    /// Batch fetch notifications by IDs
    async fn get_notifications_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<NotificationRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No notification IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.notification_key(id)).collect();

        debug!("Batch fetching {} notification data", keys.len());

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_notifications"))?;

        let mut notifications = Vec::new();
        let mut failed_count = 0;
        let mut failed_ids = Vec::new();
        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => {
                    match self.deserialize_entity::<NotificationRepoModel>(
                        &json,
                        &ids[i],
                        "notification",
                    ) {
                        Ok(notification) => notifications.push(notification),
                        Err(e) => {
                            failed_count += 1;
                            error!("Failed to deserialize notification {}: {}", ids[i], e);
                            failed_ids.push(ids[i].clone());
                            // Continue processing other notifications
                        }
                    }
                }
                None => {
                    warn!("Notification {} not found in batch fetch", ids[i]);
                }
            }
        }

        if failed_count > 0 {
            warn!(
                "Failed to deserialize {} out of {} notifications in batch",
                failed_count,
                ids.len()
            );
        }

        warn!("Failed to deserialize notifications: {:?}", failed_ids);

        debug!("Successfully fetched {} notifications", notifications.len());
        Ok(BatchRetrievalResult {
            results: notifications,
            failed_ids,
        })
    }
}

impl fmt::Debug for RedisNotificationRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisNotificationRepository")
            .field("client", &"<ConnectionManager>")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

#[async_trait]
impl Repository<NotificationRepoModel, String> for RedisNotificationRepository {
    async fn create(
        &self,
        entity: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        if entity.id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Notification ID cannot be empty".to_string(),
            ));
        }

        if entity.url.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Notification URL cannot be empty".to_string(),
            ));
        }

        let key = self.notification_key(&entity.id);
        let notification_list_key = self.notification_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Creating notification with ID: {}", entity.id);

        let value = self.serialize_entity(&entity, |n| &n.id, "notification")?;

        // Check if notification already exists
        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "create_notification_check"))?;

        if existing.is_some() {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Notification with ID '{}' already exists",
                entity.id
            )));
        }

        // Use atomic pipeline for consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(&key, &value);
        pipe.sadd(&notification_list_key, &entity.id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "create_notification"))?;

        debug!("Successfully created notification {}", entity.id);
        Ok(entity)
    }

    async fn get_by_id(&self, id: String) -> Result<NotificationRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Notification ID cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let key = self.notification_key(&id);

        debug!("Fetching notification with ID: {}", id);

        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_notification_by_id"))?;

        match value {
            Some(json) => {
                let notification =
                    self.deserialize_entity::<NotificationRepoModel>(&json, &id, "notification")?;
                debug!("Successfully fetched notification {}", id);
                Ok(notification)
            }
            None => {
                debug!("Notification {} not found", id);
                Err(RepositoryError::NotFound(format!(
                    "Notification with ID '{}' not found",
                    id
                )))
            }
        }
    }

    async fn list_all(&self) -> Result<Vec<NotificationRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let notification_list_key = self.notification_list_key();

        debug!("Fetching all notification IDs");

        let notification_ids: Vec<String> = conn
            .smembers(&notification_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_all_notification_ids"))?;

        debug!("Found {} notification IDs", notification_ids.len());

        let notifications = self.get_notifications_by_ids(&notification_ids).await?;
        Ok(notifications.results)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<NotificationRepoModel>, RepositoryError> {
        if query.per_page == 0 {
            return Err(RepositoryError::InvalidData(
                "per_page must be greater than 0".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let notification_list_key = self.notification_list_key();

        debug!(
            "Fetching paginated notifications (page: {}, per_page: {})",
            query.page, query.per_page
        );

        let all_notification_ids: Vec<String> = conn
            .smembers(&notification_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_notification_ids"))?;

        let total = all_notification_ids.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(all_notification_ids.len());

        if start >= all_notification_ids.len() {
            debug!(
                "Page {} is beyond available data (total: {})",
                query.page, total
            );
            return Ok(PaginatedResult {
                items: vec![],
                total,
                page: query.page,
                per_page: query.per_page,
            });
        }

        let page_ids = &all_notification_ids[start..end];
        let items = self.get_notifications_by_ids(page_ids).await?;

        debug!(
            "Successfully fetched {} notifications for page {}",
            items.results.len(),
            query.page
        );

        Ok(PaginatedResult {
            items: items.results.clone(),
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn update(
        &self,
        id: String,
        entity: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Notification ID cannot be empty".to_string(),
            ));
        }

        if id != entity.id {
            return Err(RepositoryError::InvalidData(
                "Notification ID in URL does not match entity ID".to_string(),
            ));
        }

        let key = self.notification_key(&id);
        let mut conn = self.client.as_ref().clone();

        debug!("Updating notification with ID: {}", id);

        // Check if notification exists
        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "update_notification_check"))?;

        if existing.is_none() {
            return Err(RepositoryError::NotFound(format!(
                "Notification with ID '{}' not found",
                id
            )));
        }

        let value = self.serialize_entity(&entity, |n| &n.id, "notification")?;

        // Update notification data
        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| self.map_redis_error(e, "update_notification"))?;

        debug!("Successfully updated notification {}", id);
        Ok(entity)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Notification ID cannot be empty".to_string(),
            ));
        }

        let key = self.notification_key(&id);
        let notification_list_key = self.notification_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Deleting notification with ID: {}", id);

        // Check if notification exists
        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_notification_check"))?;

        if existing.is_none() {
            return Err(RepositoryError::NotFound(format!(
                "Notification with ID '{}' not found",
                id
            )));
        }

        // Use atomic pipeline to ensure consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&key);
        pipe.srem(&notification_list_key, &id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_notification"))?;

        debug!("Successfully deleted notification {}", id);
        Ok(())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let notification_list_key = self.notification_list_key();

        debug!("Counting notifications");

        let count: u64 = conn
            .scard(&notification_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "count_notifications"))?;

        debug!("Notification count: {}", count);
        Ok(count as usize)
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let notification_list_key = self.notification_list_key();

        debug!("Checking if notification entries exist");

        let exists: bool = conn
            .exists(&notification_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "has_entries_check"))?;

        debug!("Notification entries exist: {}", exists);
        Ok(exists)
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let notification_list_key = self.notification_list_key();

        debug!("Dropping all notification entries");

        // Get all notification IDs first
        let notification_ids: Vec<String> = conn
            .smembers(&notification_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_get_ids"))?;

        if notification_ids.is_empty() {
            debug!("No notification entries to drop");
            return Ok(());
        }

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Delete all individual notification entries
        for notification_id in &notification_ids {
            let notification_key = self.notification_key(notification_id);
            pipe.del(&notification_key);
        }

        // Delete the notification list key
        pipe.del(&notification_list_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_pipeline"))?;

        debug!("Dropped {} notification entries", notification_ids.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::NotificationType;
    use redis::Client;
    use tokio;
    use uuid::Uuid;

    // Helper function to create test notifications
    fn create_test_notification(id: &str) -> NotificationRepoModel {
        NotificationRepoModel {
            id: id.to_string(),
            notification_type: NotificationType::Webhook,
            url: "http://localhost:8080/webhook".to_string(),
            signing_key: None,
        }
    }

    fn create_test_notification_with_url(id: &str, url: &str) -> NotificationRepoModel {
        NotificationRepoModel {
            id: id.to_string(),
            notification_type: NotificationType::Webhook,
            url: url.to_string(),
            signing_key: None,
        }
    }

    async fn setup_test_repo() -> RedisNotificationRepository {
        // Use a mock Redis URL - in real integration tests, this would connect to a test Redis instance
        let redis_url = std::env::var("REDIS_TEST_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

        let client = Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create connection manager");

        RedisNotificationRepository::new(Arc::new(connection_manager), "test_prefix".to_string())
            .expect("Failed to create RedisNotificationRepository")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_creation() {
        let repo = setup_test_repo().await;
        assert_eq!(repo.key_prefix, "test_prefix");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_empty_prefix_fails() {
        let redis_url = std::env::var("REDIS_TEST_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let client = Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create connection manager");

        let result = RedisNotificationRepository::new(Arc::new(connection_manager), "".to_string());
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_key_generation() {
        let repo = setup_test_repo().await;

        assert_eq!(
            repo.notification_key("test-id"),
            "test_prefix:notification:test-id"
        );
        assert_eq!(
            repo.notification_list_key(),
            "test_prefix:notification_list"
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]

    async fn test_serialize_deserialize_notification() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let notification = create_test_notification(&random_id);

        let serialized = repo
            .serialize_entity(&notification, |n| &n.id, "notification")
            .expect("Serialization should succeed");
        let deserialized: NotificationRepoModel = repo
            .deserialize_entity(&serialized, &random_id, "notification")
            .expect("Deserialization should succeed");

        assert_eq!(notification.id, deserialized.id);
        assert_eq!(
            notification.notification_type,
            deserialized.notification_type
        );
        assert_eq!(notification.url, deserialized.url);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_notification() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let notification = create_test_notification(&random_id);

        let result = repo.create(notification.clone()).await.unwrap();
        assert_eq!(result.id, notification.id);
        assert_eq!(result.url, notification.url);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_notification() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let notification = create_test_notification(&random_id);

        repo.create(notification.clone()).await.unwrap();
        let stored = repo.get_by_id(random_id.to_string()).await.unwrap();
        assert_eq!(stored.id, notification.id);
        assert_eq!(stored.url, notification.url);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_all_notifications() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let random_id2 = Uuid::new_v4().to_string();

        let notification1 = create_test_notification(&random_id);
        let notification2 = create_test_notification(&random_id2);

        repo.create(notification1).await.unwrap();
        repo.create(notification2).await.unwrap();

        let notifications = repo.list_all().await.unwrap();
        assert!(notifications.len() >= 2);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_count_notifications() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let notification = create_test_notification(&random_id);

        let count = repo.count().await.unwrap();
        repo.create(notification).await.unwrap();
        assert!(repo.count().await.unwrap() > count);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_nonexistent_notification() {
        let repo = setup_test_repo().await;
        let result = repo.get_by_id("nonexistent".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_duplicate_notification_creation() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();

        let notification = create_test_notification(&random_id);

        repo.create(notification.clone()).await.unwrap();
        let result = repo.create(notification).await;

        assert!(matches!(
            result,
            Err(RepositoryError::ConstraintViolation(_))
        ));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_notification() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let mut notification = create_test_notification(&random_id);

        // Create the notification first
        repo.create(notification.clone()).await.unwrap();

        // Update the notification
        notification.url = "http://updated.example.com/webhook".to_string();
        let result = repo
            .update(random_id.to_string(), notification.clone())
            .await
            .unwrap();
        assert_eq!(result.url, "http://updated.example.com/webhook");

        // Verify the update by fetching the notification
        let stored = repo.get_by_id(random_id.to_string()).await.unwrap();
        assert_eq!(stored.url, "http://updated.example.com/webhook");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_notification() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let notification = create_test_notification(&random_id);

        // Create the notification first
        repo.create(notification).await.unwrap();

        // Verify it exists
        let stored = repo.get_by_id(random_id.to_string()).await.unwrap();
        assert_eq!(stored.id, random_id);

        // Delete the notification
        repo.delete_by_id(random_id.to_string()).await.unwrap();

        // Verify it's gone
        let result = repo.get_by_id(random_id.to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_paginated() {
        let repo = setup_test_repo().await;

        // Create multiple notifications
        for i in 1..=10 {
            let random_id = Uuid::new_v4().to_string();
            let notification =
                create_test_notification_with_url(&random_id, &format!("http://test{}.com", i));
            repo.create(notification).await.unwrap();
        }

        // Test first page with 3 items per page
        let query = PaginationQuery {
            page: 1,
            per_page: 3,
        };
        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 3);
        assert!(result.total >= 10);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 3);

        // Test empty page (beyond total items)
        let query = PaginationQuery {
            page: 1000,
            per_page: 3,
        };
        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 0);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_debug_implementation() {
        let repo = setup_test_repo().await;
        let debug_str = format!("{:?}", repo);
        assert!(debug_str.contains("RedisNotificationRepository"));
        assert!(debug_str.contains("test_prefix"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_error_handling_empty_id() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_id("".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_pagination_validation() {
        let repo = setup_test_repo().await;

        let query = PaginationQuery {
            page: 1,
            per_page: 0,
        };
        let result = repo.list_paginated(query).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_nonexistent_notification() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let notification = create_test_notification(&random_id);

        let result = repo.update(random_id.to_string(), notification).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_nonexistent_notification() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();

        let result = repo.delete_by_id(random_id.to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_with_empty_id() {
        let repo = setup_test_repo().await;
        let notification = create_test_notification("test-id");

        let result = repo.update("".to_string(), notification).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_with_empty_id() {
        let repo = setup_test_repo().await;

        let result = repo.delete_by_id("".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_with_mismatched_id() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let notification = create_test_notification(&random_id);

        // Create the notification first
        repo.create(notification.clone()).await.unwrap();

        // Try to update with mismatched ID
        let result = repo.update("different-id".to_string(), notification).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_maintains_list_consistency() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let notification = create_test_notification(&random_id);

        // Create the notification
        repo.create(notification).await.unwrap();

        // Verify it's in the list
        let all_notifications = repo.list_all().await.unwrap();
        assert!(all_notifications.iter().any(|n| n.id == random_id));

        // Delete the notification
        repo.delete_by_id(random_id.to_string()).await.unwrap();

        // Verify it's no longer in the list
        let all_notifications = repo.list_all().await.unwrap();
        assert!(!all_notifications.iter().any(|n| n.id == random_id));
    }

    // test has_entries
    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_has_entries() {
        let repo = setup_test_repo().await;
        assert!(!repo.has_entries().await.unwrap());

        let notification = create_test_notification("test");
        repo.create(notification.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries() {
        let repo = setup_test_repo().await;
        let notification = create_test_notification("test");

        repo.create(notification.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}
