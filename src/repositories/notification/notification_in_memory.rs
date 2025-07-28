//! This module defines an in-memory notification repository for managing
//! notifications. It provides full CRUD functionality including create, retrieve,
//! update, delete, and list operations. The repository is implemented using a
//! `Mutex`-protected `HashMap` to ensure thread safety in asynchronous contexts.

use crate::{
    models::{NotificationConfig, NotificationRepoModel, RepositoryError},
    repositories::*,
};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryNotificationRepository {
    store: Mutex<HashMap<String, NotificationRepoModel>>,
}

impl Clone for InMemoryNotificationRepository {
    fn clone(&self) -> Self {
        // Try to get the current data, or use empty HashMap if lock fails
        let data = self
            .store
            .try_lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| HashMap::new());

        Self {
            store: Mutex::new(data),
        }
    }
}

#[allow(dead_code)]
impl InMemoryNotificationRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }
}

impl Default for InMemoryNotificationRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Repository<NotificationRepoModel, String> for InMemoryNotificationRepository {
    async fn create(
        &self,
        notification: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.contains_key(&notification.id) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Notification with ID '{}' already exists",
                notification.id
            )));
        }
        store.insert(notification.id.clone(), notification.clone());
        Ok(notification)
    }

    async fn get_by_id(&self, id: String) -> Result<NotificationRepoModel, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        match store.get(&id) {
            Some(entity) => Ok(entity.clone()),
            None => Err(RepositoryError::NotFound(format!(
                "Notification with ID '{}' not found",
                id
            ))),
        }
    }

    #[allow(clippy::map_entry)]
    async fn update(
        &self,
        id: String,
        notification: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;

        // Check if notification exists
        if !store.contains_key(&id) {
            return Err(RepositoryError::NotFound(format!(
                "Notification with ID '{}' not found",
                id
            )));
        }

        if id != notification.id {
            return Err(RepositoryError::InvalidData(format!(
                "ID mismatch: URL parameter '{}' does not match entity ID '{}'",
                id, notification.id
            )));
        }

        store.insert(id, notification.clone());
        Ok(notification)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;

        match store.remove(&id) {
            Some(_) => Ok(()),
            None => Err(RepositoryError::NotFound(format!(
                "Notification with ID {} not found",
                id
            ))),
        }
    }

    async fn list_all(&self) -> Result<Vec<NotificationRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let notifications: Vec<NotificationRepoModel> = store.values().cloned().collect();
        Ok(notifications)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<NotificationRepoModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;
        let items: Vec<NotificationRepoModel> = self
            .store
            .lock()
            .await
            .values()
            .skip(start)
            .take(query.per_page as usize)
            .cloned()
            .collect();

        Ok(PaginatedResult {
            items,
            total: total as u64,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let length = store.len();
        Ok(length)
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(!store.is_empty())
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.clear();
        Ok(())
    }
}

impl TryFrom<NotificationConfig> for NotificationRepoModel {
    type Error = ConversionError;

    fn try_from(config: NotificationConfig) -> Result<Self, Self::Error> {
        let signing_key = config.get_signing_key().map_err(|e| {
            ConversionError::InvalidConfig(format!("Failed to get signing key: {}", e))
        })?;

        Ok(NotificationRepoModel {
            id: config.id.clone(),
            url: config.url.clone(),
            notification_type: config.r#type,
            signing_key,
        })
    }
}
#[cfg(test)]
mod tests {
    use crate::models::NotificationType;

    use super::*;

    fn create_test_notification(id: String) -> NotificationRepoModel {
        NotificationRepoModel {
            id: id.clone(),
            url: "http://localhost".to_string(),
            notification_type: NotificationType::Webhook,
            signing_key: None,
        }
    }

    #[actix_web::test]
    async fn test_new_repository_is_empty() {
        let repo = InMemoryNotificationRepository::new();
        assert_eq!(repo.count().await.unwrap(), 0);
    }

    #[actix_web::test]
    async fn test_add_notification() {
        let repo = InMemoryNotificationRepository::new();
        let notification = create_test_notification("test".to_string());

        repo.create(notification.clone()).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);

