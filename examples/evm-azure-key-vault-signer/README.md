# Using Azure Key Vault for Secure EVM Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use an Azure Key Vault hosted secp256k1 key to securely sign EVM transactions in OpenZeppelin Relayer.

## Prerequisites

1. An Azure subscription with access to Azure Key Vault
2. A Microsoft Entra application (service principal) with a client secret
3. An EVM signing key stored in Azure Key Vault
4. Rust and Cargo installed
5. Git
6. [Docker](https://docs.docker.com/get-docker/)
7. [Docker Compose](https://docs.docker.com/compose/install/)

## Getting Started

### Step 1: Clone the Repository

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Create or Import an Azure Key Vault Signing Key

1. Open the Azure portal and create or select a Key Vault
2. Create or import the EVM signing key you want the relayer to use
3. Note the following values:
   - Tenant ID
   - Client ID
   - Client secret
   - Vault URL, for example `https://your-key-vault-name.vault.azure.net`
   - Key name
   - Optional key version
4. Make sure the service principal has permission to read the key metadata and sign payloads with it

### Step 3: Configure Environment Variables

Populate the environment variables used by the example:

```env
AZURE_TENANT_ID=your-tenant-id
AZURE_CLIENT_ID=your-client-id
AZURE_CLIENT_SECRET=your-client-secret
API_KEY=generated_api_key
WEBHOOK_SIGNING_KEY=generated_webhook_key
```

If you need values for `API_KEY` and `WEBHOOK_SIGNING_KEY`, you can generate them with:

```bash
cargo run --example generate_uuid
cargo run --example generate_uuid
```

### Step 4: Configure the Relayer

Update [`config/config.json`](./config/config.json) with your Azure Key Vault details:

```json
{
  "signers": [
    {
      "id": "azure-key-vault-signer-evm",
      "type": "azure_key_vault",
      "config": {
        "tenant_id": {
          "type": "env",
          "value": "AZURE_TENANT_ID"
        },
        "client_id": {
          "type": "env",
          "value": "AZURE_CLIENT_ID"
        },
        "client_secret": {
          "type": "env",
          "value": "AZURE_CLIENT_SECRET"
        },
        "vault_url": "https://your-key-vault-name.vault.azure.net",
        "key_name": "your-secp256k1-key-name",
        "key_version": null
      }
    }
  ]
}
```

If you want to pin the signer to a specific key version, replace `null` with the Azure Key Vault key version string.

### Step 5: Configure the Webhook URL

Update the notification entry in [`config/config.json`](./config/config.json):

```json
{
  "notifications": [
    {
      "url": "your_webhook_url"
    }
  ]
}
```

For testing, you can use [Webhook.site](https://webhook.site).

### Step 6: Run the Service

```bash
docker compose -f examples/evm-azure-key-vault-signer/docker-compose.yaml up
```

### Step 7: Test the Service

Verify that the relayer is running:

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

## Troubleshooting

1. Verify the tenant, client, and secret values are correct
2. Verify the Key Vault URL, key name, and key version match your Azure resources
3. Verify the service principal has access to the key for metadata lookup and signing
4. Check relayer logs for Azure Key Vault API or authentication errors

## Additional Resources

- [Azure Key Vault documentation](https://learn.microsoft.com/azure/key-vault/)
- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
