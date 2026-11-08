# Browser evals

No browser harness runs in Phase 1. These categories are reserved so later
evals have a stable list. Each one should call `ultra-instinct guard` (or the
library) and assert the decision and target id, not a pixel.

- role-and-label match
- spatial constraints (left, right, top, bottom, center)
- duplicate labels
- penalty controls (disabled, hidden, occluded, offscreen, stale, ambiguous, detached, zero-size)
- ambiguity and inspect
- temporal identity across snapshots
- source disagreement

Do not score a run by guessed coordinates.
