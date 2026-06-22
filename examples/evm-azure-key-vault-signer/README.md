# Using Azure Key Vault for Secure EVM Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use an Azure Key Vault hosted secp256k1 key to securely sign EVM transactions in OpenZeppelin Relayer.
The relayer supports three Azure authentication modes:

- `client_secret`
- `managed_identity`
- `workload_identity`

## Prerequisites

1. An Azure subscription with access to Azure Key Vault
2. One of the following Azure identity setups:
   - A Microsoft Entra application (service principal) with a client secret
   - A managed identity with access to the Key Vault
   - A workload identity / federated credential with access to the Key Vault
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
   - Vault URL, for example `https://your-key-vault-name.vault.azure.net`
   - Key name
   - Optional key version
4. Depending on the auth mode, also collect:
   - `client_secret`: tenant ID, client ID, client secret
   - `managed_identity`: optional client ID for a user-assigned identity
   - `workload_identity`: tenant ID, client ID, and optionally a federated token file path if you don't rely on `AZURE_FEDERATED_TOKEN_FILE`
5. Make sure the chosen identity has permission to read the key metadata and sign payloads with it

### Step 3: Configure Environment Variables

For `client_secret`, populate the environment variables used by the example:

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

Update [`config/config.json`](./config/config.json) with your Azure Key Vault details.

Client secret example:

```json
{
  "signers": [
    {
      "id": "azure-key-vault-signer-evm",
      "type": "azure_key_vault",
      "config": {
        "auth_type": "client_secret",
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

Managed identity example:

```json
{
  "id": "azure-key-vault-signer-evm",
  "type": "azure_key_vault",
  "config": {
    "auth_type": "managed_identity",
    "client_id": {
      "type": "env",
      "value": "AZURE_CLIENT_ID"
    },
    "vault_url": "https://your-key-vault-name.vault.azure.net",
    "key_name": "your-secp256k1-key-name",
    "key_version": null
  }
}
```

Workload identity example:

```json
{
  "id": "azure-key-vault-signer-evm",
  "type": "azure_key_vault",
  "config": {
    "auth_type": "workload_identity",
    "tenant_id": {
      "type": "env",
      "value": "AZURE_TENANT_ID"
    },
    "client_id": {
      "type": "env",
      "value": "AZURE_CLIENT_ID"
    },
    "vault_url": "https://your-key-vault-name.vault.azure.net",
    "key_name": "your-secp256k1-key-name",
    "key_version": null
  }
}
```

If you want to pin the signer to a specific key version, replace `null` with the Azure Key Vault key version string.
For workload identity, the relayer reads `AZURE_FEDERATED_TOKEN_FILE` automatically unless you set `federated_token_file` explicitly in the signer config.

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

1. Verify the auth mode and identity values are correct for your deployment
2. Verify the Key Vault URL, key name, and key version match your Azure resources
3. Verify the chosen identity has access to the key for metadata lookup and signing
4. Check relayer logs for Azure Key Vault API or authentication errors

## Additional Resources

- [Azure Key Vault documentation](https://learn.microsoft.com/azure/key-vault/)
- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
