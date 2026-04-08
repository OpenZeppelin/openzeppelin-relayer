# Midnight Support Migration Guide

Reference document for rebuilding Midnight support on top of current `main`, using
the old `plat-6527-midnight-support` branch only as source material.

## Preferred Integration Strategy

Midnight should be introduced behind `cfg(feature = "midnight")` from the start.

Why:
- It keeps default builds lightweight for users that only need EVM, Solana, and Stellar.
- It isolates the Midnight dependency stack while the integration is incomplete.
- It gives the codebase a clean seam for future work without forcing an immediate
  full per-network feature-gating refactor.

Implications:
- Runtime config is not enough for lightweight builds; Cargo features are required.
- Only Midnight should be feature-gated now.
- Full feature-gating for all networks should remain a separate follow-up project.

## Current Recommendation

Do not revive the stale branch as an implementation branch.

Build from a fresh branch off `main` and:
1. Add the `midnight` Cargo feature.
2. Gate Midnight-only modules, enum variants, and dependencies with that feature.
3. Port the safe foundation pieces first.
4. Rebuild provider, signer, sync, and transaction runtime pieces against current Midnight APIs.

## Reuse / Adapt / Rewrite

### Reuse

These are mostly framework-level or dependency-light and should be ported early:
- config parsing shape
- network model shape
- request/response enums and tagged-variant integration
- relayer-state persistence concept
- dual hash tracking concept
- ledger-context persistence concept

### Adapt

These preserve structure but need current Midnight API updates:
- address model
- signer
- relayer orchestration
- sync manager and indexer client
- docs and examples

### Rewrite

These are tightly coupled to obsolete Midnight runtime APIs and should not be ported verbatim:
- provider submission pipeline
- transaction builder
- transaction lifecycle implementation

## Dependency Direction

The stale branch was built on older Midnight dependencies and must not be copied as-is.

Target direction:
- use current `midnight-node` tags
- use `midnight-node-metadata` instead of the removed metadata export path
- replace the deleted `midnight-ledger-prototype` dependency chain with current crates
- align `subxt` with the current Midnight node metadata crate

## Implementation Order

### Phase 0

- Add `midnight` feature and gate all Midnight code behind it.
- Add Midnight config/model foundations without changing default builds.
- Keep `cargo check` passing without the feature enabled.

### Phase 1

- Port config and model layer:
  - Midnight network config
  - config-file network type
  - network collection handling
  - request/response data shapes
  - relayer-state repository

### Phase 2

- Port signer and network-model integration.
- Add relayer policy and transaction repository data shapes.

### Phase 3

- Rebuild provider against current Subxt metadata and current Midnight pallets.
- Rebuild indexer client against current API version.
- Rebuild sync and transaction lifecycle.

### Phase 4

- Verify end-to-end behavior.
- Revalidate fee assumptions.
- Update docs/examples for current Midnight infrastructure.
