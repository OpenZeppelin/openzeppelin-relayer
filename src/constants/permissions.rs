//! # API Permissions Constants
//!
//! This module defines all available permissions for API key-based access control.
//! Permissions follow the format: `{resource}:{action}:{scope}`
//!
//! ## Permission Format
//! - **resource**: The API resource being accessed (relayers, transactions, etc.)
//! - **action**: The operation being performed (read, write, execute, etc.)
//! - **scope**: The scope of access (all, owned, specific_id, etc.)
//!
//! ## Permission Hierarchy
//! - `*:*:*` - Full system access (super admin)
//! - `{resource}:*:*` - Full access to a specific resource
//! - `{resource}:{action}:*` - Specific action on all instances of a resource
//! - `{resource}:{action}:{scope}` - Specific action on scoped instances

// =============================================================================
// HEALTH & SYSTEM PERMISSIONS
// =============================================================================

/// Permission to access health check endpoints
pub const HEALTH_READ: &str = "health:read:all";

/// Permission to access system metrics
pub const METRICS_READ: &str = "metrics:read:all";

/// Permission to access specific metric by name
pub const METRICS_READ_SPECIFIC: &str = "metrics:read:specific";

/// Permission to access debug metrics endpoints
pub const METRICS_DEBUG: &str = "metrics:debug:all";

// =============================================================================
// RELAYER PERMISSIONS
// =============================================================================

/// Permission to list all relayers
pub const RELAYERS_LIST: &str = "relayers:read:all";

/// Permission to read a specific relayer's details
pub const RELAYERS_READ: &str = "relayers:read:specific";

/// Permission to create new relayers
pub const RELAYERS_CREATE: &str = "relayers:write:create";

/// Permission to update relayer configuration
pub const RELAYERS_UPDATE: &str = "relayers:write:update";

/// Permission to delete relayers
pub const RELAYERS_DELETE: &str = "relayers:write:delete";

/// Full relayer management access (all operations)
pub const RELAYERS_ADMIN: &str = "relayers:*:*";

// =============================================================================
// TRANSACTION PERMISSIONS
// =============================================================================

/// Permission to submit new transactions
pub const TRANSACTIONS_SUBMIT: &str = "transactions:execute:submit";

/// Permission to read transaction details
pub const TRANSACTIONS_READ: &str = "transactions:read:specific";

/// Permission to list transactions for a relayer
pub const TRANSACTIONS_LIST: &str = "transactions:read:list";

/// Permission to delete specific transactions
pub const TRANSACTIONS_DELETE: &str = "transactions:write:delete";

/// Full transaction management access
pub const TRANSACTIONS_ADMIN: &str = "transactions:*:*";

// =============================================================================
// SIGNING PERMISSIONS
// =============================================================================

/// Permission to sign arbitrary data
pub const SIGN_DATA: &str = "signing:execute:data";

/// Full signing access
pub const SIGNING_ADMIN: &str = "signing:*:*";

// =============================================================================
// SIGNER PERMISSIONS
// =============================================================================

/// Permission to list all signers
pub const SIGNERS_LIST: &str = "signers:read:all";

/// Permission to read specific signer details
pub const SIGNERS_READ: &str = "signers:read:specific";

/// Permission to create new signers
pub const SIGNERS_CREATE: &str = "signers:write:create";

/// Permission to update signer configuration
pub const SIGNERS_UPDATE: &str = "signers:write:update";

/// Permission to delete signers
pub const SIGNERS_DELETE: &str = "signers:write:delete";

/// Full signer management access
pub const SIGNERS_ADMIN: &str = "signers:*:*";

// =============================================================================
// NOTIFICATION PERMISSIONS
// =============================================================================

/// Permission to list all notifications
pub const NOTIFICATIONS_LIST: &str = "notifications:read:all";

/// Permission to read specific notification details
pub const NOTIFICATIONS_READ: &str = "notifications:read:specific";

/// Permission to create new notifications
pub const NOTIFICATIONS_CREATE: &str = "notifications:write:create";

/// Permission to update notification configuration
pub const NOTIFICATIONS_UPDATE: &str = "notifications:write:update";

/// Permission to delete notifications
pub const NOTIFICATIONS_DELETE: &str = "notifications:write:delete";

/// Full notification management access
pub const NOTIFICATIONS_ADMIN: &str = "notifications:*:*";

// =============================================================================
// PLUGIN PERMISSIONS
// =============================================================================

/// Permission to list all plugins
pub const PLUGINS_LIST: &str = "plugins:read:all";

/// Permission to call/execute plugin functions
pub const PLUGINS_EXECUTE: &str = "plugins:execute:call";

