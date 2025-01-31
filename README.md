# OpenZeppelin Relayer

## Development

### Prerequisites

- Docker
- Rust
- Redis

## Installation

### Local Setup

- Clone the repository:

  ```sh
  git clone https://github.com/openzeppelin/openzeppelin-relayer
  cd openzeppelin-relayer
  ```

- Install dependencies:

  ```sh
  cargo build
  ```

### Developer setup

- Run the following commands to install pre-commit hooks:

  ```bash
   # Use <pipx install pre-commit> if you prefer to install it globally.
   pip install pre-commit
   pre-commit install --install-hooks -t commit-msg -t pre-commit -t pre-push
  ```

  > Note: If you run into issues with pip install, you may need [pipx](https://pipx.pypa.io/stable/installation/) to install pre-commit globally.

- Run `rustup toolchain install nightly` to install the nightly toolchain.
- Run `rustup component add rustfmt --toolchain nightly` to install rustfmt for the nightly toolchain.

### Config file

Create `config/config.json` file before starting service in dev mode `cargo run`.

### Starting Redis manually (without docker compose)

Run Redis container:

  ```sh
  docker run --name openzeppelin-redis \
    -p 6379:6379 \
    -d redis:latest
  ```

### Running services with docker compose

- Make sure to update `.env` file with the correct values.

- Run the following command to start the services:

  ```sh
  docker-compose up
  ```

 > Note: By default docker compose command uses Dockerfile.development to build the image. If you want to use Dockerfile.production, you can use the following command: `DOCKERFILE=Dockerfile.production docker-compose up`.
