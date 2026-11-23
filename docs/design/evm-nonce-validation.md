# Atomic EVM nonce allocation validation

The primary-body/counter/index allocator was independently reviewed and exercised against real Redis and the relayer binary. This report records local qualification, not deployment approval.

## Tested artifacts

- Upstream runtime source: `306f07e1bf76ece68c56d8333870a26d31f64a5c` (v1.9.0).
- Tested source commit: `faa55d61db8afeb553f2eaa360169999ea13d190`.
- Exact binary-patch SHA-256: `3ad8e9fa0dba45fc9d191e75a9f1326789054923a6ed227677fda32c5a22eed8`.
- AMD64 image: `sha256:e2ed9ea387ee3f7a475c17c8cff312abccbc5e5a6b2bb45c3edde914ecdef62e`.
- Binary SHA-256: `41d362d97b083f9b8d2269151cf9dd7a35f71dd458f176b25fc184979728a2b4`.
- Allocation Lua SHA-1: `7a9f242dc653970d66c6d4e7f8e8a0d517dd0573`.

The image reports amd64; execution reports x86_64 and ELF64 machine62. Docker emulated linux/amd64 on an ARM64 host. This is not native AMD64 hardware qualification. Redis and private Anvil use the existing disposable fixture architectures.

## Results

| Check | Result |
|---|---|
| New real-Redis integration tests | 7 passed |
| Existing EVM transaction unit tests | 36 passed; full suite not run |
| Unmodified AMD64 allocation reply-loss negative | Failed as expected: original nonce0 became nonce1 |
| Patched successful allocation reply held, OZ and Redis SIGKILL | Passed: original UUID/nonce0 mined, distinct next UUID/nonce1 mined, counter2 |
| Patched signed-before-broadcast, dead sender RPC discarded, both processes killed | Passed: native recovery broadcast, nonce0 then nonce1 mined, counter2 |

The strict suite passed 2/2, zero skips, in 42.060s at its original reviewed harness revision. The successful allocation fault requires EVALSHA of the exact script, matching counter key and integer-one response; a CAS nil refusal cannot consume it. Redis retains its AOF across death. No filler, counter rewind or replacement UUID is used to satisfy the original request. Owned fixture cleanup was empty after all three runs.

Run the repository tests against disposable Redis using `REDIS_URL=redis://127.0.0.1:6379 cargo test --release --features redis-tls-native,serde_json/arbitrary_precision --test nonce_atomic -- --ignored --test-threads=1`. Do not point the test at shared or live Redis.

## Independent review

A separate read-only agent found no confirmed introduced blocker in the supported single-primary Redis path for initially unassigned Pending work. It checked primary identity/payer/network/full-payload binding, Pending/canceled validation, exact integer preservation, body/counter/scope CAS immediately before the write group, index visibility, malformed/missing/overflow/occupied input refusal and no Redis-error fallback.

One nonblocking harness finding was reproduced and repaired: CAS nil could consume an intended successful-commit fault. The fixed loopback test passes nil then one and gates only one; restoring the old request-only context fails. This was not a demonstrated false pass of the previous serial gate. The separate review is agent review, not upstream maintainer approval.

## Rebase and boundaries

Publication rebased the source onto upstream main `2c0c317c79778c4b9e9568c3c6c394179e80e148`. The allocator, EVM transaction, model, repository, job and service trees remain byte-identical to the tested source; independent review confirmed continuity at `872db1cd32428897c18fbc882f0c0e6dc897f8f4`. Upstream changes include dependencies and Stellar/configuration code. At original publication, only the pinned v1.9.0 candidate had runtime qualification. The follow-up below qualifies the rebased source separately.

Existing assigned-nonce inputs bypass the new allocator; stale postsign writes and cancellation of allocated Pending work retain existing behavior. No claim is made for exactly-once external signing, universal gap prevention, mixed old/new writers, rollback, Redis Cluster/failover, disk loss or repair of existing gaps. No staging or production change was made.

## Rebased-main follow-up — 2026-11-12

The exact published PR source `3ef2f4cc6500f5afeaa39df158f5caa9a41b44d5`, based on upstream main `2c0c317c79778c4b9e9568c3c6c394179e80e148`, was built with its locked dependencies and the same TLS/exact-JSON features. The allocator patch was reverified against the rebased base; it retains SHA-256 `3ad8e9fa0dba45fc9d191e75a9f1326789054923a6ed227677fda32c5a22eed8`.

- Image: `sha256:e7f9d011b3c92ab05cfe4cb12a616d8047f729d2a790146cb052bd939e2bc673`.
- Binary SHA-256: `71a612870afffe20629a09fdad022ac371d68a8d5419f9ea45425c710642240c`.
- Actual x86_64 runtime and ELF64 machine62 verified under Docker AMD64 emulation on ARM64.
- Strict recovery: **2/2 passed, zero skips, 40.592s**, at clean harness revision `8eec1efba7f41e0f5528b3ebaf9d1855ead1801e`.
- Both successful-atomic-reply loss and signed-before-broadcast deaths recovered the original UUID at nonce0, mined a distinct request at nonce1, and finished at counter2. Both owned cleanups were empty; sanitized export checksums verified.

This closes the previously unverified rebased-main binary check. It adds no staging, native-hardware, rollout or universal nonce-liveness qualification. Follow-up preparation tests exercise reserved `Some(assigned)` and reservation errors through the signer boundary; they change only test code, leaving the qualified production implementation unchanged.