/// Full plugin access
pub const PLUGINS_ADMIN: &str = "plugins:*:*";

// =============================================================================
// API KEY PERMISSIONS
// =============================================================================

/// Permission to list all API keys
pub const API_KEYS_LIST: &str = "api_keys:read:all";

/// Permission to read API key permissions
pub const API_KEYS_READ: &str = "api_keys:read:specific";

/// Permission to create new API keys
pub const API_KEYS_CREATE: &str = "api_keys:write:create";

/// Permission to delete API keys
pub const API_KEYS_DELETE: &str = "api_keys:write:delete";

/// Full API key management access
pub const API_KEYS_ADMIN: &str = "api_keys:*:*";

// =============================================================================
// SUPER ADMIN PERMISSIONS
// =============================================================================

/// Super admin permission - grants access to everything
pub const SUPER_ADMIN: &str = "*:*:*";

// =============================================================================
// PERMISSION VALIDATION HELPERS
// =============================================================================

/// Check if a permission grants access to a specific operation
///
/// This function supports wildcard matching:
/// - `*:*:*` matches everything
/// - `relayers:*:*` matches all relayer operations
/// - `relayers:read:*` matches all relayer read operations
///
/// # Arguments
/// * `granted_permission` - The permission granted to the API key
/// * `required_permission` - The permission required for the operation
///
/// # Returns
/// `true` if the granted permission covers the required permission
pub fn permission_grants_access(granted_permission: &str, required_permission: &str) -> bool {
    if granted_permission == SUPER_ADMIN {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_super_admin_grants_all_access() {
        assert!(permission_grants_access(SUPER_ADMIN, RELAYERS_LIST));
        assert!(permission_grants_access(SUPER_ADMIN, TRANSACTIONS_SUBMIT));
        assert!(permission_grants_access(
            SUPER_ADMIN,
            "custom:permission:test"
        ));
    }

    #[test]
    fn test_exact_permission_match() {
        assert!(permission_grants_access(RELAYERS_LIST, RELAYERS_LIST));
        assert!(!permission_grants_access(RELAYERS_LIST, RELAYERS_READ));
    }

    #[test]
    fn test_wildcard_resource_match() {
        assert!(permission_grants_access(RELAYERS_ADMIN, RELAYERS_LIST));
        assert!(permission_grants_access(RELAYERS_ADMIN, RELAYERS_CREATE));
        assert!(permission_grants_access(RELAYERS_ADMIN, RELAYERS_DELETE));
        assert!(!permission_grants_access(
            RELAYERS_ADMIN,
            TRANSACTIONS_SUBMIT
        ));
    }

    #[test]
    fn test_wildcard_action_match() {
        assert!(permission_grants_access("relayers:read:*", RELAYERS_LIST));
        assert!(permission_grants_access("relayers:read:*", RELAYERS_READ));
        assert!(!permission_grants_access(
            "relayers:read:*",
            RELAYERS_CREATE
        ));
    }

    #[test]
    fn test_has_permission_with_multiple_grants() {
        let permissions = vec![RELAYERS_LIST.to_string(), TRANSACTIONS_SUBMIT.to_string()];

        assert!(has_permission(&permissions, RELAYERS_LIST));
        assert!(has_permission(&permissions, TRANSACTIONS_SUBMIT));
        assert!(!has_permission(&permissions, RELAYERS_CREATE));
    }

    #[test]
    fn test_has_permission_with_wildcard() {
        let permissions = vec![RELAYERS_ADMIN.to_string()];

        assert!(has_permission(&permissions, RELAYERS_LIST));
        assert!(has_permission(&permissions, RELAYERS_CREATE));
        assert!(has_permission(&permissions, RELAYERS_DELETE));
        assert!(!has_permission(&permissions, TRANSACTIONS_SUBMIT));
    }

    #[test]
    fn test_permission_groups() {
        // Test that permission groups contain expected permissions
        assert!(READ_ONLY_PERMISSIONS.contains(&RELAYERS_LIST));
        assert!(READ_ONLY_PERMISSIONS.contains(&TRANSACTIONS_READ));
        assert!(!READ_ONLY_PERMISSIONS.contains(&RELAYERS_CREATE));

        assert!(TRANSACTION_EXECUTOR_PERMISSIONS.contains(&TRANSACTIONS_SUBMIT));

        assert!(ADMIN_PERMISSIONS.contains(&RELAYERS_ADMIN));
        assert!(ADMIN_PERMISSIONS.contains(&TRANSACTIONS_ADMIN));
    }
}
