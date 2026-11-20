# Anti-drift 7: Text is not Instinct

Rule 7 of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**Text is not Instinct.** `TYPE_TEXT` / `SELECT` payloads come from a
`TextResolver`, checked against their context fingerprint, before the
ticket is consumed, so executor revalidation always runs after resolver
latency. Payloads are CDP arguments, never spliced into script source;
there is no coordinate tier for text. SELECT payloads are grounded in
observed enabled options when observe captured them: resolver text must
equal one option label, else exactly one option label must occur in the
goal, else the agent abstains. The deterministic resolver excludes
literals successfully typed earlier, then picks an untyped quoted literal
whose preceding three goal words share a word with the field label, else
the first untyped literal, else abstains (parity with the bench A3q helper).
A literal right after a press verb (`press "Submit"`) names a control,
not a value.
