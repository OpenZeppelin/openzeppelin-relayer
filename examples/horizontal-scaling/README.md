# OpenZeppelin Relayer - Horizontal Scaling Example

This example demonstrates how to deploy the OpenZeppelin Relayer in a horizontally scaled configuration for high-throughput production environments. It includes:

- **3 Relayer instances** running in parallel
- **Nginx load balancer** distributing traffic across instances
- **Shared Redis** for coordinated state management
- **High-performance configuration** optimized for power users

## Architecture

```
                    ┌─────────────────┐
                    │  Nginx (:8080)  │
                    │  Load Balancer  │
                    └────────┬────────┘
                             │
         ┌───────────────────┼───────────────────┐
         │                   │                   │
    ┌────▼────┐         ┌────▼────┐        ┌────▼────┐
    │Relayer-1│         │Relayer-2│        │Relayer-3│
    │  :8080  │         │  :8080  │        │  :8080  │
    └────┬────┘         └────┬────┘        └────┬────┘
         │                   │                   │
         └───────────────────┼───────────────────┘
                             │
                    ┌────────▼────────┐
                    │  Redis (:6379)  │
                    │  Shared State   │
                    └─────────────────┘
```

## Features

### Load Balancing
- **Least connections** algorithm routes requests to the instance with fewest active connections
- **Health checks** automatically remove unhealthy instances from the pool
- **Automatic failover** to healthy instances if one fails
- **Connection pooling** maintains persistent connections to backends

### High Availability
- Multiple relayer instances ensure service continuity
- Automatic restart on failure
- Health checks on all services
- Graceful degradation if instances go down

### Performance Optimizations
- **Rate limits**: 500 requests/second per instance (1500 total)
- **Burst capacity**: 1000 requests per instance
- **Resource limits**: 2 CPU cores, 4GB RAM per instance
- **Redis tuning**: Optimized for high throughput with 4GB memory
- **Worker configuration**: Increased concurrency for parallel processing

### Shared State
- All instances share Redis for:
  - Configuration management
  - Transaction state
  - Job queues
  - Nonce coordination
