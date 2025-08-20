//! Notification Repository Module
//!
//! This module provides the notification repository layer for the OpenZeppelin Relayer service.
//! It implements the Repository pattern to abstract notification data persistence operations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Features
//!
//! - **CRUD Operations**: Create, read, update, and delete notification configurations
//! - **Webhook Support**: Store webhook notification configurations
//! - **Pagination Support**: Efficient paginated listing of notifications
//! - **Configuration Management**: Handle notification signing keys and URLs
//!
//! ## Repository Implementations
//!
//! - [`InMemoryNotificationRepository`]: Fast in-memory storage for testing/development
//! - [`RedisNotificationRepository`]: Redis-backed storage for production environments
//!
mod notification_in_memory;
mod notification_redis;

pub use notification_in_memory::*;
pub use notification_redis::*;
use redis::aio::ConnectionManager;

use crate::{
    models::{NotificationRepoModel, RepositoryError},
    repositories::{PaginatedResult, PaginationQuery, Repository},
};
use async_trait::async_trait;
use std::sync::Arc;

/// Enum wrapper for different notification repository implementations
#[derive(Debug, Clone)]
pub enum NotificationRepositoryStorage {
    InMemory(InMemoryNotificationRepository),
    Redis(RedisNotificationRepository),
}

impl NotificationRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryNotificationRepository::new())
    }
    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        Ok(Self::Redis(RedisNotificationRepository::new(
            connection_manager,
            key_prefix,
        )?))
    }
}

#[async_trait]
impl Repository<NotificationRepoModel, String> for NotificationRepositoryStorage {
    async fn create(
        &self,
        entity: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.create(entity).await,
            NotificationRepositoryStorage::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            NotificationRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<NotificationRepoModel>, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.list_all().await,
            NotificationRepositoryStorage::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<NotificationRepoModel>, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            NotificationRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.update(id, entity).await,
            NotificationRepositoryStorage::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.delete_by_id(id).await,
            NotificationRepositoryStorage::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.count().await,
            NotificationRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.has_entries().await,
            NotificationRepositoryStorage::Redis(repo) => repo.has_entries().await,
        }
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.drop_all_entries().await,
            NotificationRepositoryStorage::Redis(repo) => repo.drop_all_entries().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RepositoryError;
    use crate::repositories::PaginationQuery;
    use crate::utils::mocks::mockutils::create_mock_notification;
    use color_eyre::Result;

    fn create_test_notification(id: &str) -> NotificationRepoModel {
        create_mock_notification(id.to_string())
    }

    #[tokio::test]
    async fn test_new_in_memory() {
        let storage = NotificationRepositoryStorage::new_in_memory();

        match storage {
            NotificationRepositoryStorage::InMemory(_) => {
                // Success - verify it's the InMemory variant
            }
            NotificationRepositoryStorage::Redis(_) => {
                panic!("Expected InMemory variant, got Redis");
            }
        }
    }

    #[tokio::test]
    async fn test_create_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();
        let notification = create_test_notification("test-notification");

        let created = storage.create(notification.clone()).await?;
        assert_eq!(created.id, notification.id);
        assert_eq!(created.url, notification.url);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_by_id_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();
        let notification = create_test_notification("test-notification");

        // Create notification first
        storage.create(notification.clone()).await?;

        // Get by ID
        let retrieved = storage.get_by_id("test-notification".to_string()).await?;
        assert_eq!(retrieved.id, notification.id);
        assert_eq!(retrieved.url, notification.url);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_by_id_not_found_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        let result = storage.get_by_id("non-existent".to_string()).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_list_all_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        // Initially empty
        let notifications = storage.list_all().await?;
        assert!(notifications.is_empty());

        // Add notifications
        let notification1 = create_test_notification("notification-1");
        let notification2 = create_test_notification("notification-2");

        storage.create(notification1.clone()).await?;
        storage.create(notification2.clone()).await?;

        let all_notifications = storage.list_all().await?;
        assert_eq!(all_notifications.len(), 2);

