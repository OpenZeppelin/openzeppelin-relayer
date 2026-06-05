//! Cloud Monitoring backlog-depth read.
//!
//! ONE light, low-frequency, batched read of
//! `subscription/num_undelivered_messages` across the project's subscriptions,
//! mapped back to queue types. Feeds BOTH the `queue_depth` gauge and the
//! health-endpoint depth. Any failure (or the emulator, which serves no Cloud
//! Monitoring) yields "unavailable" — never a hardcoded 0. Requires ADC with
//! `roles/monitoring.viewer`.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use token_source::TokenSource;

use super::QueueType;

const MONITORING_SCOPE: &str = "https://www.googleapis.com/auth/monitoring.read";
const NUM_UNDELIVERED_METRIC: &str = "pubsub.googleapis.com/subscription/num_undelivered_messages";

/// Builds an ADC token source scoped for Cloud Monitoring reads.
///
/// Uses the same `gcloud-auth` ADC resolution the Pub/Sub client uses
/// (service-account file / `GOOGLE_APPLICATION_CREDENTIALS[_JSON]` / metadata
/// server). The token is cached and refreshed internally by the provider.
pub(crate) async fn monitoring_token_source() -> Result<Arc<dyn TokenSource>, String> {
    use gcloud_pubsub::client::google_cloud_auth::project::Config;
    use gcloud_pubsub::client::google_cloud_auth::token::DefaultTokenSourceProvider;
    use token_source::TokenSourceProvider;

    let config = Config::default().with_scopes(&[MONITORING_SCOPE]);
    let provider = DefaultTokenSourceProvider::new(config)
        .await
        .map_err(|e| format!("failed to init Cloud Monitoring ADC token source: {e}"))?;
    Ok(provider.token_source())
}

/// Reads `num_undelivered_messages` for the project's subscriptions and maps
/// them to queue types via `subscription_to_queue` (reverse of the backend's
/// subscription-name map). One batched HTTP call.
pub(crate) async fn read_backlog_depths(
    http: &reqwest::Client,
    token_source: &Arc<dyn TokenSource>,
    project_id: &str,
    subscription_to_queue: &HashMap<String, QueueType>,
) -> Result<HashMap<QueueType, u64>, String> {
    // `token()` returns the value already formatted as "Bearer <access_token>".
    let token = token_source
        .token()
        .await
        .map_err(|e| format!("Cloud Monitoring token fetch failed: {e}"))?;

    let end = chrono::Utc::now();
    let start = end - chrono::Duration::seconds(300);
    let url = format!("https://monitoring.googleapis.com/v3/projects/{project_id}/timeSeries");

    let resp = http
        .get(&url)
        .header("Authorization", token)
        .query(&[
            (
                "filter",
                format!("metric.type=\"{NUM_UNDELIVERED_METRIC}\""),
            ),
            ("interval.startTime", start.to_rfc3339()),
            ("interval.endTime", end.to_rfc3339()),
        ])
        .send()
        .await
        .map_err(|e| format!("Cloud Monitoring request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Cloud Monitoring returned HTTP {}",
            resp.status().as_u16()
        ));
    }

    let body: TimeSeriesResponse = resp
        .json()
        .await
        .map_err(|e| format!("Cloud Monitoring response parse failed: {e}"))?;

    Ok(parse_depths(&body, subscription_to_queue))
}

/// Maps a parsed timeSeries response to per-queue depths using the latest point
/// of each subscription's series.
fn parse_depths(
    body: &TimeSeriesResponse,
    subscription_to_queue: &HashMap<String, QueueType>,
) -> HashMap<QueueType, u64> {
    let mut depths = HashMap::new();
    for series in &body.time_series {
        let Some(sub_id) = series.resource.labels.get("subscription_id") else {
            continue;
        };
        let Some(&queue_type) = subscription_to_queue.get(sub_id) else {
            continue; // not one of our subscriptions
        };
        // Points are newest-first; take the most recent int64 value.
        if let Some(value) = series
            .points
            .first()
            .and_then(|p| p.value.int64_value.as_deref())
            .and_then(|v| v.parse::<u64>().ok())
        {
            depths.insert(queue_type, value);
        }
    }
    depths
}

#[derive(Debug, Deserialize)]
struct TimeSeriesResponse {
    #[serde(rename = "timeSeries", default)]
    time_series: Vec<TimeSeries>,
}

#[derive(Debug, Deserialize)]
struct TimeSeries {
    #[serde(default)]
    resource: MonitoredResource,
    #[serde(default)]
    points: Vec<Point>,
}

#[derive(Debug, Default, Deserialize)]
struct MonitoredResource {
    #[serde(default)]
    labels: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct Point {
    value: PointValue,
}

#[derive(Debug, Deserialize)]
struct PointValue {
    #[serde(rename = "int64Value", default)]
    int64_value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub_map() -> HashMap<String, QueueType> {
        HashMap::from([
            (
                "relayer-status-check-evm-sub".to_string(),
                QueueType::StatusCheckEvm,
            ),
            (
                "relayer-transaction-request-sub".to_string(),
                QueueType::TransactionRequest,
            ),
        ])
    }

    #[test]
    fn test_parse_depths_maps_subscriptions_to_queues() {
        // int64 values are JSON strings; newest point is first.
        let json = r#"{
          "timeSeries": [
            {
              "resource": { "labels": { "subscription_id": "relayer-status-check-evm-sub" } },
              "points": [ { "value": { "int64Value": "42" } }, { "value": { "int64Value": "40" } } ]
            },
            {
              "resource": { "labels": { "subscription_id": "relayer-transaction-request-sub" } },
              "points": [ { "value": { "int64Value": "7" } } ]
            },
            {
              "resource": { "labels": { "subscription_id": "some-other-sub" } },
              "points": [ { "value": { "int64Value": "999" } } ]
            }
          ]
        }"#;
        let body: TimeSeriesResponse = serde_json::from_str(json).unwrap();
        let depths = parse_depths(&body, &sub_map());

        assert_eq!(depths.get(&QueueType::StatusCheckEvm), Some(&42)); // newest point
        assert_eq!(depths.get(&QueueType::TransactionRequest), Some(&7));
        assert_eq!(depths.len(), 2, "unknown subscriptions are ignored");
    }

    #[test]
    fn test_parse_depths_empty_response() {
        let body: TimeSeriesResponse = serde_json::from_str(r#"{}"#).unwrap();
        assert!(parse_depths(&body, &sub_map()).is_empty());
    }

    #[test]
    fn test_parse_depths_skips_series_without_points() {
        let json = r#"{"timeSeries":[{"resource":{"labels":{"subscription_id":"relayer-status-check-evm-sub"}},"points":[]}]}"#;
        let body: TimeSeriesResponse = serde_json::from_str(json).unwrap();
        assert!(parse_depths(&body, &sub_map()).is_empty());
    }
}
