//! Permission model for API key authorization
//!
//! This module defines the permission grant system used to control access to API resources.
//! Permissions consist of an action (e.g., "relayers:read") and a scope (global or specific IDs).

use serde::{Deserialize, Serialize};
use std::fmt;

/// A permission grant that specifies what action can be performed and on what scope
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionGrant {
    /// The action being permitted (e.g., "relayers:read", "transactions:execute", "relayers:*")
    pub action: String,
    /// The scope of the permission (global or specific resource IDs)
    #[serde(flatten)]
    pub scope: PermissionScope,
}

/// Defines the scope of a permission grant
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PermissionScope {
    /// Permission applies globally to all resources of the action's type
    Global { scope: GlobalScope },
    /// Permission applies only to specific resource IDs
    Ids { scope: Vec<String> },
}

/// Marker type for global scope in untagged serialization
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlobalScope {
    Global,
}

impl PermissionGrant {
    /// Creates a new permission grant with global scope
    pub fn global(action: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            scope: PermissionScope::Global {
                scope: GlobalScope::Global,
            },
        }
    }

    /// Creates a new permission grant with specific resource IDs
    pub fn ids(action: impl Into<String>, ids: Vec<String>) -> Self {
        Self {
            action: action.into(),
            scope: PermissionScope::Ids { scope: ids },
        }
    }

    /// Checks if this grant has global scope
    pub fn is_global(&self) -> bool {
        matches!(self.scope, PermissionScope::Global { .. })
    }

    /// Gets the list of IDs if this is a scoped permission
    pub fn get_ids(&self) -> Option<&[String]> {
        match &self.scope {
            PermissionScope::Global { .. } => None,
            PermissionScope::Ids { scope } => Some(scope),
        }
    }
}

impl fmt::Display for PermissionGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.scope {
            PermissionScope::Global { .. } => write!(f, "{} (global)", self.action),
            PermissionScope::Ids { scope } => {
                write!(f, "{} (ids: {})", self.action, scope.join(", "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_grant_global() {
        let grant = PermissionGrant::global("relayers:read");
        assert_eq!(grant.action, "relayers:read");
        assert!(grant.is_global());
        assert!(grant.get_ids().is_none());
    }

    #[test]
    fn test_permission_grant_ids() {
        let grant =
            PermissionGrant::ids("relayers:read", vec!["id1".to_string(), "id2".to_string()]);
        assert_eq!(grant.action, "relayers:read");
        assert!(!grant.is_global());
        assert_eq!(
            grant.get_ids(),
            Some(&["id1".to_string(), "id2".to_string()][..])
        );
    }

    #[test]
    fn test_serialize_global() {
        let grant = PermissionGrant::global("relayers:read");
        let json = serde_json::to_string(&grant).unwrap();
        assert_eq!(json, r#"{"action":"relayers:read","scope":"global"}"#);
    }

    #[test]
    fn test_serialize_ids() {
        let grant = PermissionGrant::ids("relayers:update", vec!["sepolia-example".to_string()]);
        let json = serde_json::to_string(&grant).unwrap();
        assert_eq!(
            json,
            r#"{"action":"relayers:update","scope":["sepolia-example"]}"#
        );
    }

    #[test]
    fn test_deserialize_global() {
        let json = r#"{"action":"relayers:read","scope":"global"}"#;
        let grant: PermissionGrant = serde_json::from_str(json).unwrap();
        assert_eq!(grant.action, "relayers:read");
        assert!(grant.is_global());
    }

    #[test]
    fn test_deserialize_ids() {
        let json = r#"{"action":"relayers:update","scope":["id1","id2"]}"#;
        let grant: PermissionGrant = serde_json::from_str(json).unwrap();
        assert_eq!(grant.action, "relayers:update");
        assert_eq!(
            grant.get_ids(),
            Some(&["id1".to_string(), "id2".to_string()][..])
        );
    }

    #[test]
    fn test_display_global() {
        let grant = PermissionGrant::global("relayers:read");
        assert_eq!(grant.to_string(), "relayers:read (global)");
    }

    #[test]
    fn test_display_ids() {
        let grant = PermissionGrant::ids(
            "relayers:update",
            vec!["id1".to_string(), "id2".to_string()],
        );
        assert_eq!(grant.to_string(), "relayers:update (ids: id1, id2)");
    }
}
