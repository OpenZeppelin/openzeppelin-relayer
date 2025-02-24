# OpenZeppelin Relayer

[![codecov](https://codecov.io/gh/OpenZeppelin/openzeppelin-relayer/graph/badge.svg?token=HKHIQNSJ6H)](https://codecov.io/gh/OpenZeppelin/openzeppelin-relayer)

> :warning: This software is in alpha. Use in production environments at your own risk.

This relayer service enables interaction with blockchain networks through transaction submissions. It offers multi-chain support and an extensible architecture for adding new chains.

## Features

- TBD

## Supported networks

- Solana
- EVM

## For users

### Installation

View the [Installation](https://openzeppelin-relayer.netlify.app/openzeppelin_relayer/0.1.0/#getting_started) documentation for detailed information. For a quicker introduction, check out the [Quickstart](https://openzeppelin-relayer.netlify.app/openzeppelin_relayer/0.1.0/quickstart) guide. (TBD - fix links)

### Usage

View the [Usage](https://openzeppelin-relayer.netlify.app/openzeppelin_relayer/0.1.0/#running_the_relayer) documentation for more information.

## For Developers

### Technical Overview

TBD

### Project Structure

TBD

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

- Install the nightly toolchain:

  ```sh
  rustup toolchain install nightly
  rustup component add rustfmt --toolchain nightly
  ```

### Config files

Create `config/config.json` file. You can use `config/config.example.json` as a starting point:

```sh
cp config/config.example.json config/config.json
```

Create `config/keys/local-signer.json` and make sure to update this file with the correct values. Check the sample file `config/keys/local-signer.example.json`.

Create `.env` with correct values according to your needs from `.env.example` file.

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

Based on your `.env` file, docker compose may or may not start the metrics server ( within relayer app container), prometheus and grafana.

> Note: If you want to start the metrics server, prometheus and grafana, make sure to set `METRICS_SERVER_ENABLED=true` in your `.env` file.

If you want to start the services using [make](./Makefile.toml) target, you can use the following command to start the services:

```sh
cargo make docker-compose-up
```

> Note: By default docker compose command uses Dockerfile.development to build the image. If you want to use Dockerfile.production, you can set: `DOCKERFILE=Dockerfile.production` before running `cargo make docker-compose-up`.

We have a [make](./Makefile.toml) target to start the services with docker compose with metrics profile based on your `.env` file. For metrics server you will need to make sure `METRICS_SERVER_ENABLED=true` is set in your `.env` file. If you want to start the services directly using docker compose, you can use the following command:

```sh
# without metrics profile ( METRICS_SERVER_ENABLED=false by default )
# will only start the relayer app container and redis container
docker compose up -d
# or with metrics profile ( METRICS_SERVER_ENABLED=true in .env file )
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

### Observability

- Currently we support logs and metrics ( uses prometheus and grafana) for the relayer server.

## Logs

- For logs, our app defaults to writing logs to stdout/console. You can also configure it to write logs to a file pathn by setting `LOG_MODE` to `file`. See [docker compose file](./docker-compose.yaml) for more details.

## Metrics

- Metrics server is started on port `8081` by default, which collects the metrics from the relayer server.
  - Exposes list of metrics on the `/metrics` endpoint.
  - Exposes `/debug/metrics/scrape` endpoint for prometheus to scrape metrics.

- To view prometheus metrics in a UI, you can use `http://localhost:9090` on your browser.

- To view grafana dashboard, you can use `http://localhost:3000` on your browser.
