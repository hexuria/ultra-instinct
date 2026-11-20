# Verification owners

Which test, proof or check owns which part. Indexed from [AGENTS.md](../AGENTS.md).

- Agent loop, stale discards, multi-step clauses: `ultra-instinct-agent` unit tests,
  `tests/mock_loop.rs`, `tests/adversarial.rs`, `tests/replay_cdp.rs`.
- Executor boundary: `executor.rs` unit tests, `tests/props.rs`
  (substitution, staleness, consumed-beats-stale, `is_stale` classification,
  drag destination and surface cell substitution).
- Gate / ticket / world: `ultra-instinct-guard` unit tests,
  `tests/ticket_props.rs` (radius boundary 159 / 160 / 161, focus-only change,
  consumed never stale), `tests/world_context.rs`.
- User policy (allowed domains, confirm-before-press): guard
  `tests/policy_props.rs`; the `executor.rs` policy tests; the
  `tests/mock_loop.rs` policy tests (`needs_confirm_ends_blocked_without_pressing`,
  `domain_off_the_allowlist_ends_blocked`,
  `enter_in_a_field_whose_form_submits_with_a_confirm_label_blocks_unpressed`,
  `a_blob_page_mid_run_ends_blocked_with_refuse_blob`);
  the guard's `activating_keys_are_presses_of_the_target_or_its_form`; the
  `refuse_blob` switch: guard `domains_exact_and_subdomain_patterns`, mcp
  `browser_tools.rs` `navigate_to_a_blob_is_refused_with_refuse_blob` and
  `host_options.rs` `the_env_policy_parses_refuse_blob_and_fails_closed`.
- Shell verbs and the session daemon: `verbs.rs` / `batch.rs` unit tests and
  the `daemon.rs` socket tests (round trip, spawn / stale socket, idle exit,
  `--idle`, failed launch, session list; a press after the page changed
  since `snap` refuses unpressed, a press before any snap refuses;
  `bind_to_latest_look` in `snapshot_refs.rs`). `browser_*` families over CDP
  replay: `crates/ultra-instinct-mcp/tests/browser_tools.rs`,
  `browser_extra.rs`, `browser_state.rs`, `browser_env.rs`, and the
  `ultra-instinct-browser-tools` unit tests. A call that
  opens a JS dialog returns `CdpError::DialogOpened` at once: ws.rs
  `a_dialog_opening_ends_the_wait_and_stays_buffered`.
- Instinct evidence: `ultra-instinct-policy` unit tests. Do not add a second model of
  Instinct scoring.
- MCP / CLI `guard` (exact label, unique or refuse): `ultra-instinct-guard`
  unit tests and `tests/world_context.rs`; over the wire,
  `ultra-instinct-mcp` `tests/server.rs`, `tests/world_context.rs`,
  `tests/acme_mail.rs`.
- `write_fixture` / `parse_fixture` own the manifold fixture grammar. CDP
  replay is a different grammar. Fusion is the only DOM/accessibility merge.
  Region identity across observations is owned by the browser `IdentityMap`.
- Untrusted input (fixture grammar, CDP replay scripts, CDP pipe frames, MCP stdin lines,
  remote-model replies, model-text replies): `fuzz/` cargo-fuzz targets on a
  dated nightly (`scripts/fuzz-smoke.sh`, `.github/workflows/nightly.yml`),
  backed by the structured/garbage proptests at small case counts. Saved
  state files: the parser proptests in
  `crates/ultra-instinct-browser/tests/storage_state.rs`.
- CDP pipe transport (framing, routing, dialogs, target gone, fail-closed
  limits): `crates/ultra-instinct-cdp/tests/pipe.rs` against a fake Chrome on
  `std::io::pipe()`, with the frame-splitter chunking proptests; the
  `pipe_frame` fuzz target; live Chrome in `tests/pipe_live.rs` (`#[ignore]`).
