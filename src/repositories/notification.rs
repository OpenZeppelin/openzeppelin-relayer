use crate::{
    config::{NotificationFileConfig, NotificationFileConfigType},
    models::{NotificationRepoModel, NotificationType as ModelNotificationType, RepositoryError},
    repositories::*,
};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryNotificationRepository {
    store: Mutex<HashMap<String, NotificationRepoModel>>,
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
                "Notification with ID {} already exists",
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
                "Notification with ID {} not found",
                id
            ))),
        }
    }

    #[allow(clippy::map_entry)]
    async fn update(
        &self,
        _id: String,
        _relayer: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        Err(RepositoryError::NotSupported("Not supported".to_string()))
    }

    async fn delete_by_id(&self, _id: String) -> Result<(), RepositoryError> {
        Err(RepositoryError::NotSupported("Not supported".to_string()))
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
}

impl TryFrom<NotificationFileConfig> for NotificationRepoModel {
    type Error = ConversionError;

    fn try_from(config: NotificationFileConfig) -> Result<Self, Self::Error> {
        Ok(NotificationRepoModel {
            id: config.id.clone(),
            url: config.url.clone(),
            notification_type: ModelNotificationType::try_from(&config.r#type)?,
            signing_key: config.get_signing_key(),
        })
    }
}

impl TryFrom<&NotificationFileConfigType> for ModelNotificationType {
    type Error = ConversionError;

    fn try_from(config: &NotificationFileConfigType) -> Result<Self, Self::Error> {
        match config {
            NotificationFileConfigType::Webhook => Ok(ModelNotificationType::Webhook),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_notification(id: String) -> NotificationRepoModel {
        NotificationRepoModel {
            id: id.clone(),
            url: "http://localhost".to_string(),
            notification_type: ModelNotificationType::Webhook,
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

        let result = repo.update("test".to_string(), notification).await;
        assert!(matches!(result, Err(RepositoryError::NotSupported(_))));
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
        assert!(matches!(result, Err(RepositoryError::NotSupported(_))));
    }

    #[actix_web::test]
    async fn test_get_nonexistent_notification() {
        let repo = InMemoryNotificationRepository::new();

        let result = repo.get_by_id("test".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }
}
