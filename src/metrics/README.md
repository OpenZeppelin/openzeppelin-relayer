# Metrics

- This folder contains middleware that is used to intercept the requests for all the endpoints as well as the definition of the metrics that are collected.

- Metrics server is started on port `8081` which collects the metrics from the relayer app and exposes them on the `/metrics` endpoint.

- We use `prometheus` to collect metrics from the application. The list of metrics are exposed on the `/metrics` endpoint.

- For details on specific metrics you can call them on the `/metrics/{metric_name}` endpoint.

- To view prometheus metrics in a UI, you can use `http://localhost:9090` on your browser.

- To view grafana dashboard, you can use `http://localhost:3000` on your browser.

## Transaction processing histogram

The `transaction_processing_seconds` histogram tracks transaction lifecycle timing with a `stage` label:

| Stage | Description |
| --- | --- |
| `request_queue_dwell` | Time from transaction creation to request handler start (queue pickup latency) |
| `prepare_duration` | Time spent in the prepare handler (simulation, signing, fee estimation) |
| `submission_queue_dwell` | Time from submission job enqueue to submission handler start |
| `submit_duration` | Time spent submitting the transaction to the network (RPC send) |
| `creation_to_submission` | End-to-end time from creation to network submission |
| `submission_to_confirmation` | Time from network submission to on-chain confirmation |
| `creation_to_confirmation` | Total lifecycle from creation to confirmation |

Additional labels: `relayer_id`, `network_type`.
