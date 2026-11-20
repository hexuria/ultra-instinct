# Anti-drift 4: One executor boundary, one staleness barrier

Rule 4 of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**One executor boundary, one staleness barrier.** `execute_ticketed` is
the only path from a ticket to page input. Order: ledger (one-shot) →
input kind == `ticket.action` → **fresh observe** → `revalidate`
(target-scoped world fingerprint + target role / label / fingerprint) →
hard gate on the fresh region → **mark consumed before dispatch** →
dispatch to `ticket.target_id`. The agent's CLICK is a trusted pointer
press at the region center (`Input::Click`); the DOM `this.click()` tiers
are its CDP-refusal fallback and the retry (`Input::ScriptClick`) after a
pointer click shows no visible change. The MCP host's
`browser_click` (and `browser_check` / `browser_download`) presses the
same trusted pointer by default; `mouse: false` sends the DOM script
click. The host
helper `consume_ticket_once` uses
the same consume-before-press order. `Predicted` carries no fingerprint of
its own; do not add a "pre-ticket check" that is not wired into a refusal.
The user's `Policy` (guard `policy.rs`: allowed domains, confirm-before-press)
is checked on the page the input starts on, and on where a press that
submits a form sends it (`ElementState::form` / `submit_action`; other
navigation is checked at the next input there), by `execute_ticketed_with`
on the fresh observe's URL and region
after `revalidate` and the hard gate, before mark-consumed;
`execute_ticketed` runs it with an empty policy. The agent also checks it
after the gate; `consume_ticket_once_checked` runs it after `revalidate`,
before consume. An empty policy allows everything. With an allow-list,
only `http` / `https` on a listed host passes (plus `about:blank`, and
a `blob:` by its origin, none with `Policy::refuse_blob`); `data:`,
`javascript:`, `file:` and other
schemes refuse, and a host is read only after a real `scheme://`
(`Policy::check_url`, `host_of`). A confirm applies to
a press (`Policy::press_label`): the CLICK family on the target, an
activating KEY (Enter / Space / NumpadEnter, any modifiers) on a
button-like target (Enter on a checkbox / radio submits its form
instead), and Enter in a form control, which presses its
form's implicit-submission button by its accessible name (observe's
`ElementState::form` evidence; a button-less form Enter still submits,
or unread evidence → `FORM_SUBMIT_UNKNOWN` refuses while any pattern is
set; an activating key to an unnamed focus in the keyboard tools →
`UNOBSERVED_FOCUS`; a nameless submit button is `FORM_SUBMIT_UNKNOWN`).
The executor's refusal (`ExecError::Policy { reason, label }`) names the
label its fresh look checked. Space in a field and other keys press
nothing. One rule for the agent, the executor's fresh re-check and the
host tools.
