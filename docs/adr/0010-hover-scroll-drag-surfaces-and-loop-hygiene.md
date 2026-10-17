# ADR 0010: Hover, container scroll, drag, surface points, and loop hygiene

- Status: accepted
- Date: 2026-10-17 (Asia/Manila)
- Relates: ADR 0009; AGENTS.md anti-drift rules 4, 6, 7b, 8

## Context

On the PR #78 rerun, A10md failed 264 of 381 MiniWoB++ attempts. They fall
into two groups:

- **Agent mistakes, about 125.** Re-typing a filled field until the step
  cap; DONE with the goal unmet; clicks whose popup rendered after the
  settle and so read as "no change"; JEV seeing only the current clause.
- **Missing capabilities, about 105.** No hover, no scrolling inside a box,
  no drag, SVG shapes and text-less clickable boxes never observed, no way
  to click a point on a canvas or SVG. List items (`LI`) were never kept.

Every fix below is justified by general web behaviour. No benchmark name,
task name, or page class appears in `crates/`; a repo rule enforces that.

## Decision

1. **Loop hygiene.**
   - A TYPE_TEXT whose field still holds the value this run typed there is
     not offered again.
   - Remote requests carry `progress`: the clauses, which are satisfied, and
     the values typed (secret fields masked).
   - JEV asks a `goal_satisfied` noul in the same call. DONE with P(yes)
     below 0.5 is re-asked once without DONE; if that fails too, abstain.
   - A click with no visible change gets one more look after 150 ms.
2. **Observation.**
   - SVG shapes become Image + CLICK regions, named by `<title>`,
     `aria-label`, their own text, or fill plus shape.
   - `LI` elements with their own text are kept, like other text elements.
   - A text-less box is kept only when a click hint confirms it, and is named
     by its inline background colour.
   - `canvas` and `svg` roots claim `point`.
   - Scrollable boxes claim `scroll`, popup owners claim `hover`, and
     drag-marked elements claim `drag`. Drag markers are `draggable`,
     `aria-grabbed`, sliders, a move or grab cursor, or common widget-library
     handle classes.
   - All of this evidence is read in the compact walk, so observe makes no
     new CDP calls.
3. **New offered kinds.** Every one is still a menu id; no reply carries a
   coordinate.
   - `HOVER:<id>` and `SCROLL_IN:<id>` go through gate, ticket and executor
     like a click.
   - `DRAG:<src>><dst>`, capped at 8 sources × 16 nearest destinations, gets
     one ticket with a boxed `TicketDest`. Both neighbourhoods are
     revalidated, and the destination is re-checked as a drop target.
   - Surface cells are `CLICK:<surface>@c,r` on an 8×8 grid. `gate_point`
     binds the cell into the ticket, and the executor clicks exactly that
     cell from the fresh surface box.
4. **Crate split.** The remote transports move into `aui-remote`, the only
   crate with a model SDK or HTTP client. `aui-cli` re-exports them.

## Consequences

- Menus grow: up to 128 surface cells and 128 drag pairs, plus hover and
  scroll entries. The gate, ticket and verification still apply to every
  one.
- Handlers bound with `addEventListener` that show no cursor and no markup
  stay unseen. That is deliberate: guessing would be tuning to a benchmark.
- Drawing (dragging between two surface points) is not offered.
- Owners:
  - `aui-core` action-space tests: hover, scroll-in, drag caps, grid.
  - `aui-guard` `ticket_props`: drag binds both ends.
  - `aui-agent` `props`: destination and cell substitution, drag ticket on a
    click.
  - `aui-agent` `mock_loop`: retype guard, late popup, hover / scroll / drag
    end to end.
  - `aui-browser` extract / compact / observe tests.
  - `aui-remote` typesafe tests: DONE check, drag compose, progress.
