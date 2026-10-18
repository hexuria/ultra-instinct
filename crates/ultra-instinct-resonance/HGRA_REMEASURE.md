# HGRA remeasure: semantic parity and bundled query

Status: 2026-10-05 (Asia/Manila). Branch `refactor/action-firewall`.

## What changed

| Term | Before | After |
|---|---|---|
| HGRA semantic | `mean(token_recall(text, label), role_hit)` | `min(recall * (0.5 + 0.5 * precision), role_hit)`, the same `weighted_semantic` the weighted matcher calls |
| HGRA hypervector | `mean_i cosine(probe_i, signature)`, one probe per role, label token, position, action | `cosine(bundle(probe_i, weight 1), signature)`, one bundled query vector and one cosine |

Unchanged: region signature composition, `ResonanceModel::V1` weights,
penalties, `TEXT_MISS_CAP`, `temporal_stability` (still read from the region),
and the empty-query behaviour (no constraint scores the hypervector term `1`).

## Corpus (`evals/locate/cases.tsv`, `Dims::DEFAULT` = 2048)

Gate is the product act gate: top >= 0.55 and margin >= 0.05. Totals are not
calibrated across matchers. Compare tops, margins, and gate decisions.

| case | weighted top / margin / gate | HGRA before: top / total / margin / gate | HGRA after: top / total / margin / gate |
|---|---|---|---|
| send | z-send / 0.1250 / allow | z-send / 0.7209 / **0.0277 / refuse** | z-send / 0.7309 / **0.0859 / allow** |
| sidebar | nav-settings / 0.3000 / allow | nav-settings / 0.6944 / 0.1324 / allow | nav-settings / 0.7729 / 0.1718 / allow |
| export | z-export / 0.2500 / allow | z-export / 0.6732 / 0.2232 / allow | z-export / 0.6780 / 0.2500 / allow |
| admin | z-admin / 0.4500 / allow | z-admin / 0.6811 / 0.2311 / allow | z-admin / 0.6848 / 0.2916 / allow |
| undo | z-undo / 0.3500 / allow | z-undo / 0.6811 / 0.2311 / allow | z-undo / 0.6954 / 0.2882 / allow |

Top-1 vs expected: HGRA 5/5 before and after. Top agreement with weighted:
5/5 before and after. Gate agreement with weighted: 4/5 before (Send
refused), 5/5 after.

### Send score parts (`fixtures/send-buttons.manifold`, query "Send" button)

| region | hv before | sem before | total before | hv after | sem after | total after |
|---|---|---|---|---|---|---|
| z-send | 0.4170 | 1.0000 | 0.7209 | 0.4453 | 1.0000 | 0.7309 |
| a-feedback | 0.3379 | 1.0000 | 0.6933 | 0.3428 | 0.7500 | 0.6450 |
| b-device | 0.3223 | 1.0000 | 0.6878 | 0.3232 | 0.6667 | 0.6215 |

## Ablation (top minus runner-up margin)

Each fix alone, same corpus and dims:

| case | old sem + probe-mean | old sem + bundled | parity + probe-mean | parity + bundled |
|---|---|---|---|---|
| send | 0.0277 | 0.0359 | 0.0777 | **0.0859** |
| sidebar | 0.1324 | 0.1718 | 0.1324 | **0.1718** |
| export | 0.2232 | 0.2280 | 0.2500 | 0.2500 |
| admin | 0.2311 | 0.2348 | 0.2954 | 0.2916 |
| undo | 0.2311 | 0.2454 | 0.2844 | 0.2882 |

Reading: semantic parity is most of the Send fix (+0.050). The bundled query
adds +0.008 on Send and +0.039 on the 3-constraint sidebar query. On the twins
it is within +-0.004: those cases are decided by penalties and the text-miss
cap, not the vector. The first column reproduces the "before" totals exactly.

## Why

1. **Semantic precision.** The weighted matcher gained
   `recall * (0.5 + 0.5 * precision)` so an exact label beats a superset; HGRA
   kept recall alone, averaged with the role, so "Send", "Send feedback", and
   "Send to device" all scored semantic 1.0 and only the hypervector told
   them apart (0.028 apart, refused by the 0.05 margin). Sharing one function
   removes the drift. Side effect, deliberate: a wrong-role label hit now scores
   semantic 0 in HGRA too (min, not mean). The t7 test
   `hgra_label_hit_with_the_wrong_role_outranks_a_nameless_region` pins that it
   still outranks a nameless region.
2. **Bundled query.** Averaging per-probe cosines treats every constraint as a
   separate weak measurement against a signature that bundles about ten parts,
   so each cosine is small and the mean stays near the single-probe level.
   Bundling the probes first is the standard HDC record query: one vector holding
   every constraint, one similarity. For near-orthogonal probes the bundle's
   cosine grows roughly with sqrt(number of constraints) when a region matches
   them all, so multi-constraint queries separate more (sidebar +0.039).
   With two probes (text + role) half of the components tie and break to `+1`,
   so the gain there is small; that is the documented `bundle` tie-break, left
   unchanged. Equal weight per probe keeps the exact probe set the mean used, so
   the ablation isolates "mean of cosines" vs "cosine of the bundle".

## Not changed (deferred)

- Ancestor / container context channel in the query (nested Suspend-row case).
  Now handled outside the hypervector: `LocateQuery::within` / `near` fold
  into the shared semantic minimum (`src/context.rs`), so HGRA obeys them too.
  Fixture: `fixtures/twin-suspend-rows.manifold`. The HGRA vector itself still
  does not encode context.

This corpus is label ranking on static pages. It is **not** the product bar:
it never opens a dialog and never puts one label under two parents. The bar
for variable change is `crates/ultra-instinct-guard/tests/world_context.rs` (see
"World context gate" in `docs/DECISIONS.md`).
- `temporal_stability`: `locate_with` reads the region value; extractors still
  set it to 1, so the 5% weight carries no signal. Out of scope here.
- The signature still bundles state, sources, and neighbors that no query
  probes. They dilute every region equally on these fixtures. Not tuned.
- No live-suite rerun with `matcher: "hgra"` (A6/A8, 23 tasks). This is the
  5-case fixture corpus only. HGRA is still not the product default.

## Reproduce

```bash
cargo test -p ultra-instinct-resonance --features hgra --test hgra_remeasure -- --nocapture
cargo test -p ultra-instinct-resonance --features hgra --lib bundled_query
```

## World-context channel (feat/world-context-gate)

`query_vector` now takes the resolved [`ContextScope`]. When `within` or a
resolved `near` scope names a container, the query bundles the same permuted
`parent` bindings the region signature already stores for that container's
role and label tokens. Cosine therefore prefers in-scope twins; the semantic
minimum still zeros out-of-scope regions (parity with WeightedMatcher).

Evidence: `hgra_query_vector_encodes_within_as_parent_channel` and
`hgra_near_encodes_resolved_scope_into_the_query_vector` on
`fixtures/twin-suspend-rows.manifold`. The 5-case `hgra_remeasure` corpus is
unchanged (no ancestry on those pages).

## Rank traces

```sh
RUST_LOG=ultra_instinct_resonance=debug cargo test -p ultra-instinct-resonance --features hgra -- --nocapture
```

Target `ultra_instinct_resonance::rank` logs per-candidate score parts and the post-sort top/margin.
