# Anti-drift 8: Remote escalation is explicit and closed

Rule 8 of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**Remote escalation is explicit and closed.** Feature `remote`;
reply is exactly a choice id + kind from the offered menu or abstain.
Selectors, coordinates, scripts, extra fields → hard error. The request
may carry evidence a transport asks for (`wants_evidence`):
the viewport JPEG beside it (`jev`: a System One image, `--no-screenshot`),
`page` {url, title, ≤ 6000 chars of visible text} (`jev`, `clef`,
`--no-page-text`), per-target `box` / `near_text`, per-action `tried`
{times, last} when the action was already dispatched with no effect
(evidence only — the reply may still pick it), and the run's
`progress` (clauses, which are satisfied, values typed; secrets masked).
Evidence never widens the reply. New kinds are still offered
ids: `HOVER:<id>`, `SCROLL_IN:<id>`, `DRAG:<src>><dst>` (up to 8
sources × 96 destinations, sent as one compact `drag` block) including
`DRAG:<src>><surface>@c,r` onto a cell of the drawing a source sits on,
surface cells `CLICK:<surface>@c,r` (8×8), `KEY:<id>#<key>` from the
closed `KEY_NAMES` set, and `SELECT_TEXT:<id>` (a trusted triple-click,
verified by the page's selection). JEV's DONE carries a
same-call `goal_satisfied` self-check; DONE below 0.7 (0.5 once every
clause is satisfied) is re-asked without DONE, and BLOCKED below 0.6 is
re-asked without BLOCKED: at most three calls, a second
refused DONE abstains. With `--second-opinion` the text model is asked
when JEV ends on BLOCKED, abstains, or gives its operation < 0.5; its
reply must be exactly an offered id + kind, else JEV's answer stands.
Targets come before page controls in the request;
near text and row text that echo the goal are not sent.
`--decision model` (feature `model-text`, `decide.rs`) sends the same
request to the `TEXT_MODEL_*` model with the viewport JPEG and page
text; a reply that is not an offered id + kind is asked once more with
the reason, and the second reply goes to `parse_reply` unchanged (both
share `reply_shape`). Its DONE is checked by a second question (goal
met?); a refused DONE is asked again without it, a second one abstains.
