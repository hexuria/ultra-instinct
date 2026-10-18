# References

## browser-use/jev-ultrafast (MIT)

Architectural and behavioral reference for the finite action-space agent loop.
Not a mechanical translation target. License: MIT (preserve attribution when
substantially derived fixtures or flows are copied).

Upstream: <https://github.com/browser-use/jev-ultrafast>

| jev-ultrafast | ultra-instinct |
|---|---|
| `agent.py` | `ultra-instinct-agent` (`Agent`: predict / act / tick / run) |
| `browser.py` | `ultra-instinct-browser` |
| `snapshot.js` | browser observation / ActionSpace (existing DOM+AX; not a JS port) |
| `model.py` | `ultra-instinct-policy` + `ActionSpace` |
| `questions.py` | consumer policy / eval data |
| `fresh()` | fresh observe + `revalidate` + hard `gate` at the executor (ADR 0003) |
| `Browser.act()` | `ultra_instinct_agent::execute_ticketed` → `BrowserSession::{press, type_text, select_option, scroll}` |
| history | agent history / journal |

Where Ultra-Instinct intentionally goes further: DOM+AX fusion, stable cross-observation
identity, modal/front-layer and occlusion reasoning, Instinct local decision tier,
deterministic abstention, ActionTickets, postcondition verification
(no-effect / wrong-effect), replay/evals.

## hexuria/instinct

Domain-agnostic decision engine. ADR 0010: Instinct owns HOW; Ultra-Instinct owns WHAT.
Pinned by git rev `a42d16b` in the workspace; `InstinctPolicy` is the offline policy (`--policy instinct`); `jev` builds default to `RemotePolicy`. Repo: <https://github.com/hexuria/instinct>
