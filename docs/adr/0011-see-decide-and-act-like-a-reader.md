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

## Consequences

- Observe costs one more CDP call: six on the compact path. The call budget
  test pins it.
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
  - `aui-remote` typesafe tests: DONE and BLOCKED second looks.
