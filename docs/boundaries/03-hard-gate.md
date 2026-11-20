# Anti-drift 3: Hard invalidity is guard evidence, not a score

Rule 3 of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**Hard invalidity is guard evidence, not a score.** Disabled, readonly
(TYPE / SELECT), hidden / zero-area, occluded, front-layer, offscreen,
missing target, unsupported action → `ultra_instinct_guard::gate` refuses.
MCP / CLI `guard` finds its target by exact label (trimmed, whitespace
collapsed, case ignored; plus role and `within`), unique or refuse
(no match → `missing-target`, several visible → `ambiguous`, a
`proposed` id must be a match), then runs the same hard gate and issues
an `of_target` ticket. There is no score and no float gate.
