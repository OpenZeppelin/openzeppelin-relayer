# Using AWS KMS for Secure Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use AWS KMS hosted private key to securely sign transactions in OpenZeppelin Relayer.

>>[!IMPORTANT]
>> As of June 2025, AWS KMS does not yet support ED25519 signing scheme or secp256r1 curve. Therefore, the AWS KMS support is only offered to EVM chains and potentially other chains, that allow signing over secp256k1.
>>

## Prerequisites

1. An AWS account - [Get Started](https://aws.amazon.com/kms/)
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

1. Login to AWS portal
2. Navigate to Key Management Service portal
3. Click create key
4. Choose the following options
   1. Key type: Asymmetric
   2. Key usage: Sign & Verify
   3. Key spec: `ECC_SECG_P256K1`
5. Finish key creation by following the next tabs
6. Take a note of key id in the dashboard. Usually it is in UUIDv4 format

### Step 3: Setup AWS Shared Credentials

1. Go to Access Portal -> Access Keys
2. Follow Option 1: Set AWS environment variables
3. Make sure your region is the same as the key's original one
4. For advanced configuration of credentials follow the [latest guide](https://docs.aws.amazon.com/sdkref/latest/guide/file-format.html)


### Step 4: Configure the Relayer Service

Create an environment file by copying the example:

```bash
cp examples/evm-aws-kms-signer/.env.example examples/evm-aws-kms-signer/.env
```


#### Populate AWS KMS config

Edit the `config.json` file and update the following variables:

```json
{
  "signers": [
    {
      "id": "aws-kms-signer-evm",
      "type": "aws_kms",
      "config": {
        "region": "aws region",
        "key_id": "key identifier"
      }
    }
  ]
}
```

#### Populate AWS KMS Credentials

```
AWS_ACCESS_KEY_ID="Access key id from access portal"
AWS_SECRET_ACCESS_KEY="Secret access key from access portal"
AWS_SESSION_TOKEN="Session token from access portal"
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

Update the `examples/evm-aws-kms-signer/config/config.json` file with your webhook configuration:

1. For testing, get a webhook URL from [Webhook.site](https://webhook.site)
2. Update the config file:

```json
{
  "notifications": [
    {
      "url": "your_webhook_url",
    }
  ]
}
```

### Step 7: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/evm-aws-kms-signer/docker-compose.yaml up
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

1. Verify your AWS credentials are correct
2. Verify the configuration of the KMS key
3. Verify the region of the key and the region of credentials
4. Check the service logs for detailed error messages
5. Verify the transaction format matches the expected schema

### Additional Resources

- [AWS Shared Configuration](https://docs.aws.amazon.com/sdkref/latest/guide/file-format.html)
- [AWS KMS Documentation](https://docs.aws.amazon.com/kms/latest/developerguide/overview.html)
- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
