# ADR 0014: Fold a label into the control it names

- Status: accepted
- Date: 2026-11-09 (Asia/Manila)
- Boundary: observe / world (fusion in `ultra-instinct-browser`)
- Relates: AGENTS.md anti-drift rules 2, 3, 6, 7b; ADR 0009 (state-changed
  clicks), ADR 0011 (Chrome `isClickable`)

## Context

`<label for="e">Email</label><input id="e">` observed as two regions: a
`generic` "Email" and the `text_field` "Email". Chrome's DOM snapshot marks a
label that has an enabled control `isClickable` (ADR 0011 offers such boxes
CLICK), so the label was a second CLICK target in the agent's `ActionSpace`,
a second ref in `browser_get_state`, and a second line in its page text. A
custom checkbox (native input zero-size or `opacity: 0` inside a styled
`<label>`) had the opposite problem: the input had no usable box and the
label lost its listener CLICK because it holds a click control, so nothing
was offered.

## Decision

Fusion folds labels before its two merge passes (`fusion::fold_labels`). It
is still the only DOM / accessibility merge; no other layer drops regions.

- **Links** (`extract::link_labels`): a `<label>` names its `for` target (an
  HTML labelable element with that id in the same tree scope: document,
  open shadow root, or iframe document; a repeated id names nothing) or else
  its first labelable descendant. A generic element names the controls whose
  `aria-labelledby` points at it, when it holds no other kept element and
  takes no click of its own. A text element that names more than one
  control is not linked.
- **Evidence** (`session::control_sight`, from the compact walk, no extra
  CDP call): the control is *visible* when it is rendered with a box, not
  `opacity: 0`, and the hit test at its center lands on it; *faint* when it
  is rendered but has no box, is transparent, or something else is under its
  center. No compact record, `display: none`, or a hit test that found
  nothing (an offscreen center) is no evidence, and nothing folds.
- **Forward fold**: the control is visible and offered (enabled in both
  trees, not hidden, a box with area, not fully offscreen). The label's DOM
  and accessibility nodes are dropped. The control keeps its id, box, name,
  and binding.
- **Reverse fold** (custom checkbox): a faint, enabled checkbox or radio
  named by a `<label>` element that holds no other click target. The label
  keeps its box, id, and binding and takes the control's role and claims
  (CLICK / TOGGLE); the control's nodes are dropped. A click on the label
  toggles the control (pointer or `label.click()`). The compact walk records
  a label's `checked` as its checkbox's, so the click verifies as
  state-changed (ADR 0009). Only one label per control folds this way.
- **Left alone**: a label for nothing, for a disabled or hidden control, for
  a control not observed, a faint text field (typing into its label types
  nowhere), and every label without evidence (the per-node replay path).
- A label's inner targets (a link in its text) are their own regions and
  are never dropped. The only visible clickable box is never dropped.

Region fingerprint rules are unchanged. Identity stays the `IdentityMap`'s:
a folded field keeps `n{control}`, a custom checkbox is `n{label}`, both
stable while the page's evidence holds.

## Consequences

- One region per labelled field in the manifold, the `ActionSpace`, and
  `browser_get_state`. A target-scoped ticket world (rule 6) no longer counts
  the label as a clickable neighbour; the gate (rule 3) and the executor
  (rule 4) are unchanged.
- A goal that names a label ("check Remember me") resolves to the control;
  Instinct still owns the choice (rule 2): folding removes a duplicate
  candidate, it adds no score and no gate.
- A label left alone for a `display: none` checkbox carries that checkbox's
  checked state as evidence.

## Owner tests

- `ultra-instinct-browser`: `fusion` unit tests (forward fold, left alone for
  off / unobserved / unseen / faint text field, reverse fold, a second
  label, a label holding a link), `extract::labels_link_to_the_one_control_they_name`
  (`for`, wrapping, `aria-labelledby`, inner link, disabled, orphan,
  repeated id, clickable text, hidden input, shadow scope),
  `tests/label_folding.rs` (each form through a compact replay, the custom
  checkbox's box, role, state and stable ids, transparent and covered
  checkboxes, a covered text field).
- `ultra-instinct-mcp`: `tests/label_folding.rs` (`browser_get_state` lists
  each field once; a custom checkbox is clicked on its label's node).
- `ultra-instinct-agent`: `tests/replay_cdp.rs::check_goals_click_the_control_a_label_names`.
- Fixture: `ultra_instinct_cdp::script::PageSpec::labelled_form`.
