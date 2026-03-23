//! Plugin API integration tests
//!
//! Tests for the plugin REST API endpoints including listing, retrieving,
//! and updating plugin configurations.
//!
//! Note: Plugins are not created or deleted via the API — they are loaded from
//! configuration files at startup. These tests operate on whatever plugins are
//! configured in the running relayer instance.

use crate::integration::common::client::RelayerClient;
use serde_json::Value;
use serial_test::serial;
use tokio::time::{sleep, Duration};

const E2E_PLUGIN_ID: &str = "e2e-echo";

fn plugin_call_data(body: &str) -> Value {
    let parsed: Value =
        serde_json::from_str(body).expect("Plugin call response should be valid JSON");
    parsed
        .get("data")
        .cloned()
        .unwrap_or_else(|| serde_json::json!(null))
}

fn plugin_call_metadata(body: &str) -> Option<Value> {
    let parsed: Value =
        serde_json::from_str(body).expect("Plugin call response should be valid JSON");
    parsed.get("metadata").cloned()
}

// =============================================================================
// List Plugins Tests
// =============================================================================

/// Tests that listing plugins returns a successful response
#[tokio::test]
#[serial]
async fn test_list_plugins() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    let plugins = client
        .list_plugins(None, None)
        .await
        .expect("Failed to list plugins");

    // Verify plugin structure if any exist
    for plugin in &plugins {
        assert!(!plugin.id.is_empty(), "Plugin ID should not be empty");
        assert!(!plugin.path.is_empty(), "Plugin path should not be empty");
    }
}

/// Tests pagination for listing plugins
#[tokio::test]
#[serial]
async fn test_list_plugins_pagination() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let page1 = client
        .list_plugins(Some(1), Some(2))
        .await
        .expect("Failed to list plugins page 1");

    let page2 = client
        .list_plugins(Some(2), Some(2))
        .await
        .expect("Failed to list plugins page 2");

    // Pages should not overlap (if we have enough plugins)
    if !page1.is_empty() && !page2.is_empty() {
        let page1_ids: std::collections::HashSet<_> = page1.iter().map(|p| &p.id).collect();
        let page2_ids: std::collections::HashSet<_> = page2.iter().map(|p| &p.id).collect();
        assert!(
            page1_ids.is_disjoint(&page2_ids),
            "Page 1 and Page 2 should not have overlapping plugins"
        );
    }
}

// =============================================================================
// Get Plugin Tests
// =============================================================================

/// Tests getting a plugin by ID (uses the first plugin from list)
#[tokio::test]
#[serial]
async fn test_get_plugin() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let plugins = client
        .list_plugins(None, None)
        .await
        .expect("Failed to list plugins");

    if plugins.is_empty() {
        // No plugins configured — skip test
        return;
    }

    let plugin_id = &plugins[0].id;
    let plugin = client
        .get_plugin(plugin_id)
        .await
        .expect("Failed to get plugin");

    assert_eq!(plugin.id, *plugin_id);
    assert!(!plugin.path.is_empty());
}

/// Tests that getting a non-existent plugin returns 404
#[tokio::test]
#[serial]
async fn test_get_plugin_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client.get_plugin("nonexistent-plugin-id").await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent plugin"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}

// =============================================================================
// Update Plugin Tests
// =============================================================================

/// Tests updating a plugin's timeout and restoring it
#[tokio::test]
#[serial]
async fn test_update_plugin_timeout() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let plugins = client
        .list_plugins(None, None)
        .await
        .expect("Failed to list plugins");

    if plugins.is_empty() {
        return;
    }

    let plugin_id = &plugins[0].id;
    let original = client
        .get_plugin(plugin_id)
        .await
        .expect("Failed to get plugin");

    let original_timeout = original.timeout.as_secs();

    // Update timeout to a different value
    let new_timeout = if original_timeout == 120 { 60 } else { 120 };
    let updated = client
        .update_plugin(plugin_id, serde_json::json!({ "timeout": new_timeout }))
        .await
        .expect("Failed to update plugin timeout");

    assert_eq!(updated.id, *plugin_id);
    assert_eq!(updated.timeout.as_secs(), new_timeout);

    // Restore original timeout
    let _ = client
        .update_plugin(
            plugin_id,
            serde_json::json!({ "timeout": original_timeout }),
        )
        .await;
}

/// Tests updating a plugin's boolean flags and restoring them
#[tokio::test]
#[serial]
async fn test_update_plugin_flags() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let plugins = client
        .list_plugins(None, None)
        .await
        .expect("Failed to list plugins");

    if plugins.is_empty() {
        return;
    }

    let plugin_id = &plugins[0].id;
    let original = client
        .get_plugin(plugin_id)
        .await
        .expect("Failed to get plugin");

    // Toggle emit_logs
    let new_emit_logs = !original.emit_logs;
    let updated = client
        .update_plugin(plugin_id, serde_json::json!({ "emit_logs": new_emit_logs }))
        .await
        .expect("Failed to update plugin flags");

    assert_eq!(updated.emit_logs, new_emit_logs);

    // Restore original
    let _ = client
        .update_plugin(
            plugin_id,
            serde_json::json!({ "emit_logs": original.emit_logs }),
        )
        .await;
}

