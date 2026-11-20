# Anti-drift 5: Stale ≠ spent

Rule 5 of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**Stale ≠ spent.** `WorldChanged` / `TargetChanged` / `TargetGone` are
stale → discard the prediction, re-observe, decide again (bounded by
`max_consecutive_stale`). `TicketConsumed` / `TicketMismatch` / gate
refusal / page rejection / dispatch failure are **not** stale and are
never silently retried with the same lease. `DomainNotAllowed` and
`NeedsConfirm` are not stale: the agent ends BLOCKED
(`Agent::policy_refusal`), `run` prints `rerun with --confirm "<label>"`,
never retried with the same lease.
