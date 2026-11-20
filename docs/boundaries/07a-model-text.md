# Anti-drift 7a: Model text is payload-only and grounded

Rule 7a of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**Model text is payload-only and grounded** (feature
`model-text`). The model fills the payload of an action Instinct already chose;
replies must echo the context fingerprint, pass shape checks, and occur
in the goal clause and not be a described phrase ("your answer"), else
deterministic fallback, else abstain. One exception (on by default in
the CLI whenever a text model is configured, off with
`--no-derived-text`): a value read or worked out
from page text may be typed when the goal quotes no untyped value, the
field is not secret (password / hidden / file inputs; PIN /
CVV / card / one-time-code labels), and the value is one line of ≤ 64
chars, or copied word for word from the page text; every derived value
is printed. The model sees the page's reading text (rendered text plus
text-area contents) and the field's near text. CI uses scripted models only; no
provider SDK or key handling in the tree.
