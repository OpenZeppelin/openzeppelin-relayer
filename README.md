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

## Documentation

- Pre-requisites:

  - You need `antora` `site-generator` and `mermaid` extension to generate the documentation.

  - You can directly install these dependencies by running `cd docs && npm i --include dev`. If you want to install them manually, you can follow the steps mentioned below.
  - Install `antora` locally, you can follow the steps mentioned [here](https://docs.antora.org/antora/latest/install/install-antora/#install-dir), if you already have you can skip this step.
    > Note: If you want to install globally, you can run: <br/> `npm install -g @antora/cli@3.1 @antora/site-generator@3.1 @sntke/antora-mermaid-extension`
  - Verify the installation by running `antora --version` or by running `npx antora --version` if you installed it locally.

- To generate documentation locally, run the following command:

  ```sh
  cargo make rust-antora
  ```

- Site will be generated in `docs/build/site/openZeppelin_relayer/<version>/` directory.

- To view the documentation, open the `docs/build/site/openzeppelin_relayer/<version>/index.html` in your browser.