/// Tests updating a plugin's config and restoring it
#[tokio::test]
#[serial]
async fn test_update_plugin_config() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let plugins = client
        .list_plugins(None, None)
        .await
        .expect("Failed to list plugins");

    if plugins.is_empty() {
        return;
    }

    let plugin_id = &plugins[0].id;
    let original = client
        .get_plugin(plugin_id)
        .await
        .expect("Failed to get plugin");

    // Set a new config
    let updated = client
        .update_plugin(
            plugin_id,
            serde_json::json!({
                "config": { "test_key": "test_value" }
            }),
        )
        .await
        .expect("Failed to update plugin config");

    assert!(updated.config.is_some());
    let config = updated.config.as_ref().unwrap();
    assert_eq!(
        config.get("test_key"),
        Some(&serde_json::json!("test_value"))
    );

    // Restore original config
    let restore_value = match &original.config {
        Some(c) => serde_json::json!({ "config": c }),
        None => serde_json::json!({ "config": null }),
    };
    let _ = client.update_plugin(plugin_id, restore_value).await;
}

/// Tests clearing a plugin's config with null and restoring it
#[tokio::test]
#[serial]
async fn test_update_plugin_clear_config() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let plugins = client
        .list_plugins(None, None)
        .await
        .expect("Failed to list plugins");

    if plugins.is_empty() {
        return;
    }

    // Prefer the deterministic e2e plugin, then fall back to probing other plugins.
    let mut candidate_ids: Vec<String> = Vec::new();
    if plugins.iter().any(|p| p.id == E2E_PLUGIN_ID) {
        candidate_ids.push(E2E_PLUGIN_ID.to_string());
    }
    for plugin in &plugins {
        if plugin.id != E2E_PLUGIN_ID {
            candidate_ids.push(plugin.id.clone());
        }
    }

    let mut selected: Option<(String, Option<serde_json::Map<String, serde_json::Value>>)> = None;
    for plugin_id in &candidate_ids {
        let original = match client.get_plugin(plugin_id).await {
            Ok(plugin) => plugin,
            Err(_) => continue,
        };

        if client
            .update_plugin(
                plugin_id,
                serde_json::json!({ "config": { "__e2e_probe": true } }),
            )
            .await
            .is_ok()
        {
            selected = Some((plugin_id.clone(), original.config.clone()));
            break;
        }
    }

    let Some((plugin_id, original_config)) = selected else {
        // No mutable plugin in this environment, skip instead of failing.
        return;
    };

    // Clear it with null
    let _updated = client
        .update_plugin(&plugin_id, serde_json::json!({ "config": null }))
        .await
        .expect("Failed to clear plugin config");

    // Re-fetch with retries to tolerate eventual consistency in storage/cache layers.
    let mut cleared = false;
    for _ in 0..30 {
        let fetched = client
            .get_plugin(&plugin_id)
            .await
            .expect("Failed to fetch plugin after clearing config");

        cleared = match &fetched.config {
            None => true,
            Some(config) => config.is_empty() || !config.contains_key("__e2e_probe"),
        };

        if cleared {
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }

    assert!(cleared, "Cleared config should not retain probe keys");

    // Restore original config
    let restore_value = match &original_config {
        Some(c) => serde_json::json!({ "config": c }),
        None => serde_json::json!({ "config": null }),
    };
    let _ = client.update_plugin(&plugin_id, restore_value).await;
}

// =============================================================================
// Error / Validation Tests
// =============================================================================

/// Tests that updating a non-existent plugin returns 404
#[tokio::test]
#[serial]
async fn test_update_plugin_not_found() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let result = client
        .update_plugin(
            "nonexistent-plugin-id",
            serde_json::json!({ "timeout": 60 }),
        )
        .await;

    assert!(
        result.is_err(),
        "Should return an error for non-existent plugin"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("404") || error_msg.contains("not found"),
        "Error should indicate not found: {}",
        error_msg
    );
}

/// Tests that updating a plugin with timeout=0 returns 400
#[tokio::test]
#[serial]
async fn test_update_plugin_invalid_timeout() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let plugins = client
        .list_plugins(None, None)
        .await
        .expect("Failed to list plugins");

    if plugins.is_empty() {
        return;
    }

    let plugin_id = &plugins[0].id;
    let result = client
        .update_plugin(plugin_id, serde_json::json!({ "timeout": 0 }))
        .await;

    assert!(result.is_err(), "Should fail for timeout=0");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("Timeout"),
        "Error should indicate bad request: {}",
        error_msg
    );
}

