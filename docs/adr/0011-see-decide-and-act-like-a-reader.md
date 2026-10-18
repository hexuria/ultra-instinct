# ADR 0011: See what the browser knows, decide with evidence, act like a reader

- Status: accepted
- Date: 2026-10-18 (Asia/Manila)
- Relates: ADR 0006, ADR 0009, ADR 0010; AGENTS.md anti-drift rules 7, 7a, 7b, 8
- Supersedes in part: ADR 0010 "handlers bound with `addEventListener` that
  show no cursor and no markup stay unseen"

## Context

On the Merge-gateway race (jev-1.13, text only, seed 20261019) A10m passed
113 of 384 MiniWoB++ attempts. Replaying the failures with every decision
request logged showed what JEV was given. 173 of the 271 failures were JEV
answering BLOCKED, and in most of those it was right: the menu held no way
to do the task.

- Targets the page wires up by script (board cells, colour boxes, icon
  buttons) were missing. Chrome itself reports them clickable.
- Icon-only controls had no name, and repeated controls ("Reply" in every
  row) did not say which row they belong to.
- A display-only canvas (a click visualiser) added a target and 64 grid cells
  to every menu, and JEV often ranked it first.
- Unnamed fields carried the goal sentence itself as their nearby text.

As in ADR 0010, every change is justified by general web behaviour. No
benchmark name, task name, or page class appears in `crates/`.

## Decision

1. **See every button.**
   - Observe makes one `DOMSnapshot.captureSnapshot` call and reads Chrome's
     `isClickable`: nodes with click, mouse, or pointer listeners (added by
     script included) and links. A plain box with one (`span`, `div`, `li`,
     `td`, `tr`, `i`, ...) is kept and offered CLICK. This is the browser's
     own record, not a guess.
   - The compact walk collects evidence for every plain box, since whether a
     box has a listener is known only after the walk.
   - An icon-only control is named by its `title`, `alt`, or the words of
     its class or id (`fa fa-trash` → "trash", layout words dropped). Such a
     name carries its row's rendered text: "more · Conner @a 7h ago".
   - Label text skips subtrees that are not rendered (`display: none`).
   - A `canvas` or `svg` root claims `point` only with click evidence: a
     listener, an `onclick` / `tabindex`, or a pointer cursor it sets itself.
   - Targets that share a label get near text too; text several regions
     share is never near text.
   - Near text that repeats the goal (three or more words, four in five of
     them goal words) is not sent to a remote model.

2. **Decide with evidence.**
   - DONE needs JEV's same-call self-check at 0.7 or more, or 0.5 or more
     once the run's progress marks every clause satisfied. Below that it is
     re-asked without DONE.
   - BLOCKED needs 0.6 or more on the operation head. Below that it is
     re-asked without BLOCKED. A doubtful BLOCKED right after a refused DONE
     gets that second look too, so a decision costs at most three calls. A
     second refused DONE abstains; a BLOCKED that cannot be re-asked stands.
     Every answer is still an offered id: this is a fresh decision on a
     smaller menu, never "the runner-up wins".
   - The request lists page targets before page controls (scroll, WAIT,
     DONE, BLOCKED).
   - An unnamed generic wrapper around a clickable control is not offered
     CLICK: the control inside is.
   - Row text that echoes the goal is dropped from a label in the request.

3. **Work out the answer.**
   - Derived text (ADR 0009) is on by default in the CLI whenever a text
     model is configured; `--no-derived-text` turns it off. The library
     default stays off. Secret fields are never derived.
   - The text model gets the field's near text ("6 + 0 =") and the page's
     reading text: rendered text plus the contents of rendered text areas,
     which `innerText` leaves out.
   - Stricter: a goal-grounded value may not be a described phrase (two or
     more lower-case words led by a determiner, unquoted: "your answer").
   - Stricter: a quoted literal right after a press verb (`press "Submit"`)
     names a control, not a value, in the deterministic resolver and in the
     "quoted value wins" rule.
   - A derived value longer than 64 characters is typed only when it occurs
     word for word in the page text (a passage to copy).
   - A field may be typed again once another action has changed the page
     since it was typed (a submitted guess).
   - Near text also reads the text to the right on the same row.
   - A malformed JEV answer (probabilities that contradict the choice, a
     missing head) is asked once more; transport errors keep the SDK's own
     retries.

