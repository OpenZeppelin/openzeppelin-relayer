//! # API Permissions System
//!
//! This module defines all available permissions for API key-based access control.
//! Permissions follow the format: `{resource}:{action}:{scope}`
//!
//! ## Permission Format
//! - **resource**: The API resource being accessed (relayers, transactions, etc.)
//! - **action**: The operation being performed (get, create, update, delete, execute)
//! - **scope**: The scope of access (all, owned, specific_id, etc.)
//!
//! ## Permission Hierarchy
//! - `*:*:*` - Full system access (super admin)
//! - `{resource}:*:*` - Full access to a specific resource
//! - `{resource}:{action}:*` - Specific action on all instances of a resource
//! - `{resource}:{action}:{scope}` - Specific action on scoped instances

use std::{collections::HashMap, fmt::Display};

/// Represents all possible API permissions
#[derive(Debug, Clone, PartialEq)]
pub enum ApiPermission {
    // Health & System
    HealthGet,
    MetricsGet,
    MetricsDebug,

    // Relayers
    RelayersList,
    RelayersGetId(String),
    RelayersCreate,
    RelayersUpdateId(String),
    RelayersDeleteId(String),
    RelayersAdmin,

    // Transactions
    TransactionsSubmitId(String),
    TransactionsGetId(String),
    TransactionsDeleteId(String),
    TransactionsAdmin,

    // Signing
    SigningExecuteId(String),
    SigningAdmin,

    // Signers
    SignersList,
    SignersGetId(String),
    SignersCreate,
    SignersUpdateId(String),
    SignersDeleteId(String),
    SignersAdmin,

    // Notifications
    NotificationsList,
    NotificationsGetId(String),
    NotificationsCreate,
    NotificationsUpdateId(String),
    NotificationsDeleteId(String),
    NotificationsAdmin,

    // Plugins
    PluginsList,
    PluginsExecuteId(String),
    PluginsAdmin,

    // API Keys
    ApiKeysList,
    ApiKeysGetId(String),
    ApiKeysCreate,
    ApiKeysDeleteId(String),
    ApiKeysAdmin,

    // Super Admin
    SuperAdmin,
}

impl Display for ApiPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Health & System
            Self::HealthGet => write!(f, "health:get:all"),
            Self::MetricsGet => write!(f, "metrics:get:all"),
            Self::MetricsDebug => write!(f, "metrics:debug:all"),

            // Relayers
            Self::RelayersList => write!(f, "relayers:get:all"),
            Self::RelayersGetId(id) => write!(f, "relayers:get:{}", id),
            Self::RelayersCreate => write!(f, "relayers:create:all"),
            Self::RelayersUpdateId(id) => write!(f, "relayers:update:{}", id),
            Self::RelayersDeleteId(id) => write!(f, "relayers:delete:{}", id),
            Self::RelayersAdmin => write!(f, "relayers:*:*"),

            // Transactions
            Self::TransactionsSubmitId(id) => write!(f, "transactions:execute:{}", id),
            Self::TransactionsGetId(id) => write!(f, "transactions:get:{}", id),
            Self::TransactionsDeleteId(id) => write!(f, "transactions:delete:{}", id),
            Self::TransactionsAdmin => write!(f, "transactions:*:*"),

            // Signing
            Self::SigningExecuteId(id) => write!(f, "signing:execute:{}", id),
            Self::SigningAdmin => write!(f, "signing:*:*"),

            // Signers
            Self::SignersList => write!(f, "signers:get:all"),
            Self::SignersGetId(id) => write!(f, "signers:get:{}", id),
            Self::SignersCreate => write!(f, "signers:create:all"),
            Self::SignersUpdateId(id) => write!(f, "signers:update:{}", id),
            Self::SignersDeleteId(id) => write!(f, "signers:delete:{}", id),
            Self::SignersAdmin => write!(f, "signers:*:*"),

            // Notifications
            Self::NotificationsList => write!(f, "notifications:get:all"),
            Self::NotificationsGetId(id) => write!(f, "notifications:get:{}", id),
            Self::NotificationsCreate => write!(f, "notifications:create:all"),
            Self::NotificationsUpdateId(id) => write!(f, "notifications:update:{}", id),
            Self::NotificationsDeleteId(id) => write!(f, "notifications:delete:{}", id),
            Self::NotificationsAdmin => write!(f, "notifications:*:*"),

            // Plugins
            Self::PluginsList => write!(f, "plugins:get:all"),
            Self::PluginsExecuteId(id) => write!(f, "plugins:execute:{}", id),
            Self::PluginsAdmin => write!(f, "plugins:*:*"),

            // API Keys
            Self::ApiKeysList => write!(f, "api_keys:get:all"),
            Self::ApiKeysGetId(id) => write!(f, "api_keys:get:{}", id),
            Self::ApiKeysCreate => write!(f, "api_keys:create:all"),
            Self::ApiKeysDeleteId(id) => write!(f, "api_keys:delete:{}", id),
            Self::ApiKeysAdmin => write!(f, "api_keys:*:*"),

            // Super Admin
            Self::SuperAdmin => write!(f, "*:*:*"),
        }
    }
}