        let ids: Vec<&str> = all_notifications.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"notification-1"));
        assert!(ids.contains(&"notification-2"));

        Ok(())
    }

    #[tokio::test]
    async fn test_list_paginated_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        // Add test notifications
        for i in 1..=5 {
            let notification = create_test_notification(&format!("notification-{}", i));
            storage.create(notification).await?;
        }

        // Test pagination
        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };
        let page = storage.list_paginated(query).await?;

        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 5);
        assert_eq!(page.page, 1);
        assert_eq!(page.per_page, 2);

        // Test second page
        let query2 = PaginationQuery {
            page: 2,
            per_page: 2,
        };
        let page2 = storage.list_paginated(query2).await?;

        assert_eq!(page2.items.len(), 2);
        assert_eq!(page2.total, 5);
        assert_eq!(page2.page, 2);
        assert_eq!(page2.per_page, 2);

        // Test final page
        let query3 = PaginationQuery {
            page: 3,
            per_page: 2,
        };
        let page3 = storage.list_paginated(query3).await?;

        assert_eq!(page3.items.len(), 1);
        assert_eq!(page3.total, 5);
        assert_eq!(page3.page, 3);
        assert_eq!(page3.per_page, 2);

        Ok(())
    }

    #[tokio::test]
    async fn test_update_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();
        let notification = create_test_notification("test-notification");

        // Create notification first
        storage.create(notification.clone()).await?;

        let mut updated_notification = notification.clone();
        updated_notification.url = "https://updated.webhook.com".to_string();

        let result = storage
            .update(
                "test-notification".to_string(),
                updated_notification.clone(),
            )
            .await;
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert_eq!(updated.url, "https://updated.webhook.com");

        // Verify the update persisted
        let retrieved = storage.get_by_id("test-notification".to_string()).await?;
        assert_eq!(retrieved.url, "https://updated.webhook.com");

        Ok(())
    }

    #[tokio::test]
    async fn test_update_not_found_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();
        let notification = create_test_notification("non-existent");

        let result = storage
            .update("non-existent".to_string(), notification)
            .await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_by_id_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();
        let notification = create_test_notification("test-notification");

        // Create notification first
        storage.create(notification.clone()).await?;

        // Verify it exists
        let retrieved = storage.get_by_id("test-notification".to_string()).await?;
        assert_eq!(retrieved.id, "test-notification");

        let result = storage.delete_by_id("test-notification".to_string()).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_result = storage.get_by_id("test-notification".to_string()).await;
        assert!(matches!(get_result, Err(RepositoryError::NotFound(_))));

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_by_id_not_found_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        let result = storage.delete_by_id("non-existent".to_string()).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_count_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        // Initially empty
        let count = storage.count().await?;
        assert_eq!(count, 0);

        // Add notifications
        let notification1 = create_test_notification("notification-1");
        let notification2 = create_test_notification("notification-2");

        storage.create(notification1).await?;
        let count_after_one = storage.count().await?;
        assert_eq!(count_after_one, 1);

        storage.create(notification2).await?;
        let count_after_two = storage.count().await?;
        assert_eq!(count_after_two, 2);

        // Delete one - should now succeed
        let delete_result = storage.delete_by_id("notification-1".to_string()).await;
        assert!(delete_result.is_ok());

        // Count should decrease after successful delete
        let count_after_delete = storage.count().await?;
        assert_eq!(count_after_delete, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_has_entries_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        // Initially empty
        let has_entries = storage.has_entries().await?;
        assert!(!has_entries);

        // Add notification
        let notification = create_test_notification("test-notification");
        storage.create(notification).await?;

        let has_entries_after_create = storage.has_entries().await?;
        assert!(has_entries_after_create);

        // Delete notification - should now succeed
        let delete_result = storage.delete_by_id("test-notification".to_string()).await;
        assert!(delete_result.is_ok());

        // Should no longer have entries after successful delete
        let has_entries_after_delete = storage.has_entries().await?;
        assert!(!has_entries_after_delete);

        Ok(())
    }

    #[tokio::test]
    async fn test_drop_all_entries_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        // Add multiple notifications
        for i in 1..=5 {
            let notification = create_test_notification(&format!("notification-{}", i));
            storage.create(notification).await?;
        }

        // Verify they exist
        let count_before = storage.count().await?;
        assert_eq!(count_before, 5);

        let has_entries_before = storage.has_entries().await?;
        assert!(has_entries_before);

        // Drop all entries
        storage.drop_all_entries().await?;

        // Verify they're gone
        let count_after = storage.count().await?;
        assert_eq!(count_after, 0);

        let has_entries_after = storage.has_entries().await?;
        assert!(!has_entries_after);

        let all_notifications = storage.list_all().await?;
        assert!(all_notifications.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_create_duplicate_id_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();
        let notification = create_test_notification("duplicate-id");

        // Create first notification
        storage.create(notification.clone()).await?;

        // Try to create another with same ID - should fail
        let result = storage.create(notification.clone()).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_workflow_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        // 1. Start with empty storage
        assert!(!storage.has_entries().await?);
        assert_eq!(storage.count().await?, 0);

        // 2. Create notification
        let notification = create_test_notification("workflow-test");
        let created = storage.create(notification.clone()).await?;
        assert_eq!(created.id, "workflow-test");

        // 3. Verify it exists
        assert!(storage.has_entries().await?);
        assert_eq!(storage.count().await?, 1);

        // 4. Retrieve it
        let retrieved = storage.get_by_id("workflow-test".to_string()).await?;
        assert_eq!(retrieved.id, "workflow-test");

        // 5. Update it - should now succeed
        let mut updated = retrieved.clone();
        updated.url = "https://updated.example.com".to_string();
        let update_result = storage.update("workflow-test".to_string(), updated).await;
        assert!(update_result.is_ok());
        let updated_notification = update_result.unwrap();
        assert_eq!(updated_notification.url, "https://updated.example.com");

        // 6. Verify the update persisted
        let after_update = storage.get_by_id("workflow-test".to_string()).await?;
        assert_eq!(after_update.url, "https://updated.example.com");

        // 7. Delete it - should now succeed
        let delete_result = storage.delete_by_id("workflow-test".to_string()).await;
        assert!(delete_result.is_ok());

        // 8. Verify it's gone
        assert!(!storage.has_entries().await?);
        assert_eq!(storage.count().await?, 0);

        let result = storage.get_by_id("workflow-test".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));

        Ok(())
    }

    #[tokio::test]
    async fn test_pagination_edge_cases_in_memory() -> Result<()> {
        let storage = NotificationRepositoryStorage::new_in_memory();

        // Test pagination with empty storage
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let page = storage.list_paginated(query).await?;
        assert_eq!(page.items.len(), 0);
        assert_eq!(page.total, 0);
        assert_eq!(page.page, 1);
        assert_eq!(page.per_page, 10);

        // Add one notification
        let notification = create_test_notification("single-item");
        storage.create(notification).await?;

        // Test pagination with single item
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let page = storage.list_paginated(query).await?;
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total, 1);
        assert_eq!(page.page, 1);
        assert_eq!(page.per_page, 10);

        // Test pagination with page beyond total
        let query = PaginationQuery {
            page: 3,
            per_page: 10,
        };
        let page = storage.list_paginated(query).await?;
        assert_eq!(page.items.len(), 0);
        assert_eq!(page.total, 1);
        assert_eq!(page.page, 3);
        assert_eq!(page.per_page, 10);

        Ok(())
    }
}