- Browser websocket link and `aui open --attach` (session routing, events,
  target gone, socket closed, write-while-reading, the allow-prompt hint, a
  refused port, the attached backend seeing and closing only its own tabs;
  over MCP `an_attached_server_sees_only_the_tabs_it_opened` in
  `pipe_mode.rs`):
  `crates/ultra-instinct-cdp/tests/ws_browser.rs` against a fake Chrome on a
  127.0.0.1 tungstenite listener; the port file: `ultra-instinct-chrome` unit
  tests; live Chrome: `live_attach_drives_a_tab_and_closes_only_its_own`
  (`#[ignore]`).
- Embedding over a pipe ([`docs/embedding.md`](embedding.md)):
  `crates/ultra-instinct-mcp/tests/pipe_mode.rs` (`Server::with_pipe` against
  a fake Chrome: tabs as `Target.*`, `target_gone`, `pipe_closed`),
  `snapshot_refs.rs` (a shifted page refuses `stale_ref` with no press;
  `require_snapshot`), `secrets.rs` (a typed password appears in no output,
  error, or HAR), `host_options.rs` (runtime policy, confirm not sticky,
  limits, absolute paths, downloads, allow-list), `uploads.rs`
  (`with_uploads`, canonical paths), `keyboard_policy.rs` (the keyboard
  tools under the policy), `run_goal.rs`; the code
  table: `crates/ultra-instinct-browser-tools/tests/error_codes.rs`; live
  Chrome: `crates/ultra-instinct-mcp/tests/pipe_live.rs` (`#[ignore]`).
- Observe protocol cost: `crates/ultra-instinct-browser/tests/call_budget.rs`
  pins the CDP call count per observe as a regression tripwire.
- Repo rules (toolchain pin, `forbid(unsafe_code)`, crate ceilings, pinned
  nightlies): `scripts/check_repo_rules.sh`. Dependency direction and the
  no-async/no-model-SDK ban below `ultra-instinct`:
  `scripts/architecture.txt` + `scripts/check_architecture.py`. Both run in
  `ci.yml`'s `repo-rules` job.
- Workflow integrity: `zizmor --persona=auditor` in `ci.yml`. Dependency
  stagnation: `.github/dependabot.yml`.
- Mutation testing: `.github/workflows/mutants.yml` (nightly + manual) on
  `gate.rs`, `ticket.rs`, `executor.rs`, and `world.rs` (`of_target` /
  `neighborhood_of` / `nearby`). Equivalent mutants are excluded in
  `.cargo/mutants.toml`. Run locally (`cargo install cargo-mutants --locked`
  once): `cargo mutants --no-shuffle -j 3 --timeout 180 -o /tmp/mut-a -f
  crates/ultra-instinct-guard/src/gate.rs -f crates/ultra-instinct-guard/src/ticket.rs
  -f crates/ultra-instinct-agent/src/executor.rs`, and `-f
  crates/ultra-instinct-guard/src/world.rs --re 'of_target|neighborhood_of|nearby'`
  for the world set. Exit 0 = all caught, 2 = missed (`mutants.out/missed.txt`).
- Label folding: `crates/ultra-instinct-browser/tests/label_folding.rs` and
  `crates/ultra-instinct-mcp/tests/label_folding.rs`.

Live Chrome tests stay `#[ignore]`; CI uses CDP replay fixtures, plus one
`live-smoke` job that runs `live_borrow.rs` against the runner's headless
Chrome (all but the `--headed` test). The live
tests are `crates/ultra-instinct-agent/tests/live_smoke.rs` (the owned loop)
and `crates/ultra-instinct/tests/live_borrow.rs` (the shell verbs against
their own `aui open --launch` Chrome and a 127.0.0.1 test server: state,
cookies, set, download, network / HAR, dialog, close / idle lifecycle,
`--headed`, stdin batch; run with `-- --ignored --test-threads=1`).

```
Verification impact

[x] Pure Rust deterministic behavior
[ ] Concurrency / interleaving
[ ] System model
[ ] Crash-recovery / replay
[ ] Persistence
[ ] TLA+
[ ] Proof kernel
[ ] Workflow / DSL
[ ] Unsafe / memory
[x] Property-test / fuzz surface
[ ] No verification architecture impact

Reason: <which boundary — gate / ticket / executor / world / policy / text>
Affected invariants: <from the anti-drift block>
Tests or proofs updated: <owner tests>
```
