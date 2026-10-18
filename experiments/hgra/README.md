# HGRA experiment

Hyperdimensional ranking algebra (`ultra-instinct-hyper`) and the `hgra` feature on
`ultra-instinct-resonance`. Not the product default. Promote only after it beats
`WeightedMatcher` on a large adversarial target-resolution suite.

```bash
cargo test -p ultra-instinct-hyper
cargo test -p ultra-instinct-resonance --features hgra
```

## Scoring notes

- The HGRA semantic term is the weighted matcher's (`recall * (0.5 + 0.5 *
  precision)`, min with the role hit), so exact labels beat supersets in both.
- The hypervector term is one cosine of the bundled query (role, label tokens,
  position, action; equal weight) against the region signature, not the mean of
  per-probe cosines.

Before/after numbers and the ablation: `crates/ultra-instinct-resonance/HGRA_REMEASURE.md`.
Decision: `docs/DECISIONS.md`, "HGRA semantic parity and bundled query".

```bash
cargo test -p ultra-instinct-resonance --features hgra --test hgra_remeasure -- --nocapture
```
