# World-context smoke (live Chrome)

Deterministic observe → guard checks against a throwaway Chrome. No JEV, no Luna.

## Pages

| Page | Proves |
|---|---|
| `site/modal-confirm.html` | Open modal: buried "Delete project" refuses; dialog "Cancel" allows |
| `site/twin-suspend.html` | Twin "Suspend" escalates without context; `near(focus)` allows the focused row |
| `site/cookie-backdrop.html` | Non-dialog overlay: hit-test marks "Save" occluded and guard refuses |

## Run

```sh
./examples/world-context/smoke.sh
```

Requires Google Chrome at the usual macOS path (override with `ULTRA_INSTINCT_CHROME`).
Uses ports `8766` (HTTP) and `9334` (CDP) by default.

Forced HGRA remasure (experimental matcher):

```sh
ULTRA_INSTINCT_MATCHER=hgra ./examples/world-context/smoke.sh
```

## Rank traces

```sh
RUST_LOG=ultra_instinct_resonance=debug ULTRA_INSTINCT_MATCHER=hgra ./examples/world-context/smoke.sh
```

Emits one `candidate` debug line per region (semantic, hypervector, total, …) and a `rank_top` summary. Off unless `RUST_LOG` enables `ultra_instinct_resonance`.
