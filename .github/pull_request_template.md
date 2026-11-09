## What and why

<!-- One paragraph. Name the anti-drift rule (AGENTS.md) when the change touches the agent path. -->

## Checks run locally (toolchain 1.99.0)

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo nextest run --workspace --locked` (+ `cargo test --workspace --all-features --doc`)
- [ ] `python3 scripts/check_architecture.py` and `scripts/check_repo_rules.sh`
- [ ] `cargo deny check advisories`

## Verification impact

> Any change to observable semantics on this path names the boundary it
> affects (gate, ticket, executor, world, policy, text) and the test that
> owns it — AGENTS.md anti-drift block.

```text
[ ] Pure Rust deterministic behavior
[ ] Concurrency / interleaving
[ ] System model
[ ] Crash-recovery / replay
[ ] Persistence
[ ] TLA+
[ ] Proof kernel
[ ] Workflow / DSL
[ ] Unsafe / memory
[ ] Property-test / fuzz surface
[ ] No verification architecture impact

Reason:
Affected invariants:
Tests or proofs updated:
```

Miri, Loom, Kani, TLA+ and Lean boxes stay empty on purpose: `#![forbid(unsafe_code)]`
everywhere, the CDP client is blocking and single-threaded, and there is no recovery
protocol (AGENTS.md anti-drift #10). If this PR introduces any of those, update the
anti-drift block in `AGENTS.md` first.
