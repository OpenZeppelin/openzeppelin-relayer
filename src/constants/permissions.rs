//! # API Permissions System
//!
//! This module defines permission validation logic for API key-based access control.
//! Permissions consist of an action and a scope (global or specific IDs).
//!
//! ## Permission Format
//! Permissions are represented as `PermissionGrant` structs:
//! - **action**: The operation (e.g., "relayers:read", "transactions:execute")
//! - **scope**: Either Global or Ids(vec![...])
//!
//! ## Permission Hierarchy
//! - Action `*:*` with Global scope - Full system access (super admin)
//! - Action `{resource}:*` with Global scope - Full access to a specific resource
//! - Action `{resource}:{action}` with Global scope - Specific action on all instances
//! - Action `{resource}:{action}` with Ids scope - Specific action on specific instances

use crate::models::PermissionGrant;

/// Check if a permission grant allows access to a specific action
///
/// This function supports wildcard matching in actions:
/// - `*:*` matches all actions
/// - `relayers:*` matches all relayer actions
///
/// # Arguments
/// * `grant` - The permission grant from the API key
/// * `required_action` - The action required for the operation (e.g., "relayers:read")
/// * `id` - Optional resource ID being accessed
///
/// # Returns
/// `true` if the grant allows the required action with the given ID
pub fn permission_grant_allows_action(
    grant: &PermissionGrant,
    required_action: &str,
    id: Option<&str>,
) -> bool {
    // Check if action matches (with wildcard support)
    if !action_matches(&grant.action, required_action) {
        return false;
    }

    // Check scope
    match id {
        None => {
            // No ID specified - only global grants apply
            grant.is_global()
        }
        Some(requested_id) => {
            // ID specified - check if grant covers it
            if grant.is_global() {
                // Global grant covers all IDs
                true
            } else if let Some(ids) = grant.get_ids() {
                // Check if the requested ID is in the grant's ID list
                ids.contains(&requested_id.to_string())
            } else {
                false
            }
        }
    }
}

/// Check if an action matches a required action (with wildcard support)
///
/// Supports wildcard matching:
/// - `*:*` matches any action
/// - `relayers:*` matches any action on relayers resource
///
/// # Arguments
/// * `granted_action` - The action from the permission grant
/// * `required_action` - The action required for the operation
///
/// # Returns
/// `true` if the granted action covers the required action
fn action_matches(granted_action: &str, required_action: &str) -> bool {
    // Super admin wildcard
    if granted_action == "*:*" {
        return true;
    }

    // Exact match
    if granted_action == required_action {
        return true;
    }

    // Wildcard matching (e.g., "relayers:*" matches "relayers:read")
    let granted_parts: Vec<&str> = granted_action.split(':').collect();
    let required_parts: Vec<&str> = required_action.split(':').collect();

    if granted_parts.len() != required_parts.len() {
        return false;
    }

    // Check each part with wildcard support
    for i in 0..granted_parts.len() {
        if granted_parts[i] != "*" && granted_parts[i] != required_parts[i] {
            return false;
        }
    }

    true
}

/// Check if an API key has the required permission for a global-scoped action
///
/// Use this for actions that don't require a specific resource ID (e.g., listing all relayers).
///
/// # Arguments
/// * `grants` - List of permission grants from the API key
/// * `required_action` - The action required (e.g., "relayers:read")
///
/// # Returns
/// `true` if the API key has the required permission with global scope
pub fn has_permission_grant(grants: &[PermissionGrant], required_action: &str) -> bool {
    grants
        .iter()
        .any(|grant| permission_grant_allows_action(grant, required_action, None))
}

