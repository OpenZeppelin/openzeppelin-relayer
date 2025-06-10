# Using Google Cloud KMS for Secure Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use a Google Cloud KMS key to securely sign transactions in OpenZeppelin Relayer.

## Prerequisites

1. A Google Cloud account with KMS enabled
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

### Step 2: Set Up Google Cloud KMS

1. Go to the [Google Cloud Console](https://console.cloud.google.com/)
2. Create a new project or select an existing one
3. Enable the **Cloud KMS API** for your project
4. Create a **Key Ring** and a **Key** (for Solana use ED25519)
5. Grant your service account the `Cloud KMS CryptoKey Signer/Verifier` role

### Step 3: Create a Service Account and Download Credentials

1. In the Google Cloud Console, go to **IAM & Admin > Service Accounts**
2. Create a new service account or use an existing one
3. Grant it access to your KMS key
4. Create and download a JSON key for this service account

### Step 4: Configure the Relayer Service

Create an environment file by copying the example:

```bash
cp examples/solana-google-cloud-kms-signer/.env.example examples/solana-google-cloud-kms-signer/.env
```

#### Populate Google Cloud KMS config

Edit the `config.json` file and update the following variables:

```json
{
  "signers": [
    {
      "id": "google-cloud-kms-signer-solana",
      "type": "google_cloud_kms",
      "config": {
        "service_account": {
          "project_id": "your_project_id",
          "private_key_id": {
            "type": "env",
            "value": "GOOGLE_CLOUD_KMS_PRIVATE_KEY_ID"
          },
            "private_key": {
            "type": "env",
            "value": "GOOGLE_CLOUD_KMS_PRIVATE_KEY"
          },
          "client_email": {
            "type": "env",
            "value": "GOOGLE_CLOUD_KMS_CLIENT_EMAIL"
          },
          "client_id": "your_client_id"
        },
        "key": {
          "key_ring_id": "your_key_ring_id",
          "key_id": "your_key_id"
        }
      }
    }
  ]
}
```

Populate `.env` file with config values for private_key_id, private_key and client_email.


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

Update the `examples/solana-google-cloud-kms-signer/config/config.json` file with your webhook configuration:

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

### Step 7: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/solana-google-cloud-kms-signer/docker-compose.yaml up
```

### Step 8: Test the Service

1. The service exposes a REST API
2. You can test it using curl or any HTTP client:

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

### Troubleshooting

If you encounter issues:

1. Verify your Google Cloud KMS credentials and permissions are correct
2. Check the service logs for detailed error messages
3. Verify the transaction format matches the expected schema

### Additional Resources

- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
- [Google Cloud KMS Documentation](https://cloud.google.com/kms/docs)
