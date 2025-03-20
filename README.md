# OpenZeppelin Relayer

[![codecov](https://codecov.io/gh/OpenZeppelin/openzeppelin-relayer/graph/badge.svg?token=HKHIQNSJ6H)](https://codecov.io/gh/OpenZeppelin/openzeppelin-relayer)

> :warning: This software is in alpha. Use in production environments at your own risk.

This relayer service enables interaction with blockchain networks through transaction submissions. It offers multi-chain support and an extensible architecture for adding new chains.

## Features

- **Multi-Chain Support**: Interact with multiple blockchain networks, including Solana and EVM-based chains.
- **Transaction Relaying**: Submit transactions to supported blockchain networks efficiently.
- **Transaction Signing**: Securely sign transactions using configurable key management.
- **Transaction Fee Estimation**: Estimate transaction fees for better cost management.
- **Solana Gasless Transactions**: Support for gasless transactions on Solana, enabling users to interact without transaction fees.
- **Transaction Nonce Management**: Handle nonce management to ensure transaction order.
- **Transaction Status Monitoring**: Track the status of submitted transactions.
- **Extensible Architecture**: Easily add support for new blockchain networks.
- **Configurable Network Policies**: Define and enforce network-specific policies for transaction processing.
- **Metrics and Observability**: Monitor application performance using Prometheus and Grafana.
- **Docker Support**: Deploy the relayer using Docker for both development and production environments.

## Supported networks

- Solana
- EVM

## For users

### Installation

View the [Installation](https://openzeppelin-relayer.netlify.app/openzeppelin_relayer/0.1.0/#getting_started) documentation for detailed information. For a quicker introduction, check out the [Quickstart](https://openzeppelin-relayer.netlify.app/openzeppelin_relayer/0.1.0/quickstart) guide.

### Usage

View the [Usage](https://openzeppelin-relayer.netlify.app/openzeppelin_relayer/0.1.0/#running_the_relayer) documentation for more information.

## For Developers

### Technical Overview

The OpenZeppelin Relayer is built using Actix-web and provides HTTP endpoints for transaction submission, in-memory repository implementations, and configurable network policies.

### Project Structure

The project follows a standard Rust project layout:

- `src/`: Source code
  - `api/`: Route and controllers logic
  - `config/`: Configuration logic
  - `constants/`: Constant values used in the system
  - `domain/`: Domain logic
  - `init/`: Service initialization logic
  - `jobs/`: Asynchronous processing logic(queueing)
  - `logging/`: Logs File rotation logic
  - `metrics/`: Metrics logic
  - `models/`: Data structures and types
  - `repositories/`: Configuration storage
  - `services/`: Services logic
  - `utils/`: Helper functions
- `config/`: Configuration files
- `tests/`: Integration tests
- `docs/`: Documentation
- `scripts/`: Utility scripts
- `examples/`: Configuration examples
- `helpers/`: Rust helper scripts

### Prerequisites

- Docker
- Rust
- Redis

### Setup

To get started, clone the repository:

```sh
git clone https://github.com/openzeppelin/openzeppelin-relayer
cd openzeppelin-relayer
```

Run the following commands to install pre-commit hooks:

- Install pre-commit hooks:

  ```bash
  pip install pre-commit
  pre-commit install --install-hooks -t commit-msg -t pre-commit -t pre-push
  ```

  > :warning: If you encounter issues with pip, consider using [pipx](https://pipx.pypa.io/stable/installation/) for a global installation.

- Install the toolchain:

  ```sh
  rustup component add rustfmt
  ```

### Run Tests

To run tests, use the following commands:

```bash
cargo test
cargo test properties
cargo test integration
```

### Config files

Create `config/config.json` file. You can use `config/config.example.json` as a starting point:

```sh
cp config/config.example.json config/config.json
```

Refer to the [Configuration References](https://openzeppelin-relayer.netlify.app/openzeppelin_relayer/0.1.0/#configuration_references) section for a complete list of configuration options.

Create `config/keys/local-signer.json` and make sure to update this file with the correct values. Check the sample file `config/keys/local-signer.example.json`.

Create `.env` with correct values according to your needs from `.env.example` file.

### Creating a Signer

To create a new signer keystore, use the provided key generation tool:

```sh
cargo run --example create_key -- \
    --password DEFINE_YOUR_PASSWORD \
    --output-dir config/keys \
    --filename local-signer.json
```

The tool supports the following options:

- `--password`: Required. Must contain at least:
  - 12 characters
  - One uppercase letter
  - One lowercase letter
  - One number
  - One special character
- `--output-dir`: Directory for the keystore file (creates if not exists)
- `--filename`: Optional. Uses timestamp-based name if not provided
- `--force`: Optional. Allows overwriting existing files

Example with all options:

```sh
cargo run --example create_key -- \
    --password "YourSecurePassword123!" \
    --output-dir config/keys \
    --filename local-signer.json \
    --force
```

### Starting Redis manually (without docker compose)

Run Redis container:

```sh
docker run --name openzeppelin-redis \
  -p 6379:6379 \
  -d redis:latest
```

## Running the relayer locally

Install dependencies:

```sh
cargo build
```

Run relayer:

```sh
cargo run
```

### Running services with docker compose

If you use `docker-compose` over `docker compose` please read [Compose V1 vs Compose V2](#compose-v1-vs-compose-v2) section.

Based on your `.env` file, docker compose may or may not start the metrics server ( within relayer app container), prometheus and grafana.

> Note: If you want to start the metrics server, prometheus and grafana, make sure to set `METRICS_ENABLED=true` in your `.env` file.

If you want to start the services using [make](./Makefile.toml) target, you can use the following command to start the services:

```sh
cargo make docker-compose-up
```

> Note: By default docker compose command uses Dockerfile.development to build the image. If you want to use Dockerfile.production, you can set: `DOCKERFILE=Dockerfile.production` before running `cargo make docker-compose-up`.

We have a [make](./Makefile.toml) target to start the services with docker compose with metrics profile based on your `.env` file. For metrics server you will need to make sure `METRICS_ENABLED=true` is set in your `.env` file. If you want to start the services directly using docker compose, you can use the following command:

```sh
# without metrics profile ( METRICS_ENABLED=false by default )
# will only start the relayer app container and redis container
docker compose up -d
# or with metrics profile ( METRICS_ENABLED=true in .env file )
# docker compose --profile metrics up -d
```

Make sure the containers are running without any restarts/issues:

```sh
docker ps -a
```

To stop the services, run the following command:

```sh
cargo make docker-compose-down
# or
# using docker compose without make target
# without metrics profile
# docker compose down
# or with metrics profile
# docker compose --profile metrics down
```

To check the logs of the services/containers, run the following command:

```sh
docker compose logs -f
```

## Compose V1 vs Compose V2

- If you use `docker-compose` command, it will use Compose V1 by default which is deprecated. We recommend using `docker compose` command.
- You can read more about the differences between Compose V1 and Compose V2 [here](https://docs.docker.com/compose/intro/history/).
- You can also check out the issue [here](https://github.com/OpenZeppelin/openzeppelin-relayer/issues/64).

## Documentation

- Pre-requisites:

  - You need `antora` `site-generator` and `mermaid` extension to generate the documentation.

  - You can directly install these dependencies by running `cd docs && npm i --include dev`. If you want to install them manually, you can follow the steps mentioned below.
  - Install `antora` locally, you can follow the steps mentioned [here](https://docs.antora.org/antora/latest/install/install-antora/#install-dir), if you already have you can skip this step.
    > Note: If you want to install globally, you can run:
    >
    > `npm install -g @antora/cli@3.1 @antora/site-generator@3.1 @sntke/antora-mermaid-extension`
  - Verify the installation by running `antora --version` or by running `npx antora --version` if you installed it locally.

- To generate documentation locally, run the following command:

  ```sh
  cargo make rust-antora
  ```

- Site will be generated in `docs/build/site/openZeppelin_relayer/<version>/` directory.

- To view the documentation, open the `docs/build/site/openzeppelin_relayer/<version>/index.html` in your browser.

## Observability

- Currently we support logs and metrics ( uses prometheus and grafana) for the relayer server.

### Logs

- For logs, our app defaults to writing logs to stdout/console. You can also configure it to write logs to a file pathn by setting `LOG_MODE` to `file`. See [docker compose file](./docker-compose.yaml) for more details.

### Metrics

- Metrics server is started on port `8081` by default, which collects the metrics from the relayer server.

  - Exposes list of metrics on the `/metrics` endpoint.
  - Exposes `/debug/metrics/scrape` endpoint for prometheus to scrape metrics.

- To view prometheus metrics in a UI, you can use `http://localhost:9090` on your browser.

- To view grafana dashboard, you can use `http://localhost:3000` on your browser.
