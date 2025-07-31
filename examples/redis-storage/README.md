# OpenZeppelin Relayer Redis Storage Example

This guide demonstrates how to configure and use the OpenZeppelin Relayer service with Redis for storage.

Currently supported storage types are `in-memory` and `redis`.

The preferred storage type is selected by setting the `REPOSITORY_STORAGE_TYPE` environment variable value.

When the service starts, if the storage is empty, the config will be loaded from the config file on service startup and stored in storage. On subsequent service starts, the config will not be loaded as the storage already contains values.

In cases when the config file should remain the source of truth, we can always load the config file, clean up previous entries, and load new ones by setting the environment variable `RESET_STORAGE_ON_START=true`.

## Getting Started

### Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)
- Rust (for key generation tools)

### Step 1: Clone the Repository

Clone this repository to your local machine:

```bash
git clone https://github.com/OpenZeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

### Step 2: Create a Signer

Create a new signer keystore using the provided key generation tool:

```sh
cargo run --example create_key -- \
  --password <DEFINE_YOUR_PASSWORD> \
  --output-dir examples/redis-storage/config/keys \
  --filename local-signer.json
```

Note: Replace <DEFINE_YOUR_PASSWORD> with a strong password for the keystore.

Create the `examples/redis-storage/.env` file from `examples/redis-storage/.env.example`.

```bash
cp examples/redis-storage/.env.example examples/redis-storage/.env
```

Then, update the `KEYSTORE_PASSPHRASE` field in the `examples/redis-storage/.env` file with the password you used.

### Step 3: Configure Notifications

#### Configure Webhook URL

The `examples/redis-storage/config/config.json` file is partially pre-configured. You need to specify the webhook URL that will receive updates from the relayer service.

For simplicity, visit [Webhook.site](https://webhook.site), copy your unique URL, and then update the notifications[0].url field in `examples/redis-storage/config/config.json` with this value.


#### Configure Webhook Signing Key

To sign webhook notification payloads, populate the `WEBHOOK_SIGNING_KEY` entry in the `examples/redis-storage/.env` file.

For development purposes, you can generate the signing key using:

```bash
cargo run --example generate_uuid
```
> Note: Alternatively, you can use any online UUID generator.


Copy the generated UUID and update the `WEBHOOK_SIGNING_KEY` entry in the `examples/redis-storage/.env` file.


### Step 4: Configure API Key

Generate an API key signing key for development purposes using:

```bash
cargo run --example generate_uuid
```
> Note: Alternatively, you can use any online UUID generator.


Copy the generated UUID and update the `API_KEY` entry in the `examples/redis-storage/.env` file.


### Step 5: Generate Storage Encryption Key

The storage encryption key is used to encrypt sensitive values at rest.

```bash
cargo run --example generate_encryption_key
```
> Note: Alternatively, you can use `openssl rand -base64 32` for example.

Copy the generated encryption key and update the `STORAGE_ENCRYPTION_KEY` entry in the `examples/redis-storage/.env` file.

### Step 6: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/redis-storage/docker-compose.yaml up
```

### Step 7: Test the Relayer

The service is available at `http://localhost:8080/api/v1`

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```

### Transaction Expiration from Storage

In order to clean up space used by transactions, transactions are deleted after 4 hours by default once they reach their final state. In cases when they should be kept in storage for a longer time, this setting can be overridden by setting the `TRANSACTION_EXPIRATION_HOURS` environment variable. 