// =============================================================================
// PERMISSION VALIDATION HELPERS
// =============================================================================

/// Check if a permission grants access to a specific operation
///
/// This function supports wildcard matching:
/// - `*:*:*` matches everything
/// - `relayers:*:*` matches all relayer operations
/// - `relayers:get:*` matches all relayer get operations
///
/// # Arguments
/// * `granted_permission` - The permission granted to the API key
/// * `required_permission` - The permission required for the operation
///
/// # Returns
/// `true` if the granted permission covers the required permission
pub fn permission_grants_access(granted_permission: &str, required_permission: &str) -> bool {
    if granted_permission == ApiPermission::SuperAdmin.to_string() {
        return true;
    }

    if granted_permission == required_permission {
        return true;
    }

    let granted_parts: Vec<&str> = granted_permission.split(':').collect();
    let required_parts: Vec<&str> = required_permission.split(':').collect();

    if granted_parts.len() != 3 || required_parts.len() != 3 {
        return false;
    }

    // Check each part with wildcard support
    for i in 0..3 {
        if granted_parts[i] != "*" && granted_parts[i] != required_parts[i] {
            return false;
        }
    }

    true
}

/// Check if an API key has the required permission
///
/// # Arguments
/// * `api_key_permissions` - List of permissions granted to the API key
/// * `required_permission` - The permission required for the operation
///
/// # Returns
/// `true` if the API key has the required permission
pub fn has_permission(api_key_permissions: &[String], required_permission: &str) -> bool {
    api_key_permissions
        .iter()
        .any(|granted| permission_grants_access(granted, required_permission))
}

