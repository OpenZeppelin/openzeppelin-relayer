# OpenZeppelin Relayer

## Development

### Prerequisites

- Docker
- Rust
- Redis

### Developer setup

1. Run `chmod +x .githooks/*` to make the git hooks executable.
2. Run `git config core.hooksPath .githooks` to setup git hooks for linting and formatting.
3. Run `rustup toolchain install nightly` to install the nightly toolchain.
4. Run `rustup component add rustfmt --toolchain nightly` to install rustfmt for the nightly toolchain.

### Starting Redis manually (without docker compose)

Run Redis container:

```bash
docker run --name openzeppelin-redis \
  -p 6379:6379 \
  -d redis:latest
