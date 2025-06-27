# Using Google Cloud KMS for Secure EVM Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use Google Cloud KMS hosted private key to securely sign EVM transactions in OpenZeppelin Relayer.

## Prerequisites

1. A Google Cloud Platform account with KMS API enabled - [Get Started](https://cloud.google.com/kms)
2. Rust and Cargo installed
3. Git
4. [Docker](https://docs.docker.com/get-docker/)
5. [Docker Compose](https://docs.docker.com/compose/install/)

## Getting Started

### Step 1: Clone the Repository

Clone this repository to your local machine:

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Create KMS Key

1. Login to Google Cloud Console
2. Navigate to Security -> Key Management
3. Create a new key ring or use an existing one
4. Click "Create Key" and choose the following options:
   1. Protection level: HSM
   2. Purpose: Asymmetric sign
   3. Algorithm: Elliptic Curve secp256k1 - SHA256 Digest
5. Take note of:
   - Project ID
   - Location (region)
   - Key ring ID
   - Key ID
   - Key version (usually 1 for new keys)

### Step 3: Setup Google Cloud Service Account

1. Go to IAM & Admin -> Service Accounts
2. Create a new service account or use an existing one
3. Grant the following roles:
   - Cloud KMS CryptoKey Signer
   - Cloud KMS Viewer (for public key retrieval)
4. Create and download a JSON key file for the service account
5. Extract the following values from the JSON file:
   - `project_id`
   - `private_key_id`
   - `private_key` (PEM format)
   - `client_email`
   - `client_id`

### Step 4: Configure the Relayer Service

Create an environment file by copying the example:

```bash
cp examples/evm-gcp-kms-signer/.env.example examples/evm-gcp-kms-signer/.env
```

#### Populate Google Cloud KMS config

Edit the `config.json` file and update the following variables:

```json
{
  "signers": [
    {
      "id": "gcp-kms-signer-evm",
      "type": "google_cloud_kms",
      "config": {
        "service_account": {
          "project_id": "your-gcp-project-id",
          "private_key_id": {
            "type": "env",
            "value": "GCP_PRIVATE_KEY_ID"
          },
          "private_key": {
            "type": "env",
            "value": "GCP_PRIVATE_KEY"
          },
          "client_email": {
            "type": "env",
            "value": "GCP_CLIENT_EMAIL"
          },
          "client_id": "your-client-id"
        },
        "key": {
          "location": "us-west2",
          "key_ring_id": "your-key-ring",
          "key_id": "your-key-id",
          "key_version": 1
        }
      }
    }
  ]
}
```

#### Populate Google Cloud KMS Credentials

Add these values to your `.env` file (extracted from the service account JSON):

```env
GCP_PRIVATE_KEY_ID="private_key_id_from_service_account_json"
GCP_PRIVATE_KEY="-----BEGIN PRIVATE EXAMPLE KEY-----\n...\n-----END PRIVATE EXAMPLE KEY-----\n"
GCP_CLIENT_EMAIL="service-account@your-project.iam.gserviceaccount.com"
```

#### Generate Security Keys

Generate random keys for API authentication and webhook signing:

```bash
# Generate API key
cargo run --example generate_uuid

# Generate webhook signing key
cargo run --example generate_uuid
```

Add these to your `.env` file:

```env
WEBHOOK_SIGNING_KEY=generated_webhook_key
API_KEY=generated_api_key
```

#### Configure Webhook URL

Update the `examples/evm-gcp-kms-signer/config/config.json` file with your webhook configuration:

1. For testing, get a webhook URL from [Webhook.site](https://webhook.site)
2. Update the config file:

```json
{
  "notifications": [
    {
      "url": "your_webhook_url"
    }
  ]
}
```

### Step 5: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/evm-gcp-kms-signer/docker-compose.yaml up
```

### Step 6: Test the Service

#### 6.1 Check Relayer Status

First, verify that your relayer is running and properly configured:

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

This should return information about your relayer, including its address derived from the Google Cloud KMS public key.

#### 6.2 Test EVM Transaction Signing

Test the complete transaction signing and submission process:

```bash
curl -X POST http://localhost:8080/api/v1/relayers/your-relayer-id/transactions \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY" \
  -d '{
    "value": 1,
    "data": "0x",
    "to": "0x742d35cc6604c532532db3ae0f4d03e7c7b17e3e",
    "gas_limit": 21000,
    "speed": "average"
  }'
```

**What this does:**

- Creates a transaction sending 1 wei to the specified address
- Uses Google Cloud KMS to sign the transaction
- Submits the signed transaction to the network
- Returns transaction details including the transaction hash

### Troubleshooting

If you encounter issues:

1. **Authentication Issues**:
   - Verify your service account JSON is correct
   - Ensure the service account has the required KMS permissions
   - Check that the project ID matches your GCP project

2. **Key Access Issues**:
   - Verify the key location, key ring ID, and key ID are correct
   - Ensure the key is created with the correct algorithm (EC_SIGN_P256_SHA256)
   - Check that the key version exists

3. **Signing Failures**:
   - Verify the key has signing permissions
   - Check that the key algorithm supports secp256k1 operations
   - Review service logs for detailed error messages

4. **Network Issues**:
   - Ensure your environment can reach Google Cloud KMS APIs
   - Check firewall settings if running in a restricted environment

### Additional Resources

- [Google Cloud KMS Documentation](https://cloud.google.com/kms/docs)
- [Service Account Authentication](https://cloud.google.com/docs/authentication/getting-started)
- [KMS Key Management](https://cloud.google.com/kms/docs/creating-keys)
- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