/// Check if an API key has the required permission with dynamic parameter substitution
///
/// This function handles permission templates with {param_name} placeholders by substituting
/// them with actual parameter values before checking permissions.
///
/// # Arguments
/// * `api_key_permissions` - List of permissions granted to the API key
/// * `required_permission` - The permission template (e.g., "relayers:write:{relayer_id}")
/// * `param_values` - Map of parameter names to their values (e.g., "relayer_id" -> "abc-123")
///
/// # Returns
/// `true` if the API key has the required permission with parameters substituted
pub fn has_permission_with_params(
    api_key_permissions: &[String],
    required_permission: &str,
    param_values: &HashMap<String, String>,
) -> bool {
    // If no parameters in the permission string, use regular check
    if !required_permission.contains('{') {
        return has_permission(api_key_permissions, required_permission);
    }

    // Substitute parameters in the required permission
    let mut substituted_permission = required_permission.to_string();
    for (param_name, param_value) in param_values {
        let placeholder = format!("{{{}}}", param_name);
        substituted_permission = substituted_permission.replace(&placeholder, param_value);
    }

    tracing::debug!(
        "Permission check: template='{}', substituted='{}', api_key_permissions={:?}",
        required_permission,
        substituted_permission,
        api_key_permissions
    );

    // Check if API key has the substituted permission
    has_permission(api_key_permissions, &substituted_permission)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_super_admin_grants_all_access() {
        let super_admin = ApiPermission::SuperAdmin.to_string();
        let relayers_list = ApiPermission::RelayersList.to_string();
        let transactions_submit =
            ApiPermission::TransactionsSubmitId("test-relayer".to_string()).to_string();

        assert!(permission_grants_access(&super_admin, &relayers_list));
        assert!(permission_grants_access(&super_admin, &transactions_submit));
        assert!(permission_grants_access(
            &super_admin,
            "custom:permission:test"
        ));
    }

    #[test]
    fn test_exact_permission_match() {
        let relayers_list = ApiPermission::RelayersList.to_string();
        let relayers_get_id = ApiPermission::RelayersGetId("test-id".to_string()).to_string();

        assert!(permission_grants_access(&relayers_list, &relayers_list));
        assert!(!permission_grants_access(&relayers_list, &relayers_get_id));
    }

    #[test]
    fn test_wildcard_resource_match() {
        let relayers_admin = ApiPermission::RelayersAdmin.to_string();
        let relayers_list = ApiPermission::RelayersList.to_string();
        let relayers_create = ApiPermission::RelayersCreate.to_string();
        let relayers_delete = ApiPermission::RelayersDeleteId("test-id".to_string()).to_string();
        let transactions_submit =
            ApiPermission::TransactionsSubmitId("test-id".to_string()).to_string();

        assert!(permission_grants_access(&relayers_admin, &relayers_list));
        assert!(permission_grants_access(&relayers_admin, &relayers_create));
        assert!(permission_grants_access(&relayers_admin, &relayers_delete));
        assert!(!permission_grants_access(
            &relayers_admin,
            &transactions_submit
        ));
    }

    #[test]
    fn test_wildcard_action_match() {
        let relayers_list = ApiPermission::RelayersList.to_string();
        let relayers_get_id = ApiPermission::RelayersGetId("test-id".to_string()).to_string();
        let relayers_create = ApiPermission::RelayersCreate.to_string();

        assert!(permission_grants_access("relayers:get:*", &relayers_list));
        assert!(permission_grants_access("relayers:get:*", &relayers_get_id));
        assert!(!permission_grants_access(
            "relayers:get:*",
            &relayers_create
        ));
    }

    #[test]
    fn test_has_permission_with_multiple_grants() {
        let relayers_list = ApiPermission::RelayersList.to_string();
        let transactions_submit =
            ApiPermission::TransactionsSubmitId("test-relayer".to_string()).to_string();
        let relayers_create = ApiPermission::RelayersCreate.to_string();

        let permissions = vec![relayers_list.clone(), transactions_submit.clone()];

        assert!(has_permission(&permissions, &relayers_list));
        assert!(has_permission(&permissions, &transactions_submit));
        assert!(!has_permission(&permissions, &relayers_create));
    }

    #[test]
    fn test_has_permission_with_wildcard() {
        let relayers_admin = ApiPermission::RelayersAdmin.to_string();
        let relayers_list = ApiPermission::RelayersList.to_string();
        let relayers_create = ApiPermission::RelayersCreate.to_string();
        let relayers_delete = ApiPermission::RelayersDeleteId("test-id".to_string()).to_string();
        let transactions_submit =
            ApiPermission::TransactionsSubmitId("test-id".to_string()).to_string();

        let permissions = vec![relayers_admin];

        assert!(has_permission(&permissions, &relayers_list));
        assert!(has_permission(&permissions, &relayers_create));
        assert!(has_permission(&permissions, &relayers_delete));
        assert!(!has_permission(&permissions, &transactions_submit));
    }

    #[test]
    fn test_has_permission_with_params_basic() {
        let mut param_values = HashMap::new();
        param_values.insert("relayer_id".to_string(), "test-relayer-123".to_string());

        let template_permission = "relayers:get:{relayer_id}";

        // Test exact match with parameter substitution
        let permissions = vec!["relayers:get:test-relayer-123".to_string()];
        assert!(has_permission_with_params(
            &permissions,
            template_permission,
            &param_values
        ));

        // Test no match with different relayer ID
        let permissions = vec!["relayers:get:different-relayer".to_string()];
        assert!(!has_permission_with_params(
            &permissions,
            template_permission,
            &param_values
        ));

        // Test wildcard match
        let permissions = vec!["relayers:get:*".to_string()];
        assert!(has_permission_with_params(
            &permissions,
            template_permission,
            &param_values
        ));
    }

    #[test]
    fn test_has_permission_with_params_multiple_resources() {
        let mut param_values = HashMap::new();
        param_values.insert("relayer_id".to_string(), "relayer-abc".to_string());

        let permissions = vec![
            "relayers:get:relayer-abc".to_string(),
            "transactions:execute:relayer-abc".to_string(),
            "signing:execute:relayer-abc".to_string(),
        ];

        // Test relayer permissions
        assert!(has_permission_with_params(
            &permissions,
            "relayers:get:{relayer_id}",
            &param_values
        ));

        // Test transaction permissions
        assert!(has_permission_with_params(
            &permissions,
            "transactions:execute:{relayer_id}",
            &param_values
        ));

        // Test signing permissions
        assert!(has_permission_with_params(
            &permissions,
            "signing:execute:{relayer_id}",
            &param_values
        ));

        // Test permissions for different relayer should fail
        param_values.insert("relayer_id".to_string(), "different-relayer".to_string());
        assert!(!has_permission_with_params(
            &permissions,
            "relayers:get:{relayer_id}",
            &param_values
        ));
        assert!(!has_permission_with_params(
            &permissions,
            "transactions:execute:{relayer_id}",
            &param_values
        ));
    }

    #[test]
    fn test_has_permission_with_params_no_templates() {
        let param_values = HashMap::new();

        // Test permissions without templates should work normally
        let relayers_list = ApiPermission::RelayersList.to_string();
        let relayers_create = ApiPermission::RelayersCreate.to_string();
        let permissions = vec![relayers_list.clone(), relayers_create.clone()];

        assert!(has_permission_with_params(
            &permissions,
            &relayers_list,
            &param_values
        ));
        assert!(has_permission_with_params(
            &permissions,
            &relayers_create,
            &param_values
        ));
        assert!(!has_permission_with_params(
            &permissions,
            "relayers:get:specific-id",
            &param_values
        ));
    }

    #[test]
    fn test_has_permission_with_params_plugin_scoping() {
        let mut param_values = HashMap::new();
        param_values.insert("plugin_id".to_string(), "my-plugin-123".to_string());

        // Test plugin-specific permissions
        let permissions = vec![
            "plugins:execute:my-plugin-123".to_string(),
            "plugins:get:all".to_string(),
        ];

        assert!(has_permission_with_params(
            &permissions,
            "plugins:execute:{plugin_id}",
            &param_values
        ));
        assert!(has_permission_with_params(
            &permissions,
            "plugins:get:all",
            &param_values
        ));

        // Test different plugin ID should fail
        param_values.insert("plugin_id".to_string(), "different-plugin".to_string());
        assert!(!has_permission_with_params(
            &permissions,
            "plugins:execute:{plugin_id}",
            &param_values
        ));
    }

    #[test]
    fn test_has_permission_with_params_admin_wildcard() {
        let mut param_values = HashMap::new();
        param_values.insert("relayer_id".to_string(), "any-relayer".to_string());

        // Test that admin permissions work with any relayer ID
        let relayers_admin = ApiPermission::RelayersAdmin.to_string();
        let permissions = vec![relayers_admin];

        assert!(has_permission_with_params(
            &permissions,
            "relayers:get:{relayer_id}",
            &param_values
        ));
        assert!(has_permission_with_params(
            &permissions,
            "relayers:update:{relayer_id}",
            &param_values
        ));
        assert!(has_permission_with_params(
            &permissions,
            "relayers:delete:{relayer_id}",
            &param_values
        ));

        // Change relayer ID and verify it still works
        param_values.insert(
            "relayer_id".to_string(),
            "completely-different-relayer".to_string(),
        );
        assert!(has_permission_with_params(
            &permissions,
            "relayers:get:{relayer_id}",
            &param_values
        ));
        assert!(has_permission_with_params(
            &permissions,
            "relayers:update:{relayer_id}",
            &param_values
        ));
    }

    /// Test that simulates how the macro processes permission validation
    /// This simulates the exact flow: parameter extraction -> template substitution -> permission check
    #[test]
    fn test_macro_simulation_with_scoped_permissions() {
        // Simulate API key with scoped permission for specific relayer
        let api_key_permissions = vec!["relayers:get:sepolia-example".to_string()];

        // Simulate the macro extracting relayer_id parameter from web::Path
        let relayer_id_from_path = "sepolia-example";

        // Simulate the macro building the parameter map
        let mut param_values = HashMap::new();
        param_values.insert("relayer_id".to_string(), relayer_id_from_path.to_string());

        // Simulate the macro checking against the template permission
        let required_permission_template = "relayers:get:{relayer_id}";

        // This should pass because:
        // 1. Template: "relayers:get:{relayer_id}"
        // 2. Parameter substitution: "relayers:get:sepolia-example"
        // 3. API key has: "relayers:get:sepolia-example"
        assert!(has_permission_with_params(
            &api_key_permissions,
            required_permission_template,
            &param_values
        ));

        // Test with different relayer_id should fail
        param_values.insert("relayer_id".to_string(), "different-relayer".to_string());
        assert!(!has_permission_with_params(
            &api_key_permissions,
            required_permission_template,
            &param_values
        ));

        // Test with wildcard permission should work for any relayer_id
        let wildcard_permissions = vec!["relayers:get:*".to_string()];
        assert!(has_permission_with_params(
            &wildcard_permissions,
            required_permission_template,
            &param_values
        ));

        // Test with admin permission should work for any relayer_id
        let admin_permissions = vec![ApiPermission::RelayersAdmin.to_string()]; // "relayers:*:*"
        assert!(has_permission_with_params(
            &admin_permissions,
            required_permission_template,
            &param_values
        ));
    }

    /// Test multiple parameter extraction (like TransactionPath with relayer_id + transaction_id)
    #[test]
    fn test_macro_simulation_multiple_params() {
        // API key with scoped permission for specific relayer
        let api_key_permissions = vec!["transactions:get:sepolia-example".to_string()];

        // Simulate extracting multiple parameters from path
        let mut param_values = HashMap::new();
        param_values.insert("relayer_id".to_string(), "sepolia-example".to_string());
        param_values.insert("transaction_id".to_string(), "tx-123".to_string());

        // Test transaction permission (only uses relayer_id parameter)
        let required_permission = "transactions:get:{relayer_id}"; // "transactions:get:{relayer_id}"

        assert!(has_permission_with_params(
            &api_key_permissions,
            required_permission,
            &param_values
        ));

        // Even though we have transaction_id in params, it shouldn't affect the result
        // since the permission template only uses relayer_id
        param_values.insert("transaction_id".to_string(), "different-tx".to_string());
        assert!(has_permission_with_params(
            &api_key_permissions,
            required_permission,
            &param_values
        ));
    }
}
