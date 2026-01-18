# Plugin System Architecture

Developer guide for understanding and modifying the plugin execution system.

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              Rust Relayer                                │
│  ┌─────────────┐    ┌──────────────┐    ┌──────────────────────────┐   │
│  │ HTTP API    │───▶│ PluginRunner │───▶│ PoolManager              │   │
│  │ /plugins/*  │    │              │    │  - CircuitBreaker        │   │
│  └─────────────┘    └──────────────┘    │  - ConnectionPool        │   │
│                                          │  - RequestQueue          │   │
│                                          └───────────┬──────────────┘   │
│                                                      │                   │
│                                    Unix Socket (JSON-line protocol)      │
│                                                      │                   │
└──────────────────────────────────────────────────────┼───────────────────┘
                                                       │
┌──────────────────────────────────────────────────────┼───────────────────┐
│                           Node.js Pool Server        │                   │
│                                          ┌───────────▼──────────────┐    │
│  ┌──────────────────────┐               │ pool-server.ts           │    │
│  │ Piscina Worker Pool  │◀──────────────│  - Request Router        │    │
│  │  ┌─────────────────┐ │               │  - Memory Pressure Mon   │    │
│  │  │ Worker Thread 1 │ │               │  - Cache Management      │    │
│  │  │ (sandbox-exec)  │ │               └──────────────────────────┘    │
│  │  ├─────────────────┤ │                                                │
│  │  │ Worker Thread 2 │ │               ┌──────────────────────────┐    │
│  │  │ (sandbox-exec)  │ │               │ SharedSocketService      │    │
│  │  ├─────────────────┤ │◀─────────────▶│ (Rust ↔ Plugin comms)    │    │
│  │  │ Worker Thread N │ │               └──────────────────────────┘    │
│  │  └─────────────────┘ │                                                │
│  └──────────────────────┘                                                │
└──────────────────────────────────────────────────────────────────────────┘
```

## Component Overview

### Rust Side (`src/services/plugins/`)

| Module | Purpose |
|--------|---------|
| `config.rs` | Centralized configuration with auto-derivation from `PLUGIN_MAX_CONCURRENCY` |
| `runner.rs` | Entry point - routes requests to pool executor, handles precompilation |
| `pool_executor.rs` | Manages Node.js process lifecycle, connection pooling, circuit breaker |
| `health.rs` | Circuit breaker, health status, dead server detection |
| `protocol.rs` | JSON-line message types (PoolRequest, PoolResponse) |
| `connection.rs` | Lock-free connection pool with semaphore-based concurrency |
| `shared_socket.rs` | Per-request Unix socket for plugin ↔ Rust API communication |
| `relayer_api.rs` | Plugin API implementation (sendTransaction, signMessage, etc.) |

### Node.js Side (`plugins/lib/`)

| Module | Purpose |
|--------|---------|
| `pool-server.ts` | Main server - accepts connections, routes to workers, manages memory |
| `worker-pool.ts` | Piscina wrapper with dynamic scaling and cache management |
| `pool-executor.ts` | Plugin execution |
| `compiler.ts` | TypeScript/JavaScript compilation with esbuild |
| `plugin.ts` | Plugin SDK (PluginContext, PluginAPI) |
| `kv.ts` | Redis-backed key-value store for plugins |

## Communication Protocol

### Pool Protocol (Rust ↔ pool-server.ts)

Unix socket at `/tmp/relayer-plugin-pool-{uuid}.sock` using newline-delimited JSON.

**Request Types:**
```typescript
// Execute a plugin
{ type: "execute", taskId, pluginId, compiledCode?, pluginPath?, params, socketPath, timeout? }

// Precompile TypeScript
{ type: "precompile", taskId, pluginId, pluginPath?, sourceCode? }

// Cache compiled code
{ type: "cache", taskId, pluginId, compiledCode }

// Invalidate cache
{ type: "invalidate", taskId, pluginId }

// Health check
{ type: "health", taskId }

// Graceful shutdown
{ type: "shutdown", taskId }
```

**Response:**
```typescript
{ taskId, success: boolean, result?, error?: { message, code?, status?, details? }, logs? }
```

### Shared Socket Protocol (Plugin ↔ Rust)

Per-request socket at `/tmp/relayer-shared-{uuid}.sock` for plugin API calls.

```typescript
// Plugin sends:
{ type: "sendTransaction", requestId, relayerId, transaction }
{ type: "getRelayerBalance", requestId, relayerId }
// ... other API methods

// Rust responds:
{ requestId, success: boolean, data?, error? }
```

## Request Flow

```
1. HTTP POST /api/v1/plugins/{id}/call
   │
2. PluginRunner.run_plugin()
   │
   ├─ Check if compiled code cached
   │  └─ If not: precompile via pool
   │
3. PoolManager.execute_plugin()
   │
   ├─ Circuit breaker check (reject if open)
   │
   ├─ Try acquire connection permit (fast path)
   │  └─ If unavailable: queue request (slow path)
   │
4. Send Execute request to pool-server
   │
5. pool-server routes to Piscina worker
   │
6. pool-executor.ts runs plugin in VM
   │
   ├─ Plugin calls api.useRelayer().sendTransaction()
   │  └─ Communicates via shared socket to Rust
   │
7. Response flows back through chain
   │
8. HTTP 200 with { success, data, metadata }
```

## Environment Variables

> **Source of truth**: `src/constants/plugins.rs` defines all default values.
> Auto-derivation logic is in `src/services/plugins/config.rs`.

### Primary Scaling Knob

| Variable | Default | Description |
|----------|---------|-------------|
| `PLUGIN_MAX_CONCURRENCY` | 2048 | **Main knob** - drives auto-derivation of all other values |

### Auto-Derived (override only if needed)

| Variable | Default Formula | Description |
|----------|-----------------|-------------|
| `PLUGIN_POOL_MAX_CONNECTIONS` | = max_concurrency | Rust connection pool size |
| `PLUGIN_POOL_MAX_QUEUE_SIZE` | = max_concurrency * 2 | Request queue capacity |
| `PLUGIN_SOCKET_MAX_CONCURRENT_CONNECTIONS` | = max_concurrency * 1.5 | Shared socket connections |
| `PLUGIN_POOL_MIN_THREADS` | max(2, cpu_count/2) | Node.js minimum workers |
| `PLUGIN_POOL_MAX_THREADS` | memory-aware, max 32 | Node.js maximum workers |
| `PLUGIN_POOL_CONCURRENT_TASKS` | (concurrency/threads)*1.2, max 250 | Tasks per worker |
| `PLUGIN_WORKER_HEAP_MB` | 512 + (concurrent_tasks * 5) | Per-worker heap size |

### Fine-Tuning

| Variable | Default | Description |
|----------|---------|-------------|
| `PLUGIN_POOL_WORKERS` | 0 (auto) | Rust queue worker threads |
| `PLUGIN_POOL_CONNECT_RETRIES` | 15 | Connection retry attempts |
| `PLUGIN_POOL_REQUEST_TIMEOUT_SECS` | 30 | Per-request timeout |
| `PLUGIN_POOL_QUEUE_SEND_TIMEOUT_MS` | 500 | Queue wait timeout (auto-scales to 1000) |
| `PLUGIN_POOL_IDLE_TIMEOUT` | 60000 | Worker idle timeout (ms) |
| `PLUGIN_POOL_SOCKET_BACKLOG` | max(concurrency, 2048) | Socket backlog size |
| `PLUGIN_POOL_HEALTH_CHECK_INTERVAL_SECS` | 5 | Health check frequency |
| `PLUGIN_SOCKET_IDLE_TIMEOUT_SECS` | 60 | Shared socket idle timeout |
| `PLUGIN_SOCKET_READ_TIMEOUT_SECS` | 30 | Shared socket read timeout |

## Health & Recovery

### Circuit Breaker States

```
CLOSED ──(>50% failure rate)──▶ OPEN
   ▲                              │
   │                         (5s backoff)
   │                              ▼
   └──(10 consecutive OK)──── HALF_OPEN
```

- **Closed**: All requests allowed
- **Open**: Most requests rejected, pool server restart triggered
- **Half-Open**: 10% of requests allowed to probe recovery

### Dead Server Detection

Errors that trigger automatic restart:
- `eof while parsing` - Connection closed mid-message
- `broken pipe` - Write to closed socket
- `connection refused` - Server not listening
- `connection reset` - Server forcefully closed
- `socket file missing` - Unix socket deleted

### Memory Pressure Handling

pool-server.ts monitors heap usage and triggers:
1. **75% heap**: Force GC, evict old cache entries
2. **85% heap**: Aggressive cache eviction, warning logs
3. **95% heap**: Emergency measures, reject new requests

## Performance Tuning

### Low Concurrency (<100 plugins/sec)
```bash
export PLUGIN_MAX_CONCURRENCY=100
# Everything auto-derives appropriately
```

### Medium Concurrency (100-1000 plugins/sec)
```bash
export PLUGIN_MAX_CONCURRENCY=500
# Consider increasing if seeing queue full errors:
# export PLUGIN_POOL_MAX_QUEUE_SIZE=2000
```

### High Concurrency (1000+ plugins/sec)
```bash
export PLUGIN_MAX_CONCURRENCY=5000
# On 32GB+ systems, can increase threads:
# export PLUGIN_POOL_MAX_THREADS=16
# export PLUGIN_POOL_CONCURRENT_TASKS=200
```

### Debugging

Enable debug logging:
```bash
RUST_LOG=openzeppelin_relayer::services::plugins=debug cargo run
```

Check health endpoint:
```bash
curl http://localhost:8080/api/v1/health/plugins
```

## Module Dependency Graph

```
config.rs
    │
    ▼
health.rs ◀─────────────────────┐
    │                           │
    ▼                           │
protocol.rs                     │
    │                           │
    ▼                           │
connection.rs                   │
    │                           │
    ▼                           │
pool_executor.rs ───────────────┘
    │
    ▼
runner.rs ◀──── shared_socket.rs ◀──── relayer_api.rs
```

## Testing

```bash
# Rust unit tests
cargo test --package openzeppelin-relayer plugins

# TypeScript tests
cd plugins && pnpm test

# Integration test with load
# (requires k6: https://k6.io)
k6 run tests/load/plugins.js
```

## Common Issues

| Symptom | Likely Cause | Solution |
|---------|--------------|----------|
| "Queue full" errors | Max concurrency too low | Increase `PLUGIN_MAX_CONCURRENCY` |
| OOM in Node.js | Too many threads/tasks | Reduce `PLUGIN_POOL_MAX_THREADS` |
| Slow response times | GC pressure | Check health endpoint, reduce concurrent tasks |
| "Circuit breaker open" | Pool server crashed | Check logs, will auto-recover |
| Connection refused | Pool server not started | Check `ensure_started()` is called |
