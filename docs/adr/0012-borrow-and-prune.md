# ADR 0012: Borrow from agent-browser, prune what the agent loop outgrew

- Status: accepted
- Date: 2026-11-08 (Asia/Manila)
- Relates: ADR 0001, ADR 0003; AGENTS.md anti-drift rules 3 and 9
- Supersedes: the ranked `guard()` (`0.55` / `0.05` float gate) and the HGRA
  freeze ("HGRA stays frozen") recorded in AGENTS.md and `docs/DECISIONS.md`

## Context

The owner asked to clean up Ultra-Instinct and to take what is worth taking
from [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser),
expecting that some of our own features would have to go. A side-by-side
comparison of the two tools produced one card per idea, and the owner marked
each card:

- **take**: a compact one-line-per-control page snapshot, a session daemon
  that one shell agent drives with short `aui` verbs, a guard policy (allowed
  domains, presses that need a confirm), `wait for` a text / URL / control,
  and a `skills` command that prints the guide shipped in the binary;
- **later**: launching Chrome from `aui`, saved login state;
- **skip**: other browser engines, live streaming;
- **drop**: the deprecated helpers (`locate`, `inspect`, `act`, `diff`), the
  ranked guard, the compare command, and the stale result tables;
- **keep**: the browser-use alias tools.

Since ADR 0001 the agent owns the loop and Instinct chooses among a finite
`ActionSpace`; nothing on that path ranks labels. What still ranked was the
host preflight: MCP / CLI `locate` and `guard` called
`ultra-instinct-resonance`'s `WeightedMatcher`, `guard` added the `0.55` /
`0.05` float gate and a buried-label rule on top, and the frozen HGRA matcher
(`ultra-instinct-hyper`) was still built, tested and published so the
resonance `hgra` feature resolved. Those existed for the historical A5 / A6
bench arms. They were a second, scored way to pick a target beside the hard
gate.

## Decision

### Borrow

Everything borrowed wraps the agent / gate / ticket path; none of it adds a
second way to press.

1. **Compact snapshot.** `browser_get_state` defaults to text: a title / url
   line, then `@eN role "label"  state` per control (value, checked,
   expanded, selected; never a password value); `format:"json"` keeps
   `elements[]`. Every index argument takes `5`, `"5"` or `"@e5"`.
2. **Session daemon and shell verbs.** `aui daemon --session <name>` serves
   `$TMPDIR/aui/<session>.sock` (0700 dir, 0600 socket) with one MCP
   `Server` in one thread: one session, one CDP socket. `aui open`, `snap`,
   `click`, `type`, … spawn it on first use; every press is the ticketed
   `browser_*` tool. `aui batch` runs several verbs in order on one session;
   `aui session [list]` names the current / live sessions.
3. **Guard policy.** `Policy` in guard `policy.rs`: allowed domains
   (`HostPattern`) and confirm-before-press labels (`LabelGlob`), checked
   after `revalidate` and the hard gate, before consume. `run
   --allow-domain / --confirm-press / --confirm`, the MCP env vars and the
   `confirm` argument set it. A refusal is BLOCKED, not stale.
4. **Wait-for.** `browser_wait` / `aui wait` with `text`, `url`, `control`,
   `gone` (no control has that exact label) or `load` (`document.readyState`;
   never network idle), polling a fresh observe about every 200 ms.
5. **Skills in the binary.** `aui skills list | get core` prints the guide
   (`crates/ultra-instinct/skills/core.md`).
6. **Ported tools** (`browser_extra.rs`): `browser_dialog`,
   `browser_go_forward`, `browser_reload`, `browser_check`, `browser_focus`,
   `browser_keyboard`, `browser_get` / `browser_is` / `browser_count`,
   `browser_screenshot` `annotate`, and `max_chars` on the text tools
   (`aui --max-output N`).
