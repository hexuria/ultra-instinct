# Anti-drift 2: Instinct owns HOW, Ultra-Instinct owns WHAT

Rule 2 of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**Instinct owns HOW, Ultra-Instinct owns WHAT.** Instinct picks among finite offered
actions (scores, threshold / margin, abstain). Ultra-Instinct supplies browser
evidence and builds the action space. No browser concepts in Instinct. No
second float confidence gate on the agent path. Abstain is never turned
into "top candidate wins". Habituation only lowers the urge of
a repeated `dispatched-no-visible-change` / wrong-effect action; it never
adds candidates or bypasses the gate. A click whose only change is a
target's control state (checked / expanded / value / selected /
aria-selected / aria-current) verifies
as state-changed, without that state entering the fingerprint.