- Data encrypted at rest with `STORAGE_ENCRYPTION_KEY`

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) (20.10+)
- [Docker Compose](https://docs.docker.com/compose/install/) (v2.0+)
- Rust (for key generation tools)
- At least 8GB RAM available for Docker
- 4+ CPU cores recommended

## Getting Started

### Step 1: Clone the Repository

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Create Configuration Directory

```bash
mkdir -p examples/horizontal-scaling/config/keys
```

### Step 3: Generate Encryption Key

Generate a secure encryption key for Redis storage:

```bash
cargo run --example generate_encryption_key
```

Save this key - you'll need it in the `.env` file.

### Step 4: Create a Signer

Create a new signer keystore:

```bash
cargo run --example create_key -- \
  --password <STRONG_PASSWORD> \
  --output-dir examples/horizontal-scaling/config/keys \
  --filename local-signer.json
```

**Important**: Use a strong password with at least:
- 12 characters
- One uppercase letter
- One lowercase letter
- One number
- One special character

### Step 5: Configure Environment Variables

Create the `.env` file:

```bash
cp examples/horizontal-scaling/.env.example examples/horizontal-scaling/.env
```

Edit `examples/horizontal-scaling/.env` and set:

```bash
# API Key (generate with: cargo run --example generate_uuid)
API_KEY=your-api-key-here

# Webhook Signing Key (generate with: cargo run --example generate_uuid)
WEBHOOK_SIGNING_KEY=your-webhook-signing-key-here

# Keystore Passphrase (password used in Step 4)
KEYSTORE_PASSPHRASE=your-keystore-password-here

# Storage Encryption Key (generated in Step 3)
STORAGE_ENCRYPTION_KEY=your-encryption-key-here
```

To generate UUIDs for API_KEY and WEBHOOK_SIGNING_KEY:

```bash
cargo run --example generate_uuid
```

### Step 6: Configure Relayers

Edit `examples/horizontal-scaling/config/config.json` to configure your relayers.

The example includes a Sepolia testnet relayer. Update:
- `notifications[0].url`: Your webhook URL (get one from [Webhook.site](https://webhook.site))
- `custom_rpc_urls`: Your RPC endpoints (recommended for production)
- `policies`: Adjust as needed for your use case

### Step 7: Start the Services

Start all services (without metrics):

```bash
docker compose -f examples/horizontal-scaling/docker-compose.yaml up -d
```

### Step 8: Verify Deployment

Check that all services are running:

```bash
docker compose -f examples/horizontal-scaling/docker-compose.yaml ps
```

All services should show as "running" with healthy status.

### Step 9: Test Load Balancing

Test the load balancer:

```bash
# Check health
curl http://localhost:8080/health

# List relayers (replace YOUR_API_KEY)
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

## Performance Tuning

### Rate Limits

Adjust per-instance rate limits:

```yaml
environment:
  RATE_LIMIT_REQUESTS_PER_SECOND: 1000  # Increase for higher throughput
  RATE_LIMIT_BURST_SIZE: 2000
```

**Total capacity** = `RATE_LIMIT_REQUESTS_PER_SECOND` × number of instances

### Redis Configuration

For higher throughput, adjust Redis settings in `docker-compose.yaml`:

```yaml
command:
  - redis-server
  - --maxmemory
  - 8gb              # Increase if needed
  - --maxmemory-policy
  - allkeys-lru
```

### Worker Concurrency

The default worker concurrency is 1500 concurrent jobs per worker. You can tune it using environment variables based on your resource constraints:

```yaml
environment:
  # Global rate limiting for all workers (jobs per time window)
  - BACKGROUND_WORKER_RATE_LIMIT=5000
  - BACKGROUND_WORKER_RATE_LIMIT_DURATION_MS=1000
  
  # Per-worker concurrency settings
  - BACKGROUND_WORKER_TRANSACTION_REQUEST_CONCURRENCY=20
  - BACKGROUND_WORKER_TRANSACTION_SENDER_CONCURRENCY=20
  - BACKGROUND_WORKER_TRANSACTION_STATUS_CHECKER_CONCURRENCY=15
  - BACKGROUND_WORKER_TRANSACTION_STATUS_CHECKER_EVM_CONCURRENCY=15
  - BACKGROUND_WORKER_TRANSACTION_STATUS_CHECKER_STELLAR_CONCURRENCY=20
  - BACKGROUND_WORKER_NOTIFICATION_SENDER_CONCURRENCY=20
```

**Available Worker Configuration Variables:**

- `BACKGROUND_WORKER_RATE_LIMIT` - Global rate limit for all workers (default: 3000)
- `BACKGROUND_WORKER_RATE_LIMIT_DURATION_MS` - Rate limit time window in milliseconds (default: 1000)
- `BACKGROUND_WORKER_TRANSACTION_REQUEST_CONCURRENCY` - Transaction request worker concurrency (default: 1500)
- `BACKGROUND_WORKER_TRANSACTION_SENDER_CONCURRENCY` - Transaction submission worker concurrency (default: 1500)
- `BACKGROUND_WORKER_TRANSACTION_STATUS_CHECKER_CONCURRENCY` - Generic status checker concurrency (default: 1500)
- `BACKGROUND_WORKER_TRANSACTION_STATUS_CHECKER_EVM_CONCURRENCY` - EVM status checker concurrency (default: 1500)
- `BACKGROUND_WORKER_TRANSACTION_STATUS_CHECKER_STELLAR_CONCURRENCY` - Stellar status checker concurrency (default: 1500)
- `BACKGROUND_WORKER_NOTIFICATION_SENDER_CONCURRENCY` - Notification worker concurrency (default: 1500)
- `BACKGROUND_WORKER_SOLANA_TOKEN_SWAP_REQUEST_CONCURRENCY` - Solana swap worker concurrency (default: 1500)
- `BACKGROUND_WORKER_TRANSACTION_CLEANUP_CONCURRENCY` - Cleanup worker concurrency (default: 1)
- `BACKGROUND_WORKER_RELAYER_HEALTH_CHECK_CONCURRENCY` - Health check worker concurrency (default: 1500)

**Tuning Recommendations:**

- **High throughput**: Increase concurrency for transaction_request and transaction_sender workers
- **Fast finality networks** (Stellar): Increase the Stellar status checker concurrency
- **Slow finality networks** (Ethereum): Keep EVM status checker concurrency moderate to avoid overwhelming RPC endpoints
- **Rate limiting**: Adjust `BACKGROUND_WORKER_RATE_LIMIT` based on your infrastructure capacity
- **Cleanup worker**: Keep at 1 to avoid database conflicts

## Production Considerations

### Security

1. **Use HTTPS**: Deploy Nginx with TLS certificates
2. **Network isolation**: Use private networks in production
3. **Secrets management**: Use Docker secrets or vault solutions
4. **API key rotation**: Regularly rotate API keys
5. **Redis AUTH**: Enable Redis password authentication
6. **Firewall rules**: Restrict access to Redis and internal services

### High Availability

1. **Redis persistence**: Configured with AOF + RDB snapshots
2. **Multiple availability zones**: Deploy instances across zones
3. **Health checks**: Configured for all services
4. **Auto-restart**: All services restart on failure
5. **Backup strategy**: Regular Redis backups

### Backup & Recovery

```bash
# Backup Redis data
docker compose -f examples/horizontal-scaling/docker-compose.yaml exec redis \
  redis-cli BGSAVE

# Copy backup
docker cp $(docker compose ps -q redis):/data/dump.rdb ./backup/

# Restore
docker cp ./backup/dump.rdb $(docker compose ps -q redis):/data/
docker compose -f examples/horizontal-scaling/docker-compose.yaml restart redis
```

## Further Reading

- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer/)
- [Configuration Reference](https://docs.openzeppelin.com/relayer/configuration)
- [Storage Configuration](https://docs.openzeppelin.com/relayer/storage)
- [Nginx Load Balancing](https://nginx.org/en/docs/http/load_balancing.html)
- [Redis Persistence](https://redis.io/docs/manual/persistence/)

## Support

For issues or questions:
- [GitHub Issues](https://github.com/OpenZeppelin/openzeppelin-relayer/issues)
- [OpenZeppelin Forum](https://forum.openzeppelin.com/)
- [Documentation](https://docs.openzeppelin.com/relayer/)
