# Using HashiCorp Vault for Secret Key Management in OpenZeppelin Relayer

This example demonstrates how to use HashiCorp Vault for securely storing private keys for OpenZeppelin Relayer. It includes a Docker Compose setup with Vault in development mode and instructions for configuring the KV-v2 secrets engine with AppRole authentication.

> **Note:** This example uses Vault in development mode which is not suitable for production environments. For production deployments, use a properly configured and sealed Vault instance with appropriate security measures.


## Getting Started


### Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)
- [HashiCorp Vault CLI](https://developer.hashicorp.com/vault/tutorials/get-started/install-binary?productSlug=vault&tutorialSlug=getting-started&tutorialSlug=getting-started-install) (Optional, for advanced operations)



### Step 1: Clone the Repository

Clone this repository to your local machine:

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```


### Step 2: Start the Docker Compose Vault Service

Start the Vault service with the following command:

```bash
docker compose -f examples/vault-secret-signer/docker-compose.yaml up vault

```

Vault will run in dev mode and bind to `0.0.0.0:8200`. You can access its UI by navigating to [http://localhost:8200](http://localhost:8200) in your browser.


### Step 3: Install and Configure the Vault CLI (Optional)

If you haven't already, install the Vault CLI by following the instructions in the [Vault installation guide](https://developer.hashicorp.com/vault/tutorials/get-started/install-binary?productSlug=vault&tutorialSlug=getting-started&tutorialSlug=getting-started-install).

Set the necessary environment variables so that your CLI can communicate with Vault:

```bash
export VAULT_ADDR='http://0.0.0.0:8200'
export VAULT_TOKEN='dev-only-token'  # This is the default token for dev mode defined in docker-compose fi;e
```


### Step 4: Enable the KV-v2 Secrets Engine

Enable the KV-v2 secrets engine at the `secret` path:

```bash
vault secrets enable -path=secret kv-v2
```


### Step 5: Create secret

Create secret with private key value:

```bash
vault kv put secret/my-app value=REPLACE_WITH_PRIVATE_KEY
```

Note: For Solana, we can use this [tool](https://cyphr.me/ed25519_tool/ed.html) for development purposes to generate a key.


### Step 6: Create a Vault Policy

Create a policy that grants your service permissions to manage secrets. Save the following policy as `secret-policy` in Vault:

```bash
vault policy write secret-policy - <<EOF
path "secret/data/*" {
  capabilities = ["create", "read", "update", "delete"]
}

path "secret/metadata/*" {
  capabilities = ["list"]
}
EOF
```

This policy allows:
- Data operations (create, read, update, delete) on `secret/data/*`
- Listing of secrets via the metadata endpoint `secret/metadata/*`


### Step 7: Enable AppRole Authentication

Enable the AppRole authentication method in Vault, which allows your service to authenticate using a RoleID and SecretID:

```bash
vault auth enable approle
```


### Step 8: Create an AppRole

Create an AppRole and attach the `secret-policy` to it:

```bash
vault write auth/approle/role/my-role \
  policies="secret-policy" \
  token_ttl=1h \
  token_max_ttl=4h
```


### Step 9: Retrieve the RoleID and SecretID

Retrieve the RoleID for your AppRole(store these values as they are needed for next step):

```bash
vault read auth/approle/role/my-role/role-id
```

Update

Then, generate a SecretID for your AppRole:

```bash
vault write -f auth/approle/role/my-role/secret-id
```

Use these credentials within your application to authenticate with Vault and access secrets securely.


### Step 10: Configure Your Service to Use Vault

Now that you have set up Vault with the appropriate permissions, configure your OpenZeppelin Relayer service to use the Vault credentials for authentication.

Create an environment file by copying the example:

```bash
cp examples/vault-secret-signer/.env.example examples/vault-secret-signer/.env
```

Then edit this file and update the following variables with the values obtained in the previous step:

`VAULT_ROLE_ID`: The Role ID retrieved from Vault
`VAULT_SECRET_ID`: The Secret ID generated from Vault

Generate random keys for API authentication and webhook signing by running the UUID generation script twice:
```bash
cargo run --example generate_uuid
```

Then update the following fields in your .env file:

`WEBHOOK_SIGNING_KEY`: The key used for signing webhook notifications
`API_KEY`: They api key used to authorize requests


### Step 11: Configure Webhook URL

`examples/vault-secret-signer/config/config.json` file is partially pre-configured. You need to specify the webhook URL that will receive updates from the relayer service.

For simplicity, visit [Webhook.site](https://webhook.site), copy your unique URL, and then update the notifications[0].url field in `examples/vault-secret-signer/config/config.json` with this value.


### Step 12: Configure Webhook Signing Key

To sign webhook notification payloads, populate the `WEBHOOK_SIGNING_KEY` entry in the `examples/vault-secret-signer/.env` file.

For development purposes, you can generate the signing key using:

```bash
cargo run --example generate_uuid
```
> Note: Alternatively, you can use any online UUID generator.


Copy the generated UUID and update the `WEBHOOK_SIGNING_KEY` entry in the `examples/vault-secret-signer/.env` file.


### Step 13: Configure API Key

Generate an API key signing key for development purposes using:

```bash
cargo run --example generate_uuid
```
> Note: Alternatively, you can use any online UUID generator.


Copy the generated UUID and update the `API_KEY` entry in the `examples/vault-secret-signer/.env` file.


### Step 14: Start Relayer and Redis services

Start remaining docker-compose service with command:

```
docker compose -f examples/vault-secret-signer/docker-compose.yaml up -d

```

### Step 15: Test the Relayer

The service is available at `http://localhost:8080/api/v1`

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

### Additional Resources

- [HashiCorp Vault Documentation](https://www.vaultproject.io/docs/)
- [KV Secrets Engine - Version 2](https://www.vaultproject.io/docs/secrets/kv/kv-v2)
- [AppRole Authentication](https://www.vaultproject.io/docs/auth/approle)


## Troubleshooting

- **Vault UI Not Accessible:**
  Ensure that the Vault container is running and that the command includes `-dev-listen-address=0.0.0.0:8200` so Vault binds to all network interfaces.

- **Permission Denied Errors:**
  Verify that the policy includes both `secret/data/*` and `secret/metadata/*` paths. Also, ensure that the policy name provided during AppRole creation matches exactly with the policy you created.

- **Connectivity Issues:**
  Confirm that environment variables such as `VAULT_ADDR` and `VAULT_TOKEN` are correctly set.
