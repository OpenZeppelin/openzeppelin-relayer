# OpenZeppelin Relayer Redis TLS Example

This example demonstrates how to configure the OpenZeppelin Relayer with encrypted Redis connections using TLS.

## Overview

By default, the relayer connects to Redis over unencrypted `redis://` connections. For production deployments — especially with cloud-managed Redis services like AWS ElastiCache, Azure Cache for Redis, or Google Cloud Memorystore — TLS encryption is strongly recommended.

This example shows:

- Building the relayer with TLS support enabled
- Configuring Redis with TLS certificates
- Connecting over the `rediss://` (double `s`) URI scheme

## Getting Started

### Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)
- [OpenSSL](https://www.openssl.org/) (for generating test certificates)
- Rust (for key generation tools)

### Step 1: Generate TLS Certificates

Generate self-signed certificates for local testing:

```bash
chmod +x examples/redis-tls/generate-certs.sh
./examples/redis-tls/generate-certs.sh
```

This creates `ca.crt`, `redis.key`, and `redis.crt` in `examples/redis-tls/certs/`.

> **Note**: These self-signed certificates are for local development only. In production, use certificates from your cloud provider or a trusted CA.

### Step 2: Create a Signer

```bash
cargo run --example create_key -- \
  --password <DEFINE_YOUR_PASSWORD> \
  --output-dir examples/redis-tls/config/keys \
  --filename local-signer.json
```

### Step 3: Configure Environment

```bash
cp examples/redis-tls/.env.example examples/redis-tls/.env
```

Update the `.env` file with your values:

- `KEYSTORE_PASSPHRASE`: Password from Step 2
- `API_KEY`: Generate with `cargo run --example generate_uuid`
- `WEBHOOK_SIGNING_KEY`: Generate with `cargo run --example generate_uuid`
- `STORAGE_ENCRYPTION_KEY`: Generate with `cargo run --example generate_encryption_key`

### Step 4: Configure Notifications

Update the `notifications[0].url` field in `examples/redis-tls/config/config.json` with your webhook URL. For testing, use [Webhook.site](https://webhook.site).

### Step 5: Run the Service

```bash
docker compose -f examples/redis-tls/docker-compose.yaml up
```

The relayer will:

1. Build with the `redis-tls-native` feature enabled
2. Start Redis with TLS on port 6380
3. Connect to Redis using `rediss://redis:6380` (encrypted)

### Step 6: Test the Relayer

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

## TLS Feature Flags

The relayer supports two TLS backends via Cargo feature flags:

| Feature            | Backend                     | Use Case                                        |
| ------------------ | --------------------------- | ----------------------------------------------- |
| `redis-tls-native` | System native TLS (OpenSSL) | Most production deployments, AWS ElastiCache    |
| `redis-tls-rustls` | rustls (pure Rust)          | Minimal container images, no OpenSSL dependency |

To use `rustls` instead, change the `CARGO_FEATURES` build arg in `docker-compose.yaml`:

```yaml
args:
  CARGO_FEATURES: redis-tls-rustls
```

## Production: Cloud-Managed Redis

For cloud-managed Redis with TLS, no certificate generation is needed — the cloud provider handles certificates. Simply use the `rediss://` URI scheme:

```bash
# AWS ElastiCache
REDIS_URL=rediss://my-cluster.xxx.cache.amazonaws.com:6380

# With authentication token
REDIS_URL=rediss://:your-auth-token@my-cluster.xxx.cache.amazonaws.com:6380

# With read replicas
REDIS_URL=rediss://my-cluster.xxx.cache.amazonaws.com:6380
REDIS_READER_URL=rediss://my-cluster-ro.xxx.cache.amazonaws.com:6380
```

## Security Notes

- **Never commit certificates or private keys** to version control
- Use certificates from a trusted CA in production
- Rotate certificates before expiration
- Enable Redis AUTH alongside TLS for defense in depth
- See the [Storage Configuration docs](../../docs/configuration/storage.mdx) for full security recommendations
