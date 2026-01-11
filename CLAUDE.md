# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

OpenZeppelin Relayer is a multi-chain blockchain transaction relaying service. It enables interaction with EVM, Solana, and Stellar networks through HTTP endpoints, supporting transaction submission, signing, monitoring, and extensible plugins.

**Tech Stack:** Rust 1.88 (Actix-web 4) + TypeScript/Node.js (plugin runtime) + Redis (job queues/caching)

## Build & Development Commands

```bash
# Core Rust commands
cargo build                              # Build the project
cargo run                                # Run locally (requires Redis + config)
cargo test                               # Run all unit tests
cargo test <test_name>                   # Run a specific test
cargo test --features integration-tests --test integration  # Integration tests

# Plugin system (TypeScript)
cd plugins && pnpm install               # Install plugin dependencies
pnpm run build                           # Build sandbox executor
pnpm test                                # Run plugin tests

# Docker
cargo make docker-compose-up             # Start with docker-compose
cargo make docker-compose-down           # Stop services

```

## Architecture

### Layered Structure

```
src/
├── api/           # HTTP routes (Actix-web), OpenAPI specs, middleware
├── bootstrap/     # Service initialization
├── config/        # JSON/env configuration loading
├── domain/        # Business logic (relayers, transactions, policies)
├── jobs/          # Async job processing (Apalis + Redis)
├── models/        # Data structures (Signer, Transaction, Network)
├── repositories/  # Redis-backed storage (immutable at runtime)
├── services/      # Core services
│   ├── plugins/   # Plugin execution system (Rust side)
│   ├── provider/  # Blockchain RPC providers (EVM, Solana, Stellar)
│   ├── signer/    # Transaction signing (KMS, Vault, local keystore)
│   ├── gas/       # Gas estimation
│   └── notification/  # Webhook notifications
└── utils/         # Helpers
```

### Key Abstractions

- **Repository Pattern**: Redis-backed repositories for configs (RelayerRepository, SignerRepository)
- **Service Traits**: Async traits for dependency injection and testability
- **Job Processing**: Apalis-based async job queue

## Code Standards

- **Formatting**: rustfmt with max_width=100, edition 2021
- **Naming**: snake_case functions/vars, PascalCase types, SCREAMING_SNAKE_CASE constants
- **Errors**: Avoid `unwrap()`, use Result with `?` operator, custom error types
- **Async**: Tokio runtime, await eagerly, avoid blocking in async
- **Params**: Prefer `&str` over `&String`, `&[T]` over `&Vec<T>`
- **Logging**: Use `tracing` crate for structured logs
- **Testing**: Define traits for services to enable mocking with `mockall`
- Prefer **idiomatic Rust**.
- Follow `rustfmt` formatting and `clippy` best practices.
- Avoid unnecessary abstractions.
- Keep code readable over clever.
- Use `Option` and `Result` exhaustively.
- Avoid breaking changes unless explicitly requested.
- Document API endpoints with utoipa schemas and regenerate OpenAPI spec when needed with `cargo run --example generate_openapi`

## Documentation
- Add Rust doc comments (`///`) for public items.
- Include examples in doc comments when useful.
- Keep comments concise and factual.

## Testing

```bash
# Run specific test file
cargo test --test <test_file_name>

# Run tests matching pattern
cargo test <pattern>

# Run with single thread (for tests with shared state)
RUST_TEST_THREADS=1 cargo test

# Redis tests (require active Redis)
cargo test -- --ignored

# Plugin-specific Rust tests
cargo test --package openzeppelin-relayer plugins
```

## Debugging Plugins

```bash
# Enable debug logging for plugins
RUST_LOG=openzeppelin_relayer::services::plugins=debug cargo run

# Check plugin health
curl -H "Authorization: Bearer $API_KEY" http://localhost:8080/api/v1/health/plugins
```

## Helper Binaries

```bash
cargo run --example create_key          # Generate signing keys
cargo run --example generate_uuid       # Generate UUIDs for config
cargo run --example generate_encryption_key  # Generate encryption keys
cargo run --example generate_openapi    # Generate OpenAPI spec
```

## When Unsure
- Ask clarifying questions before making large changes.
- If multiple approaches exist, explain trade-offs briefly.
