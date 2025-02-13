# OpenZeppelin Relayer

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
  ```bash
  rustup toolchain install nightly
  rustup component add rustfmt --toolchain nightly
  ```

### Config files

Create `config/config.json` file. You can use `config/config.example.json` as a starting point:

  ```sh
  cp config/config.example.json config/config.json
  ```

Create `config/keys/local-signer.json` and make sure to update this file with the correct values. Check the sample file `config/keys/local-signer.example.json`.

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
  docker-compose up -d
  ```

 > Note: By default docker compose command uses Dockerfile.development to build the image. If you want to use Dockerfile.production, you can use the following command: `DOCKERFILE=Dockerfile.production docker-compose up`.

Make sure the containers are running without any restarts/issues:
  ```sh
  docker ps -a
  ```

To stop the services, run the following command:

  ```sh
  docker-compose down
  ```

To check the logs of the services/containers, run the following command:

  ```sh
  docker compose logs -f
  ```

  ```sh
  # for individual container
  docker logs -f <container_name>
  ```
