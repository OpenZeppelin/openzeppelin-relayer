# OpenZeppelin Relayer Basic Example

This guide demonstrates how to configure and use the OpenZeppelin Relayer service with a basic setup. In this example, we configure and utilize an Ethereum Sepolia Relayer.

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
  --output-dir examples/basic-example/config/keys \
  --filename local-signer.json
```

Note: Replace <DEFINE_YOUR_PASSWORD> with a strong password for the keystore.

Create `examples/basic-example/.env` file from `examples/basic-example/.env.example`.

```bash
cp examples/basic-example/.env.example examples/basic-example/.env
```

Then, update the `KEYSTORE_PASSPHRASE` field in the `examples/basic-example/.env` file with the password you used.

### Step 3: Configure Notifications

#### Configure Webhook URL

`examples/basic-example/config/config.json` file is partially pre-configured. You need to specify the webhook URL that will receive updates from the relayer service.

For simplicity, visit [Webhook.site](https://webhook.site), copy your unique URL, and then update the notifications[0].url field in `examples/basic-example/config/config.json` with this value.

#### Configure Webhook Signing Key

To sign webhook notification payloads, populate the `WEBHOOK_SIGNING_KEY` entry in the `examples/basic-example/.env` file.

For development purposes, you can generate the signing key using:

```bash
cargo run --example generate_uuid
```

> Note: Alternatively, you can use any online UUID generator.

Copy the generated UUID and update the `WEBHOOK_SIGNING_KEY` entry in the `examples/basic-example/.env` file.

### Step 4: Configure API Key

Generate an API key signing key for development purposes using:

```bash
cargo run --example generate_uuid
```

> Note: Alternatively, you can use any online UUID generator.

Copy the generated UUID and update the `API_KEY` entry in the `examples/basic-example/.env` file.

### Step 5: Run the Service

Start the service with Docker Compose:

```bash
docker compose -f examples/basic-example/docker-compose.yaml up
```

### Step 6: Test the Relayer

The service is available at `http://localhost:8080/api/v1`

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer YOUR_API_KEY"
```
