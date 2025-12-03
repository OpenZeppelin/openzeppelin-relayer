# Using AWS KMS for Secure Stellar Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use AWS KMS hosted Ed25519 private key to securely sign Stellar transactions in OpenZeppelin Relayer.

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
4. Choose the following options:
   1. Key type: Asymmetric
   2. Key usage: Sign & Verify
   3. Key spec: `ECC_EDWARDS_ED25519`
5. Finish key creation by following the next tabs
6. Take a note of the key id in the dashboard. Usually it is in UUIDv4 format

> **Important**: Stellar networks use Ed25519 cryptography. Make sure to select `ECC_EDWARDS_ED25519` as the key spec. Using other key types (like `ECC_SECG_P256K1`) will result in signing failures.

### Step 3: Setup AWS Shared Credentials

1. Go to Access Portal -> Access Keys
2. Follow Option 1: Set AWS environment variables
3. Make sure your region is the same as the key's original one
4. For advanced configuration of credentials follow the [latest guide](https://docs.aws.amazon.com/sdkref/latest/guide/file-format.html)

### Step 4: Configure the Relayer Service

Create an environment file by copying the example:

```bash
cp examples/stellar-aws-kms-signer/.env.example examples/stellar-aws-kms-signer/.env
```

#### Populate AWS KMS config

Edit the `config/config.json` file and update the following variables:

```json
{
  "signers": [
    {
      "id": "aws-kms-signer-stellar",
      "type": "aws_kms",
      "config": {
        "region": "us-east-1",
        "key_id": "your-key-id-here"
      }
    }
  ]
}
```

#### Populate AWS KMS Credentials

Add these values to your `.env` file:

```env
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

#### Configure Webhook URL (Optional)

Update the `examples/stellar-aws-kms-signer/config/config.json` file with your webhook configuration:

1. For testing, get a webhook URL from [Webhook.site](https://webhook.site)
2. Update the config file:

```json
{
  "notifications": [
    {
      "id": "notification-example",
      "type": "webhook",
      "url": "your_webhook_url",
      "signing_key": {
        "type": "env",
        "value": "WEBHOOK_SIGNING_KEY"
      }
    }
  ]
}
```

### Step 5: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/stellar-aws-kms-signer/docker-compose.yaml up
```

### Step 6: Test the Service

#### 6.1 Check Relayer Status

First, verify that your relayer is running and properly configured:

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

This should return information about your relayer, including its Stellar address (G-prefixed) derived from the AWS KMS Ed25519 public key.

#### 6.2 Test Stellar Transaction Signing

Test the transaction signing process by sending a transaction request:

```bash
curl -X POST http://localhost:8080/api/v1/relayers/stellar-example/transactions \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY" \
  -d '{
    "to": "GDESTINATION_ADDRESS_HERE",
    "amount": "10",
    "asset": "native"
  }'
```

**What this does:**

- Creates a Stellar payment transaction
- Uses AWS KMS Ed25519 key to sign the transaction
- Submits the signed transaction to the Stellar network
- Returns transaction details including the transaction hash

### Troubleshooting

If you encounter issues:

1. **Key Type Error**:
   - Verify your AWS KMS key was created with `ECC_EDWARDS_ED25519` spec
   - Stellar requires Ed25519 keys; secp256k1 keys will not work

2. **Authentication Issues**:
   - Verify your AWS credentials are correct
   - Ensure the credentials have permissions to use the KMS key

3. **Region Mismatch**:
   - Verify the region in your config matches the key's original region
   - Non-replicated keys must use the exact region where they were created

4. **Signing Failures**:
   - Check that the key has signing permissions
   - Review service logs for detailed error messages

5. **Network Issues**:
   - Ensure your environment can reach AWS KMS APIs
   - Check firewall settings if running in a restricted environment

### Additional Resources

- [AWS Shared Configuration](https://docs.aws.amazon.com/sdkref/latest/guide/file-format.html)
- [AWS KMS Documentation](https://docs.aws.amazon.com/kms/latest/developerguide/overview.html)
- [AWS KMS Key Specs](https://docs.aws.amazon.com/kms/latest/developerguide/asymmetric-key-specs.html)
- [Stellar Documentation](https://developers.stellar.org/docs)
- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
