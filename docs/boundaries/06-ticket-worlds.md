# Anti-drift 6: Ticket worlds are target-scoped

Rule 6 of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**Ticket worlds are target-scoped.** `WorldSnapshot::of_target`: front
layer and focus are global; clickable / occluded sets are the target
neighborhood (ancestors, same-parent siblings, children, root peers within
`NEIGHBOR_RADIUS_PX` = 160, inclusive). MCP `seen_world` stays whole-page
(`WorldSnapshot::of`). A DRAG ticket binds two ends: the source world as
above and a `TicketDest` with the destination's own neighborhood (and,
for a drop on a surface point, its grid `cell`); a surface-point ticket
binds its grid `cell`; a KEY ticket binds its one key. The executor
refuses a swapped destination, another cell, another key, or a two-end /
cell / key ticket on any other input (`TicketMismatch`), and re-checks
the drop target.
