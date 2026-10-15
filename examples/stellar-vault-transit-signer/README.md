# Using HashiCorp Vault Transit for Secure Stellar Transaction Signing in OpenZeppelin Relayer

This example demonstrates how to use HashiCorp Vault's Transit engine to securely sign Stellar transactions in OpenZeppelin Relayer. It uses a Stellar testnet relayer with a Vault Transit Ed25519 key, and includes a Docker Compose setup with Vault running in development mode.

> **Note:** This example uses Vault in development mode, which is not suitable for production. For production deployments, use a properly configured and sealed Vault instance with appropriate security controls.

## Prerequisites

1. [Docker](https://docs.docker.com/get-docker/)
2. [Docker Compose](https://docs.docker.com/compose/install/)
3. [HashiCorp Vault CLI](https://developer.hashicorp.com/vault/tutorials/get-started/install-binary?productSlug=vault&tutorialSlug=getting-started&tutorialSlug=getting-started-install) (optional but recommended)
4. Rust and Cargo installed
5. Git

## Getting Started

### Step 1: Clone the Repository

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Start Vault

Start the Vault service:

```bash
docker compose -f examples/stellar-vault-transit-signer/docker-compose.yaml up vault
```

Vault will run in dev mode and be available at [http://localhost:8200](http://localhost:8200).

### Step 3: Configure the Vault CLI

If you have the Vault CLI installed, point it at the dev server:

```bash
export VAULT_ADDR='http://127.0.0.1:8200'
export VAULT_TOKEN='dev-only-token'
```

### Step 4: Enable Transit and Create a Stellar Signing Key

Enable the Transit engine:

```bash
vault secrets enable transit
```

Create an exportable Ed25519 signing key:

```bash
vault write -f transit/keys/my_signing_key type=ed25519 exportable=true
```

### Step 5: Create the Vault Policy

Create a policy that allows signing and verification with the Transit key:

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

### Step 6: Enable AppRole Authentication

```bash
vault auth enable approle
```

Create an AppRole for the relayer:

```bash
vault write auth/approle/role/my-role \
  policies="transit-sign-policy" \
  token_ttl=1h \
  token_max_ttl=4h
```

### Step 7: Retrieve the Role ID, Secret ID, and Base64 Public Key

Retrieve the AppRole `role_id`:

```bash
vault read auth/approle/role/my-role/role-id
```

Generate a `secret_id`:

```bash
vault write -f auth/approle/role/my-role/secret-id
```

Retrieve the Transit public key from Vault. In the Vault UI, open `transit/my_signing_key`, choose the key version, and copy the public key value.

For this Stellar signer, the `pubkey` field in the relayer config must contain the raw Ed25519 public key encoded as standard base64.

### Step 8: Configure the Relayer Files

Copy the environment example:

```bash
cp examples/stellar-vault-transit-signer/.env.example examples/stellar-vault-transit-signer/.env
```

Update `examples/stellar-vault-transit-signer/.env`:

```env
VAULT_ROLE_ID=your_role_id
VAULT_SECRET_ID=your_secret_id
```

Update `examples/stellar-vault-transit-signer/config/config.json` and set the signer `pubkey` to the base64 public key from Vault.

### Step 9: Generate API and Webhook Keys

Generate an API key:

```bash
cargo run --example generate_uuid
```

Generate a webhook signing key:

```bash
cargo run --example generate_uuid
```

Add both values to `examples/stellar-vault-transit-signer/.env`:

```env
API_KEY=generated_api_key
WEBHOOK_SIGNING_KEY=generated_webhook_key
```

### Step 10: Configure the Webhook URL

Update `examples/stellar-vault-transit-signer/config/config.json` and set `notifications[0].url`.

For testing, you can use [Webhook.site](https://webhook.site).

### Step 11: Start the Relayer and Redis

```bash
docker compose -f examples/stellar-vault-transit-signer/docker-compose.yaml up -d
```

### Step 12: Test the Relayer

Check that the relayer is running:

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

This should return the Stellar relayer and its address derived from the Vault Transit public key.

### Step 13: Test a Stellar Payment Transaction

Submit a Stellar testnet payment transaction:

```bash
curl -X POST http://localhost:8080/api/v1/relayers/stellar-example/transactions \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY" \
  -d '{
    "network": "testnet",
    "operations": [
      {
        "type": "payment",
        "destination": "GDESTINATION_ADDRESS_HERE",
        "asset": { "type": "native" },
        "amount": 1000000
      }
    ],
    "memo": { "type": "text", "value": "Vault Transit test payment" }
  }'
```

This creates a payment of `0.1 XLM` (`1,000,000` stroops) on Stellar testnet, signs it through Vault Transit, and submits it through the relayer.

## Troubleshooting

1. **Vault connection issues**
   - Verify `VAULT_ROLE_ID` and `VAULT_SECRET_ID`
   - Confirm Vault is reachable at `http://localhost:8200`
   - Ensure the Transit engine is mounted at `transit`

2. **Signing failures**
   - Confirm the Transit key is Ed25519
   - Verify the policy grants `update` access to `transit/sign/my_signing_key`
   - Check that the configured `pubkey` is the raw Ed25519 public key in standard base64

3. **Stellar transaction issues**
   - Make sure the destination account is a valid Stellar address
   - Ensure the relayer account has testnet funds before submitting transactions

## Additional Resources

- [HashiCorp Vault Documentation](https://www.vaultproject.io/docs/)
- [AppRole Authentication](https://developer.hashicorp.com/vault/docs/auth/approle)
- [Stellar Documentation](https://developers.stellar.org/)
- [OpenZeppelin Relayer Documentation](https://docs.openzeppelin.com/relayer)