/// Tests that updating a plugin with unknown fields returns 400
#[tokio::test]
#[serial]
async fn test_update_plugin_unknown_field() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };

    let plugins = client
        .list_plugins(None, None)
        .await
        .expect("Failed to list plugins");

    if plugins.is_empty() {
        return;
    }

    let plugin_id = &plugins[0].id;
    let result = client
        .update_plugin(plugin_id, serde_json::json!({ "unknown_field": "value" }))
        .await;

    assert!(result.is_err(), "Should fail for unknown field");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("400") || error_msg.contains("unknown"),
        "Error should indicate bad request: {}",
        error_msg
    );
}

/// Tests plugin state transition by updating flags/config and verifying both GET and /call behavior.
#[tokio::test]
#[serial]
async fn test_update_plugin_state_transition_reflected_in_get_and_call() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    if client.get_plugin(E2E_PLUGIN_ID).await.is_err() {
        return;
    }

    let original = client
        .get_plugin(E2E_PLUGIN_ID)
        .await
        .expect("Failed to get e2e plugin");

    client
        .update_plugin(
            E2E_PLUGIN_ID,
            serde_json::json!({
                "emit_logs": true,
                "emit_traces": false,
                "config": {
                    "suite": "integration",
                    "transition": "active"
                }
            }),
        )
        .await
        .expect("Failed to update plugin for state transition test");

    let fetched = client
        .get_plugin(E2E_PLUGIN_ID)
        .await
        .expect("Failed to fetch updated plugin");

    let call_result = client
        .call_plugin_post_raw(
            E2E_PLUGIN_ID,
            "/state-transition",
            serde_json::json!({ "probe": true }),
        )
        .await;

    let _ = client
        .update_plugin(
            E2E_PLUGIN_ID,
            serde_json::json!({
                "emit_logs": original.emit_logs,
                "emit_traces": original.emit_traces,
                "config": original.config
            }),
        )
        .await;

    assert!(fetched.emit_logs, "emit_logs should be true after update");
    assert!(
        !fetched.emit_traces,
        "emit_traces should be false after update"
    );
    assert_eq!(
        fetched
            .config
            .as_ref()
            .and_then(|cfg| cfg.get("transition"))
            .and_then(Value::as_str),
        Some("active"),
        "GET plugin should reflect updated config"
    );

    let (status, body) = call_result.expect("Failed to call plugin after update");
    assert_eq!(status, 200, "Expected plugin call to succeed: {}", body);

    let metadata = plugin_call_metadata(&body);
    assert!(
        metadata
            .as_ref()
            .and_then(|m| m.get("logs"))
            .is_some_and(Value::is_array),
        "metadata.logs should be populated when emit_logs=true: {}",
        body
    );
    assert!(
        metadata.as_ref().and_then(|m| m.get("traces")).is_none(),
        "metadata.traces should be omitted when emit_traces=false: {}",
        body
    );

    let data = plugin_call_data(&body);
    assert_eq!(
        data.pointer("/config/transition"),
        Some(&serde_json::json!("active")),
        "Plugin call data should include the updated config"
    );
}

/// Tests update idempotency: repeating the same payload keeps response/state stable.
#[tokio::test]
#[serial]
async fn test_update_plugin_idempotent_repeated_payload() {
    let Some(client) = RelayerClient::from_env_or_skip().await else {
        return;
    };
    if client.get_plugin(E2E_PLUGIN_ID).await.is_err() {
        return;
    }

    let original = client
        .get_plugin(E2E_PLUGIN_ID)
        .await
        .expect("Failed to fetch e2e plugin");

    let payload = serde_json::json!({
        "timeout": original.timeout.as_secs(),
        "emit_logs": original.emit_logs,
        "emit_traces": original.emit_traces,
        "allow_get_invocation": original.allow_get_invocation,
        "raw_response": original.raw_response,
        "forward_logs": original.forward_logs,
        "config": original.config
    });

    let first = client
        .update_plugin(E2E_PLUGIN_ID, payload.clone())
        .await
        .expect("First idempotent update should succeed");

    let second = client
        .update_plugin(E2E_PLUGIN_ID, payload)
        .await
        .expect("Second idempotent update should succeed");

    let fetched = client
        .get_plugin(E2E_PLUGIN_ID)
        .await
        .expect("Failed to fetch plugin after repeated updates");

    assert_eq!(first.timeout.as_secs(), second.timeout.as_secs());
    assert_eq!(first.emit_logs, second.emit_logs);
    assert_eq!(first.emit_traces, second.emit_traces);
    assert_eq!(first.allow_get_invocation, second.allow_get_invocation);
    assert_eq!(first.raw_response, second.raw_response);
    assert_eq!(first.forward_logs, second.forward_logs);
    assert_eq!(first.config, second.config);

    assert_eq!(fetched.timeout.as_secs(), first.timeout.as_secs());
    assert_eq!(fetched.emit_logs, first.emit_logs);
    assert_eq!(fetched.emit_traces, first.emit_traces);
    assert_eq!(fetched.allow_get_invocation, first.allow_get_invocation);
    assert_eq!(fetched.raw_response, first.raw_response);
    assert_eq!(fetched.forward_logs, first.forward_logs);
    assert_eq!(fetched.config, first.config);
}