/// Check if an API key has the required permission for a specific resource ID
///
/// Use this for actions on specific resources (e.g., getting/updating a specific relayer).
///
/// # Arguments
/// * `grants` - List of permission grants from the API key
/// * `required_action` - The action required (e.g., "relayers:read")
/// * `id` - The resource ID being accessed
///
/// # Returns
/// `true` if the API key has the required permission for the specific ID
pub fn has_permission_grant_for_id(
    grants: &[PermissionGrant],
    required_action: &str,
    id: &str,
) -> bool {
    grants
        .iter()
        .any(|grant| permission_grant_allows_action(grant, required_action, Some(id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PermissionGrant;

    #[test]
    fn test_action_matches_exact() {
        assert!(action_matches("relayers:read", "relayers:read"));
        assert!(!action_matches("relayers:read", "relayers:update"));
    }

    #[test]
    fn test_action_matches_wildcard_super_admin() {
        assert!(action_matches("*:*", "relayers:read"));
        assert!(action_matches("*:*", "transactions:execute"));
        assert!(action_matches("*:*", "anything:action"));
    }

    #[test]
    fn test_action_matches_wildcard_resource() {
        assert!(action_matches("relayers:*", "relayers:read"));
        assert!(action_matches("relayers:*", "relayers:update"));
        assert!(!action_matches("relayers:*", "transactions:execute"));
    }

    #[test]
    fn test_permission_grant_global_no_id() {
        let grant = PermissionGrant::global("relayers:read");
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:read",
            None
        ));
        assert!(!permission_grant_allows_action(
            &grant,
            "relayers:update",
            None
        ));
    }

    #[test]
    fn test_permission_grant_global_with_id() {
        let grant = PermissionGrant::global("relayers:read");
        // Global grants allow access to any ID
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:read",
            Some("sepolia-example")
        ));
    }

    #[test]
    fn test_permission_grant_scoped_no_id() {
        let grant = PermissionGrant::ids("relayers:read", vec!["sepolia-example".to_string()]);
        // Scoped grants don't apply to global requests (no ID specified)
        assert!(!permission_grant_allows_action(
            &grant,
            "relayers:read",
            None
        ));
    }

    #[test]
    fn test_permission_grant_scoped_matching_id() {
        let grant = PermissionGrant::ids("relayers:update", vec!["sepolia-example".to_string()]);
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:update",
            Some("sepolia-example")
        ));
    }

    #[test]
    fn test_permission_grant_scoped_non_matching_id() {
        let grant = PermissionGrant::ids("relayers:update", vec!["sepolia-example".to_string()]);
        assert!(!permission_grant_allows_action(
            &grant,
            "relayers:update",
            Some("mainnet-example")
        ));
    }

    #[test]
    fn test_permission_grant_scoped_multiple_ids() {
        let grant = PermissionGrant::ids(
            "relayers:update",
            vec!["sepolia-example".to_string(), "mainnet-example".to_string()],
        );
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:update",
            Some("sepolia-example")
        ));
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:update",
            Some("mainnet-example")
        ));
        assert!(!permission_grant_allows_action(
            &grant,
            "relayers:update",
            Some("polygon-example")
        ));
    }

    #[test]
    fn test_permission_grant_wildcard_action_global() {
        let grant = PermissionGrant::global("relayers:*");
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:read",
            None
        ));
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:update",
            None
        ));
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:delete",
            None
        ));
        assert!(!permission_grant_allows_action(
            &grant,
            "transactions:execute",
            None
        ));
    }

    #[test]
    fn test_permission_grant_super_admin() {
        let grant = PermissionGrant::global("*:*");
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:read",
            None
        ));
        assert!(permission_grant_allows_action(
            &grant,
            "relayers:read",
            Some("any-id")
        ));
        assert!(permission_grant_allows_action(
            &grant,
            "transactions:execute",
            None
        ));
        assert!(permission_grant_allows_action(
            &grant,
            "transactions:execute",
            Some("any-id")
        ));
    }

    #[test]
    fn test_has_permission_grant_global() {
        let grants = vec![
            PermissionGrant::global("relayers:read"),
            PermissionGrant::ids("relayers:update", vec!["sepolia-example".to_string()]),
        ];

        assert!(has_permission_grant(&grants, "relayers:read"));
        assert!(!has_permission_grant(&grants, "relayers:update")); // Only scoped grant exists
        assert!(!has_permission_grant(&grants, "relayers:delete"));
    }

    #[test]
    fn test_has_permission_grant_for_id() {
        let grants = vec![
            PermissionGrant::global("relayers:read"),
            PermissionGrant::ids("relayers:update", vec!["sepolia-example".to_string()]),
        ];

        // Global grant covers any ID
        assert!(has_permission_grant_for_id(
            &grants,
            "relayers:read",
            "sepolia-example"
        ));
        assert!(has_permission_grant_for_id(
            &grants,
            "relayers:read",
            "mainnet-example"
        ));

        // Scoped grant only covers specific ID
        assert!(has_permission_grant_for_id(
            &grants,
            "relayers:update",
            "sepolia-example"
        ));
        assert!(!has_permission_grant_for_id(
            &grants,
            "relayers:update",
            "mainnet-example"
        ));
    }

    #[test]
    fn test_has_permission_grant_for_id_super_admin() {
        let grants = vec![PermissionGrant::global("*:*")];

        assert!(has_permission_grant_for_id(
            &grants,
            "relayers:read",
            "any-id"
        ));
        assert!(has_permission_grant_for_id(
            &grants,
            "relayers:update",
            "any-id"
        ));
        assert!(has_permission_grant_for_id(
            &grants,
            "transactions:execute",
            "any-id"
        ));
    }

    #[test]
    fn test_has_permission_grant_empty_grants() {
        let grants: Vec<PermissionGrant> = vec![];
        assert!(!has_permission_grant(&grants, "relayers:read"));
        assert!(!has_permission_grant_for_id(
            &grants,
            "relayers:read",
            "any-id"
        ));
    }

    #[test]
    fn test_has_permission_grant_multiple_grants() {
        let grants = vec![
            PermissionGrant::global("relayers:read"),
            PermissionGrant::global("transactions:execute"),
            PermissionGrant::ids("signers:update", vec!["signer-1".to_string()]),
        ];

        assert!(has_permission_grant(&grants, "relayers:read"));
        assert!(has_permission_grant(&grants, "transactions:execute"));
        assert!(!has_permission_grant(&grants, "signers:update")); // Only scoped

        assert!(has_permission_grant_for_id(
            &grants,
            "signers:update",
            "signer-1"
        ));
        assert!(!has_permission_grant_for_id(
            &grants,
            "signers:update",
            "signer-2"
        ));
    }
}