        let stored = repo.get_by_id("test".to_string()).await.unwrap();
        assert_eq!(stored.id, notification.id);
    }

    #[actix_web::test]
    async fn test_update_notification() {
        let repo = InMemoryNotificationRepository::new();
        let notification = create_test_notification("test".to_string());

        // First create the notification
        repo.create(notification.clone()).await.unwrap();

        // Update the notification
        let mut updated_notification = notification.clone();
        updated_notification.url = "http://updated.example.com".to_string();

        let result = repo
            .update("test".to_string(), updated_notification.clone())
            .await;
        assert!(result.is_ok());

        let updated = result.unwrap();
        assert_eq!(updated.id, "test");
        assert_eq!(updated.url, "http://updated.example.com");

        // Verify the update persisted
        let stored = repo.get_by_id("test".to_string()).await.unwrap();
        assert_eq!(stored.url, "http://updated.example.com");
    }

    #[actix_web::test]
    async fn test_list_notifications() {
        let repo = InMemoryNotificationRepository::new();
        let notification1 = create_test_notification("test".to_string());
        let notification2 = create_test_notification("test2".to_string());

        repo.create(notification1.clone()).await.unwrap();
        repo.create(notification2).await.unwrap();

        let notifications = repo.list_all().await.unwrap();
        assert_eq!(notifications.len(), 2);
    }

    #[actix_web::test]
    async fn test_update_nonexistent_notification() {
        let repo = InMemoryNotificationRepository::new();
        let notification = create_test_notification("test".to_string());

        let result = repo.update("test2".to_string(), notification).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_get_nonexistent_notification() {
        let repo = InMemoryNotificationRepository::new();

        let result = repo.get_by_id("test".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    // test has_entries
    #[actix_web::test]
    async fn test_has_entries() {
        let repo = InMemoryNotificationRepository::new();
        assert!(!repo.has_entries().await.unwrap());

        let notification = create_test_notification("test".to_string());

        repo.create(notification.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[actix_web::test]
    async fn test_drop_all_entries() {
        let repo = InMemoryNotificationRepository::new();
        let notification = create_test_notification("test".to_string());

        repo.create(notification.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }

    #[actix_web::test]
    async fn test_delete_notification() {
        let repo = InMemoryNotificationRepository::new();
        let notification = create_test_notification("test".to_string());

        // Create the notification first
        repo.create(notification.clone()).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);

        // Delete the notification
        let result = repo.delete_by_id("test".to_string()).await;
        assert!(result.is_ok());

        // Verify it's gone
        assert_eq!(repo.count().await.unwrap(), 0);
        let get_result = repo.get_by_id("test".to_string()).await;
        assert!(matches!(get_result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_delete_nonexistent_notification() {
        let repo = InMemoryNotificationRepository::new();

        let result = repo.delete_by_id("nonexistent".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_update_with_id_mismatch() {
        let repo = InMemoryNotificationRepository::new();
        let notification = create_test_notification("test".to_string());

        // Create the notification first
        repo.create(notification.clone()).await.unwrap();

        // Try to update with mismatched ID
        let mut updated_notification = notification.clone();
        updated_notification.id = "different-id".to_string();

        let result = repo.update("test".to_string(), updated_notification).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[actix_web::test]
    async fn test_update_delete_integration() {
        let repo = InMemoryNotificationRepository::new();
        let notification1 = create_test_notification("test1".to_string());
        let notification2 = create_test_notification("test2".to_string());

        // Create two notifications
        repo.create(notification1.clone()).await.unwrap();
        repo.create(notification2.clone()).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 2);

        // Update the first notification
        let mut updated_notification1 = notification1.clone();
        updated_notification1.url = "http://updated.example.com".to_string();

        let update_result = repo
            .update("test1".to_string(), updated_notification1)
            .await;
        assert!(update_result.is_ok());

        // Verify the update
        let stored = repo.get_by_id("test1".to_string()).await.unwrap();
        assert_eq!(stored.url, "http://updated.example.com");

        // Delete the second notification
        let delete_result = repo.delete_by_id("test2".to_string()).await;
        assert!(delete_result.is_ok());

        // Verify final state
        assert_eq!(repo.count().await.unwrap(), 1);
        let remaining = repo.list_all().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "test1");
        assert_eq!(remaining[0].url, "http://updated.example.com");
    }
}