7. **Launch, doctor, install.** `run --launch [--headed]` and `aui open
   --launch` start a local Chrome (`ultra-instinct-browser` `launch.rs`;
   the daemon owns it for the session's life); `aui doctor` checks Chrome,
   the endpoint, a test launch and the socket dir; `aui install` prints (or
   with `--run` runs) the Chrome for Testing install command.
8. **Saved state.** `browser_state_save` / `_load`, `browser_cookies`,
   `browser_storage`, `run --load-state / --save-state`: values masked
   unless `reveal`, 0600 files, load refuses group / other permissions,
   storage only for the current origin (others pending).
9. **Environment.** `browser_set` (viewport, device, media, offline,
   headers, reset), `browser_download` (a ticketed press, then the file
   moves to `path`), `browser_network` (block / unblock through
   `Network.setBlockedURLs`, HAR without bodies, secret headers masked).

Not borrowed: other browser engines, live streaming, the dashboard, mouse
coordinates, CSS / XPath selectors, the auth vault and plugins, and the
react / vitals / a11y / trace / record commands.

The owner's decisions on the open questions: refs are `@eN`; the text
snapshot is the default; confirm-before-press only for labels the user
lists (no built-in risky-word list); confirmation is the `--confirm` flag,
not a tty prompt; and `guard` matches the exact label (see Prune 1).

### Prune

1. **One hard gate, no score.** `guard()` finds its target by exact label: a
   region matches when its label equals the query text after trimming,
   collapsing whitespace and ignoring case; a query role must be equal;
   `within` keeps only descendants of that container. No match refuses
   `missing-target`. Without a proposed id, more than one visible match
   escalates `ambiguous` (hidden matches are not counted; if all are hidden
   the first is the target and the hidden check refuses it). A proposed id
   must be one of the matches, else `front-layer` when it is buried and
   `proposed-not-top` otherwise. The target then goes through the existing
   checks in order (disabled, hidden, occluded, front layer, offscreen, then
   `gate::check`). Allow carries confidence 1.0, no margin, and an
   `of_target` ticket; candidates are up to five matches at 1.0. The world
   change check still comes first. `near` and `position` are a
   `GuardError` that points at `proposed` and `within`; MCP refuses `near`,
   `position`, `matcher` and `dims` by name. `GuardReason::LowConfidence`
   stays in the wire enum, unused, because the protocol crate is shared with
   the borrow change.
2. **Region state lives in geometry.** `RegionState`, `Availability` and
   `Visibility` move from resonance to `ultra-instinct-geometry` (not core:
   they use geometry's viewport test, and geometry depends on core).
3. **Resonance and HGRA go.** `ultra-instinct-resonance`
   (`WeightedMatcher`, `HgraMatcher`, `ContextScope`), `experiments/hgra`
   (`ultra-instinct-hyper`), the `hgra` features, and the `evals/locate`
   corpus are deleted. The product graph is 13 crates.
4. **No deprecated helpers.** MCP drops `locate`, `inspect`, `act` and
   `diff` and the `repeated_query` signal that only served locate;
   `tool_for_phase` maps Locate and Inspect to `observe` and Diff to
   `verify`. The CLI drops the same subcommands, the matcher options,
   `compare.rs`, and the `sidebar_locate`, `fixture_compare` and
   `live_drive` examples. `observe` already returns every region's id,
   label, parent and state, and `verify` compares two snapshots.
5. **Results state the current product.** The pre-ticket A1–A8 tables move
   verbatim to `bench/results/HISTORICAL-A1-A8.md`; `RESULTS.md` opens with
   the 2026-10-17 Claude Code agents table.

## Consequences

- The A5 / A6 bench arms (and the A7 / A8 combo arms and the B0/B1 stub,
  which also call `locate` / `inspect` / `act`) can no longer be re-run on
  this build. Their scripts and result files stay as history.
- `ultra-instinct-hyper` and `ultra-instinct-resonance` are no longer
  published. Versions already on crates.io stay there.
- A host that asked `guard` for a near-miss label ("Sign" for "Sign in")
  used to get a ranked Allow or a low-confidence refusal; it now gets
  `missing-target` and must use the label `observe` reports.
- Two exact twins (for example a page "Cancel" and a dialog "Cancel")
  escalate `ambiguous` where the old front-layer ranking picked the dialog's;
  the host passes `proposed` or `within`.
- The `ultra-instinct-browser` and `ultra-instinct-mcp` line ceilings were
  raised for the ported tools, and `ultra-instinct` for the shell verbs
  (reasons in `scripts/crate-ceilings.txt`).
- `Network.setBlockedURLs` takes `urls`; very new Chrome may want
  `urlPatterns` instead.
- `browser_download` and `browser_set` are tested over CDP replay only, not
  against live Chrome in CI. One manual smoke of the shell verbs against a
  launched Chrome 156 (2026-11-08) ran `download`, `set viewport / media /
  reset`, `network block / har` and the rest; it is not a recorded test.
- Owners:
  - `ultra-instinct-guard` unit tests (clear button, different label, twins,
    hidden twin, `within`, case and whitespace, disabled, buried page button
    under a dialog, proposed non-match, `near` error) and
    `tests/world_context.rs`.
  - `ultra-instinct-geometry` state tests.
  - `ultra-instinct-mcp` `tests/server.rs`, `tests/world_context.rs`,
    `tests/acme_mail.rs` (t7 / t8 re-homed under guard), `tests/chaos.rs`.
  - `ultra-instinct` `tests/cli_execute.rs`, `tests/mcp_stdio*.rs`.
  - Borrow: guard `tests/policy_props.rs`, the executor and `mock_loop`
    policy tests, the `daemon.rs` socket tests and `verbs.rs` / `batch.rs`
    unit tests, `crates/ultra-instinct-mcp/tests/browser_tools.rs`,
    `browser_extra.rs`, `browser_state.rs`, `browser_env.rs`, and the
    state-file proptests in `crates/ultra-instinct-browser/tests/storage_state.rs`.
  - `scripts/architecture.txt` and `scripts/check_architecture.py` for the
    dependency graph.