4. **New ways to act.** Every one is still an offered id; no reply carries
   a coordinate or a key name of its own.
   - Date, time, and month fields take the resolved text in their own value
     format (`03/07/2016` → `2016-03-07`, `8:16 PM` → `20:16`): their native
     setter drops anything else. Text that does not parse stays as it is.
   - DRAG destinations are every viable region (up to 96 per source, nearest
     first), sent once as a compact `drag` block (sources, destinations,
     pairs) instead of one entry per pair.
   - A source drawn on a surface can be dropped on any cell of it:
     `DRAG:<src>><surface>@c,r`. The surface must take points or be the
     drawing the source sits in. The ticket's `TicketDest` binds the cell; the
     executor releases at that cell's center from the fresh surface box.
   - SVG shapes claim DRAG when they, or their drawing, listen to the pointer.
   - A pair that names its source as its own destination is a broken answer:
     asked once more, then an abstain.
   - `KEY:<id>#<key>` presses one key from the closed `KEY_NAMES` set on a
     region that takes keys: sliders and slider handles take the arrows,
     Home / End, Page Up / Down; text fields take Enter (sent with its `\r`
     text so the form submits); comboboxes take Down / Up / Enter / Escape.
     `gate_key` binds the key; the executor presses exactly that key after
     focusing the target.
   - `SELECT_TEXT:<id>` selects a text block (three words or more of its own,
     not a control or field) with a trusted triple-click. A selection changes
     no element, so it verifies by `getSelection()`: success when it holds
     some of the target's text, wrong effect when it holds none.

5. **Describe pictures in words.**
   - A target named by its paint alone (a box with no text, class words, or
     a raw colour; a shape whose markup names no fill) takes its computed
     colour's name: the nearest basic keyword ("orange", "purple"), else a
     hue family with "light" / "dark" ("light green"). The compact walk reads
     the computed `background-color` and `fill`.
   - Polygons say how many corners they have ("green 3-corner polygon").
   - A target whose row text is the goal sentence is flagged
     `in_instruction` in the request: a sample swatch in the instruction
     line is part of the question, not an answer.
   - SELECT_TEXT counts a block's own words, not words from its name.

6. **A second opinion.** With `--second-opinion` (features `jev` +
   `model-text`), the configured text model is asked to choose when JEV's
   final answer is BLOCKED, an abstain, or an operation it gave less than
   0.5. It gets the same closed request plus `first_opinion` (JEV's reply and
   probability), at medium reasoning effort. Its reply must be exactly
   `{"choice": {"id", "kind"}}` naming an offered action with its own kind (a
   DRAG id must be an offered pair); anything else, an abstain or an error
   included, leaves JEV's answer standing. Every second opinion is printed.
   The gate, ticket, and executor are unchanged: the advice is one more
   offered id.

## Consequences

- Observe costs one more CDP call: six on the compact path. The call budget
  test pins it.
- `aui-browser` grows past its ceiling (7400 → 7800): listener, label,
  paint, and act evidence, most of it inline tests.
- Drawings rendered on a canvas stay unseen: no DOM, no shapes. A text-only
  model cannot read them; that needs an image.
- A canvas whose listener sits on a parent element loses its point grid.
- Owners:
  - `aui-browser` extract tests: listener boxes, markup names, row context,
    unrendered text, canvas evidence, snapshot parsing.
  - `aui-browser` observe tests: a listener box and canvas end to end;
    `tests/call_budget.rs`.
  - `aui-core` action-space tests: near text for shared labels.
  - `aui-policy` remote tests: near text and row text that echo the goal,
    target-first order.
  - `aui-core` action-space tests: unnamed wrappers.
  - `aui-remote` typesafe tests: DONE and BLOCKED second looks, malformed
    answers.
  - `aui-policy` text / model-text tests: press-verb literals, goal
    phrases, field context, copied passages.
  - `aui-agent` `mock_loop`: retyping after the page answered.
  - `aui-core` action-space tests: near text on the right, uncapped drag
    destinations, drawing cells, keys per role.
  - `aui-guard` `ticket_props`: drag to a cell, the drawing a shape sits in,
    key tickets.
  - `aui-agent` `props`: cell and key substitution; `mock_loop`: selection
    verification.
  - `aui-policy` text tests: date / time / month value formats.
  - `aui-remote` typesafe tests: the compact drag block, self pairs.
  - `aui-browser` colour tests and the painted-label observe test.
  - `aui-remote` typesafe tests: when a second opinion is asked, and that
    advice must name an offered action.
