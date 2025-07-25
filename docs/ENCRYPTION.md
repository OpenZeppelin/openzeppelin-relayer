# Field-Level Encryption for Sensitive Data

The OpenZeppelin Relayer now includes field-level encryption to protect sensitive data at rest in Redis. This feature ensures that private keys, API secrets, and other sensitive information are encrypted before being stored.

## Overview

The encryption system uses **AES-256-GCM** encryption with the following features:

- **Transparent encryption/decryption**: Happens automatically in the repository layer
- **Field-level encryption**: Only sensitive fields are encrypted, not entire records
- **Backward compatibility**: Can read both encrypted and legacy base64-encoded data
- **Memory protection**: Uses `SecretString` and `SecretVec` for in-memory protection
- **Authenticated encryption**: AES-GCM provides both confidentiality and integrity

## Protected Data

The following sensitive fields are automatically encrypted:

### Signer Configurations
- **Private keys** (`raw_key` in LocalSignerConfig)
- **API secrets** (Turnkey, Vault, Google Cloud KMS credentials)
- **Role IDs and Secret IDs** (Vault authentication)
- **Service account private keys** (Google Cloud KMS)

### Other Sensitive Fields
- Any field using `SecretString` type
- Custom sensitive fields marked for encryption

## Setup

### 1. Generate Encryption Key

Generate a 32-byte encryption key using OpenSSL:

```bash
# Generate and export the key
export STORAGE_ENCRYPTION_KEY=$(openssl rand -base64 32)

# Or generate hex-encoded key (alternative)
export STORAGE_ENCRYPTION_KEY_HEX=$(openssl rand -hex 32)
```

### 2. Environment Configuration

Set one of the following environment variables:

```bash
# Option 1: Base64-encoded key (recommended)
export STORAGE_ENCRYPTION_KEY="your-base64-encoded-32-byte-key"

# Option 2: Hex-encoded key (alternative)
export STORAGE_ENCRYPTION_KEY_HEX="your-hex-encoded-32-byte-key"
```

### 3. Production Deployment

For production deployments, consider using:

- **Container secrets**: Mount the key as a secret volume
- **Environment injection**: Use your orchestration platform's secret management
- **External secret management**: AWS Secrets Manager, HashiCorp Vault, etc.

Example Docker Compose:
```yaml
services:
  relayer:
    image: openzeppelin/relayer
    environment:
      - STORAGE_ENCRYPTION_KEY=${ENCRYPTION_KEY}
    secrets:
      - encryption_key
    
secrets:
  encryption_key:
    external: true
```

## How It Works

### Encryption Process

1. **Data Input**: Sensitive data enters as plaintext
2. **Encryption**: Data is encrypted with AES-256-GCM using a random nonce
3. **Storage**: Encrypted data structure (nonce + ciphertext + version) is stored directly as JSON

### Decryption Process

1. **Data Retrieval**: JSON-encoded encrypted data is retrieved from storage
2. **Decryption**: Data is decrypted using the configured encryption key
3. **Fallback**: If encryption is not configured, data is treated as JSON-encoded strings

### Data Format

Encrypted data is stored directly as JSON with this structure:
```json
{
  "nonce": "base64-encoded-12-byte-nonce",
  "ciphertext": "base64-encoded-encrypted-data-with-auth-tag", 
  "version": 1
}
```

When encryption is disabled (development mode), data is stored as simple JSON strings.

## Migration

### For New Deployments
- Set up encryption key before first run
- All sensitive data will be encrypted from the start

### For Existing Deployments
- **Data migration required** if you have existing sensitive data
- Export existing data before upgrading
- Set up encryption key
- Re-import data (will be encrypted during import)

### Migration Steps
If you have existing deployments with sensitive data:

1. **Backup**: Export all existing signer configurations
2. **Setup**: Configure encryption keys in your environment
3. **Clear**: Remove existing signer data from Redis (optional but recommended)
4. **Import**: Re-create signers through the API (data will be automatically encrypted)
5. **Verify**: Test that encrypted data can be properly read and used

## Security Considerations

### Key Management
- **Never commit keys to version control**
- **Rotate keys periodically** (requires data re-encryption)
- **Use secure key storage** in production
- **Limit key access** to essential personnel only

### Operational Security
- **Monitor key access logs**
- **Use different keys per environment**
- **Implement key backup and recovery procedures**
- **Consider HSM integration** for high-security environments

### Development vs Production
- **Development**: Can run without encryption (falls back to base64)
- **Production**: Always require encryption keys
- **Testing**: Use test-specific keys, never production keys

## Troubleshooting

### Common Issues

#### 1. Key Not Found Error
```
Missing encryption key environment variable: Either STORAGE_ENCRYPTION_KEY (base64) or STORAGE_ENCRYPTION_KEY_HEX (hex) must be set
```
**Solution**: Set one of the required environment variables.

#### 2. Invalid Key Length
```
Invalid key length: expected 32 bytes, got X
```
**Solution**: Ensure your key is exactly 32 bytes when decoded.

#### 3. Decryption Failed
```
Failed to decrypt raw_key: Decryption failed
```
**Possible causes**:
- Wrong encryption key
- Corrupted data
- Mixed data from different keys

### Validation

Test your encryption setup:

```bash
# Check if encryption is configured
curl http://localhost:3000/health

# Create a test signer to verify encryption works
curl -X POST http://localhost:3000/relayers \
  -H "Content-Type: application/json" \
  -d '{"id": "test", "type": "local", "config": {...}}'
```

### Logging

Enable debug logging to see encryption operations:
```bash
RUST_LOG=debug ./openzeppelin-relayer
```

Look for log messages like:
- `"Retrieved signer with ID: ..."`
- `"Created signer with ID: ..."`

## Performance Impact

### Encryption Overhead
- **CPU**: Minimal overhead (~1-5% for typical workloads)
- **Memory**: Slight increase due to JSON structure
- **Storage**: ~15-25% increase due to encryption metadata (reduced from previous base64-in-base64 format)

### Optimization Tips
- **Batch operations**: Already optimized for bulk reads/writes
- **Connection pooling**: Use Redis connection pooling (already implemented)
- **Monitoring**: Monitor Redis performance metrics

## Compliance and Auditing

### Standards Compliance
- **Encryption**: AES-256-GCM (NIST approved)
- **Key size**: 256-bit keys
- **Nonces**: Cryptographically secure random generation
- **Authentication**: Integrated with GCM mode

### Audit Trail
- All encryption/decryption operations are logged
- Failed decryption attempts are logged as warnings
- Key access should be monitored externally

### Data Protection
- **Data at rest**: Encrypted in Redis
- **Data in transit**: Use TLS for Redis connections
- **Data in memory**: Protected with `SecretString`/`SecretVec`
- **Data in logs**: Sensitive data is redacted from logs

## Example Implementation

Here's how encryption works in practice:

```rust
// Creating a signer (automatically encrypted)
let signer = SignerRepoModel {
    id: "my-signer".to_string(),
    config: SignerConfig::Local(LocalSignerConfig {
        raw_key: SecretVec::new(32, |buf| {
            buf.copy_from_slice(&private_key_bytes);
        }),
    }),
};

// This will be automatically encrypted before storage
repository.create(signer).await?;

// Reading a signer (automatically decrypted)
let retrieved_signer = repository.get_by_id("my-signer".to_string()).await?;
// retrieved_signer contains decrypted data, ready to use
```

The encryption/decryption happens transparently - your application code doesn't need to change. 