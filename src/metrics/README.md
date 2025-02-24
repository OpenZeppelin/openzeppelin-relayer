# Metrics

- This folder contains middleware that is used to intercept the requests for all the endpoints as well as the definition of the metrics that are collected.

- Metrics server is started on port `8081` which collects the metrics from the relayer app and exposes them on the `/metrics` endpoint.

- We use `prometheus` to collect metrics from the application. The list of metrics are exposed on the `/metrics` endpoint.

- For details on specific metrics you can call them on the `/metrics/{metric_name}` endpoint.

- To view prometheus metrics in a UI, you can use `http://localhost:9090` on your browser.

- To view grafana dashboard, you can use `http://localhost:3000` on your browser.
