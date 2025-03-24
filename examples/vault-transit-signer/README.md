# Using HashiCorp Vault Transit for Secure Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use HashiCorp Vault's Transit engine to securely sign transactions in OpenZeppelin Relayer. It includes a Docker Compose setup with Vault running in development mode and provides detailed instructions for configuring the Transit engine with AppRole authentication.

> **Note:** This example uses Vault in development mode, which is not suitable for production. For production deployments, use a properly configured and sealed Vault instance with appropriate security measures.


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
docker compose -f examples/vault-transit-signer/docker-compose.yaml up vault

```

Vault will run in dev mode and bind to `0.0.0.0:8200`. You can access its UI by navigating to [http://localhost:8200](http://localhost:8200) in your browser.


### Step 3: Install and Configure the Vault CLI (Optional)

If you haven't already, install the Vault CLI by following the instructions in the [Vault installation guide](https://developer.hashicorp.com/vault/tutorials/get-started/install-binary?productSlug=vault&tutorialSlug=getting-started&tutorialSlug=getting-started-install).

Set the necessary environment variables so that your CLI can communicate with Vault:

```bash
export VAULT_ADDR='http://0.0.0.0:8200'
export VAULT_TOKEN='dev-only-token'  # This is the default token for dev mode defined in docker-compose fi;e
```


### Step 4: Enable the Transit Engine

Enable the Transit engine at transit path

```bash
vault secrets enable transit
```


### Step 5: Create an Ed25519 Signing Key

Create a policy that grants your service permissions to manage secrets. Save the following policy as `secret-policy` in Vault:

```bash
vault write -f transit/keys/my_signing_key type=ed25519 exportable=true
```


### Step 6: Create a Vault Policy

Create a policy that grants your service permissions to sign and verify using generated key. Save the following policy as `transit-sign-policy` in Vault:

```bash
vault policy write transit-sign-policy -<<EOF
path "transit/sign/my_signing_key" {
capabilities = ["update"]
}

path "transit/verify/my_signing_key" {
capabilities = ["update"]
}
EOF
```


### Step 7: Enable AppRole Authentication

Enable the AppRole authentication method in Vault, which allows your service to authenticate using a RoleID and SecretID:

```bash
vault auth enable approle
```


### Step 8: Create an AppRole

Create an AppRole and attach the `transit-sign-policy` to it:

```bash
vault write auth/approle/role/my-role \
  policies="transit-sign-policy" \
  token_ttl=1h \
  token_max_ttl=4h
```


### Step 9: Retrieve the RoleID, SecretID and Public Key

Retrieve Public key(store these values as they are needed for next step) by opening vault UI(localhost:8200).

Sign in with token `dev-only-token`. Navigate to http://localhost:8200/ui/vault/secrets/transit/show/my_signing_key?tab=versions and copy public key value from version dropdown menu.



Retrieve the RoleID for your AppRole:

```bash
vault read auth/approle/role/my-role/role-id
```

Then, generate a SecretID for your AppRole:

```bash
vault write -f auth/approle/role/my-role/secret-id
```


### Step 10: Configure the Relayer Service


#### Create and Update Environment File

Create an environment file by copying the example:

```bash
cp examples/vault-transit-signer/.env.example examples/vault-transit-signer/.env
```

Edit the `examples/vault-transit-signer/.env` file and update the following variables:

`VAULT_ROLE_ID`: The Role ID retrieved from Vault
`VAULT_SECRET_ID`: The Secret ID generated from Vault


#### Update Configuration File

Update `examples/vault-transit-signer/config/config.json` file. Replace `pubkey` placeholder value with `pubkey` value from step 9.


#### Generate Security Keys

Generate random keys for API authentication and webhook signing:


```bash
# Generate API key
cargo run --example generate_uuid

# Generate webhook signing key
cargo run --example generate_uuid

```

Then update the following fields in your .env file:

`WEBHOOK_SIGNING_KEY`: The key used for signing webhook notifications
`API_KEY`: They api key used to authorize requests


### Step 11 Configure Webhook URL

`examples/vault-transit-signer/config/config.json` file is partially pre-configured. You need to specify the webhook URL that will receive updates from the relayer service.

For simplicity, visit [Webhook.site](https://webhook.site), copy your unique URL, and then update the notifications[0].url field in `examples/vault-transit-signer/config/config.json` with this value.


### Step 12: Configure Webhook Signing Key

To sign webhook notification payloads, populate the `WEBHOOK_SIGNING_KEY` entry in the `examples/vault-transit-signer/.env` file.

For development purposes, you can generate the signing key using:

```bash
cargo run --example generate_uuid
```
> Note: Alternatively, you can use any online UUID generator.


Copy the generated UUID and update the `WEBHOOK_SIGNING_KEY` entry in the `examples/vault-transit-signer/.env` file.


### Step 13: Configure API Key

Generate an API key signing key for development purposes using:

```bash
cargo run --example generate_uuid
```
> Note: Alternatively, you can use any online UUID generator.


Copy the generated UUID and update the `API_KEY` entry in the `examples/vault-transit-signer/.env` file.


### Step 14: Start Relayer and Redis services

Start remaining docker-compose service with command:

```
docker compose -f examples/vault-transit-signer/docker-compose.yaml up -d
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
- [AppRole Authentication](https://www.vaultproject.io/docs/auth/approle)

## Troubleshooting

- **Vault UI Not Accessible:**
  Ensure that the Vault container is running and that the command includes `-dev-listen-address=0.0.0.0:8200` so Vault binds to all network interfaces.

- **Permission Denied Errors:**
  Verify that the policy includes both `secret/data/*` and `secret/metadata/*` paths. Also, ensure that the policy name provided during AppRole creation matches exactly with the policy you created.

- **Connectivity Issues:**
  Confirm that environment variables such as `VAULT_ADDR` and `VAULT_TOKEN` are correctly set.
