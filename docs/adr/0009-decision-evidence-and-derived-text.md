# ADR 0009: Decision evidence, trusted clicks, and derived text

- Status: accepted
- Date: 2026-10-17 (Asia/Manila)
- Relates: ADR 0001, ADR 0006, ADR 0008; AGENTS.md anti-drift rules 2, 4, 7a, 8

## Context

The MiniWoB++ race (run `20261017-142404`) put `aui run --policy jev` (A10m)
beside jev-ultrafast (A3m) with the same model. A10m passed 16% against
A3m's 28%. Reading every A10m failure gave four general mechanisms, none
specific to MiniWoB:

1. JEV saw only the action menu. Values that exist only on the page (a sum
   to compute, a word to find) could not be chosen from.
2. The agent clicked with `HTMLElement.click()` first. That does not move
   focus and does not fire `pointerdown` / `mousedown`, so focus handlers and
   many widgets never saw the click.
3. A click that worked but changed nothing in the region fingerprint (a
   checkbox toggle, a button that only sets a flag) verified `no-effect`, and
   the model read that as failure.
4. Unnamed targets (`label: ""`) could not be told apart even with a
   screenshot, and typed values had to occur in the goal, so "type the
   answer" tasks always abstained.

## Decision

1. **Decision evidence.** `BrowserPolicy::wants_evidence` /
   `decide_with_evidence` carry a `DecisionEvidence` { screenshot, page }.
   The agent captures only what the policy asks for, after observe and before
   the decision, read only. Instinct asks for nothing. The remote request
   gains `page` {url, title, text ≤ 6000 chars}; the JEV transport sends the
   viewport JPEG as a System One image with the viewport's CSS size. Every
   target in the request carries `box` [x, y, w, h] in viewport CSS pixels and,
   when unnamed, `near_text` (parent text, else text to the left on the row
   within 160 px, else text directly above within 40 px). The region label is
   unchanged, so tickets and Instinct scoring do not move.
2. **Trusted clicks.** The agent's CLICK is a trusted pointer move / press /
   release at the region center. The DOM tiers run when CDP refuses the
   pointer press, and as the retry after a pointer click with no visible
   change. The MCP host's `browser_click` keeps its DOM-side click.
3. **Verification.** A click with no fingerprint delta but a changed control
   state on a region present in both observations is `state-changed`. A
   dispatched input with no observed change is `dispatched-no-visible-change`
   (was `no-effect`); it still counts toward the consecutive no-progress
   bound and habituation, so loops stay bounded.
4. **Derived text** (opt-in `--allow-derived-text`, feature `model-text`).
   The text model receives page text. A shape-checked reply that is not in
   the goal may be typed only when the goal quotes no untyped value (a quoted
   goal value wins; a loose phrase such as "type your answer into …" names the
   field's purpose, not a value), the field is not secret (input type
   password / hidden / file; label words password, passcode, pin, cvv, cvc,
   otp, ssn, card, security, one-time), and the value is one line of at most
   64 characters. Each derived value is printed as
   `text derived from page: "…"`.

## Consequences

- Same ticket / gate / revalidate chain. Replies are still an offered id or
  abstain; evidence cannot name a target or carry a coordinate back.
- Page text and screenshots are untrusted data; the JEV and text-model rules
  say so. A wrong read can now type a wrong derived value; it is opt-in,
  guarded, and printed.
- Owners: `aui-core` action-space tests (box, near text, gap boundaries),
  `aui-policy` remote and model-text tests (evidence reaches transports,
  derived guards), `aui-agent` `verify_map` and `mock_loop` tests (control
  state, evidence capture, page-text cap), `aui-cli` typesafe / run tests.
