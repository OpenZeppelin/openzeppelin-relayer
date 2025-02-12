# OpenZeppelin Relayer

> :warning: This software is in alpha. Use in production environments at your own risk.
This relayer service offers multi-chain support and an extensible architecture for adding new chains.

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
  ```bash
  rustup toolchain install nightly
  rustup component add rustfmt --toolchain nightly
  ```

### Config files

Create `config/config.json` file. You can use `config/config.example.json` as a starting point:

  ```sh
  cp config/config.example.json config/config.json
  ```

Update `.env` file with the correct values

### Starting Redis manually (without docker compose)

Run Redis container:

  ```sh
  docker run --name openzeppelin-redis \
    -p 6379:6379 \
    -d redis:latest
  ```

## Running the relayer locally:

Install dependencies:

  ```sh
  cargo build
  ```

Run relayer:
  ```sh
  cargo run
  ```

### Running services with docker compose

Run the following command to start the services:

  ```sh
  docker-compose up
  ```

 > Note: By default docker compose command uses Dockerfile.development to build the image. If you want to use Dockerfile.production, you can use the following command: `DOCKERFILE=Dockerfile.production docker-compose up`.

## Documentation

Pre-requisites:

  - You need `antora` `site-generator` and `mermaid` extension to generate the documentation.

  - You can directly install these dependencies by running `cd docs && npm i --include dev`. If you want to install them manually, you can follow the steps mentioned below.
  - Install `antora` locally, you can follow the steps mentioned [here](https://docs.antora.org/antora/latest/install/install-antora/#install-dir), if you already have you can skip this step.
    > Note: If you want to install globally, you can run: <br/> `npm install -g @antora/cli@3.1 @antora/site-generator@3.1 @sntke/antora-mermaid-extension`
  - Verify the installation by running `antora --version` or by running `npx antora --version` if you installed it locally.

To generate documentation locally, run the following command:

  ```sh
  cargo make rust-antora
  ```

Site will be generated in `docs/build/site/openZeppelin_relayer/<version>/` directory.

To view the documentation, open the `docs/build/site/openzeppelin_relayer/<version>/index.html` in your browser.
