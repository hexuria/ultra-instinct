# Examples

Preflight a click on a fixture: `guard` finds the region whose label equals
the target, unique or refuse, and never clicks.

```bash
cargo run -p ultra-instinct --bin ultra-instinct -- observe --fixture fixtures/sign-in.cdp.json
cargo run -p ultra-instinct --bin ultra-instinct -- guard \
  --fixture fixtures/sign-in.cdp.json --target "Sign in" --role button --json
```

## Benchmark scenarios

Six local scenario sites for the uniform agent benchmark in `bench/` (results in
each scenario's `RESULTS.md`). Every page
takes `?seed=N` (reorders rows and twins, never changes what a task means) and
reports ground truth to the bench server through `/_shared/bench.js`.

| Scenario | What it stresses |
|---|---|
| `acme-mail/` | Gmail-like inbox: twin Send buttons, hidden/offscreen Archive twins, disabled Save, star per row |
| `shop-checkout/` | product grid with near-twin names, cart steppers, address form, gated Place order |
| `admin-table/` | users table, per-row menus, confirm dialogs, role filters, paging, near-duplicate names |
| `booking-calendar/` | month grid with long day labels, time slots, confirm step, native select, toggles |
| `hard-dom/` | icon-only twins, open shadow roots, a same-origin iframe banner, a promo overlay |
| `travel-search/` | adapted from jev-ultrafast's fixture (MIT, Browser Use): search, select, checkbox, detail view |

Look at them by hand with `bench/.venv/bin/python bench/server.py --port 8765` and
open `http://127.0.0.1:8765/<scenario>/site/index.html`.

## World-context smoke

`world-context/` — live Chrome observe → guard checks (modal, twin Suspend,
cookie backdrop). No JEV. See [world-context/README.md](world-context/README.md).

```sh
./examples/world-context/smoke.sh
```
