# World-context smoke (live Chrome)

Deterministic observe → guard checks against a throwaway Chrome. No JEV, no Luna.

## Pages

| Page | Proves |
|---|---|
| `site/modal-confirm.html` | Open modal: buried "Delete project" refuses; twin "Cancel" escalates, the proposed dialog "Cancel" allows |
| `site/twin-suspend.html` | Twin "Suspend" escalates without context; `within` the focused field's row allows that row |
| `site/cookie-backdrop.html` | Non-dialog overlay: hit-test marks "Save" occluded and guard refuses |

## Run

```sh
./examples/world-context/smoke.sh
```

Requires Google Chrome at the usual macOS path (override with `ULTRA_INSTINCT_CHROME`).
Uses ports `8766` (HTTP) and `9334` (CDP) by default.
