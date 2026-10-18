# ADR 0006: Model-text TextResolver (feature `model-text`)

- Status: accepted
- Date: 2026-10-06 (Asia/Manila)
- Baseline: `5c7d893`
- Relates: ADR 0001 §4 (TextResolver is separate), ADR 0002 §6, ADR 0003,
  ADR 0005 §R2 (one staleness barrier)

## Context

`DeterministicTextResolver` only extracts quoted values and the
`type X into F` / `fill F with X` forms. Goals such as
`type rust ownership in the Search box` or `select business in Cabin class`
choose the right target through Instinct but leave the payload unresolved, so the
agent fails with nothing typed. ADR 0002 reserved the `model-text` feature
name for a model-backed resolver; it was a stub.

## Decision

1. **Payload only.** `ModelTextResolver<M: TextModel, F = DeterministicTextResolver>`
   implements `TextResolver`. It runs **after** Instinct has chosen a TYPE_TEXT /
   SELECT action from the finite `ActionSpace`, and only proposes that
   action's payload. It never sees other regions, selectors, or coordinates,
   never chooses a target, and never dispatches input. Gate → ActionTicket →
   executor revalidate → consume → input is unchanged.
2. **Injectable client.** `TextModel::complete(&TextModelRequest)` is the only
   model seam. The request is the active goal clause, field label, field role
   (`select` for SELECT), the `TextContext::fingerprint()`, and `max_chars`.
   Shipped clients: `ScriptedTextModel` (tests, offline demos) and
   `CommandTextModel` (runs a user program, one JSON line in, one JSON reply
   out). ultra-instinct has **no** HTTP client, provider SDK, or credential handling
   for this path; a live LLM is the user's program and owns its own keys.
   Stderr of that program is discarded so it cannot leak into ultra-instinct output.
3. **Every reply is vetted** (`vet`):
   - **Context binding** — the reply must echo the request's context
     fingerprint. A reply bound to another context (stale / reordered) is
     refused (`ModelRefusal::StaleContext`). The agent then also checks the
     resolution's fingerprint against the live `TextContext`, as for every
     resolver.
   - **Shape** — trimmed, one layer of matching quotes stripped, non-empty,
     `<= max_chars` (default 256), no control characters (no newline / tab /
     escape smuggling).
   - **Grounding** (`Grounding::Goal`, default) — the value must occur
     (case-insensitive) in the active goal clause: the model may *extract* what
     the deterministic patterns miss, never invent. A bare echo of the field
     label (label occurs once in the goal) is refused. `Grounding::ShapeOnly`
     exists for generative fields and is opt-in only.
4. **Fallback, then abstain.** On a model error or refused reply the resolver
   tries its fallback (default `DeterministicTextResolver`). If there is no
   fallback (`without_fallback`) or it also fails, it returns
   `TextError::Abstain`; the agent maps that to `AgentError::Abstain` →
   `AgentOutcome::Abstained` with nothing typed — the same first-class abstain
   as Instinct, never a guessed value. `last_source()` reports
   `Model | Fallback(reason) | Abstained(reason)`; `model_calls()` counts calls.
5. **Staleness stays single-barrier.** Model latency is covered exactly like
   deterministic resolution (ADR 0005 §R2): the payload is resolved before the
   ticket is consumed, and the executor's fresh observe + revalidate catches
   any target drift. A stale ticket discards the prediction; the next predict
   builds a new `TextContext` and asks the model again with the new
   fingerprint (tested).
6. **Feature-gated, default unchanged.** `ultra-instinct-policy/model-text`
   (optional `serde_json` for the command protocol),
   `ultra-instinct-agent/model-text` (`AgentBuilder::model_text(model)`), and
   `ultra-instinct/model-text` (`run --text-model-cmd <program>`). Without the
   feature nothing is compiled in; with it, `AgentBuilder::new` still uses
   `DeterministicTextResolver` until `model_text` / `text_resolver` is called.

## Consequences

- CI runs clippy + tests for the feature with scripted / local `sh` models
  only. No paid API, no network, no secrets.
- SELECT values are grounded in the goal, not in the page's option list: the
  manifold does not carry `<option>`s yet. The page-side `select_option`
  still refuses a non-unique / missing option (`InputRejected`, nothing
  changed). Feeding options into `TextModelRequest` is future work.
- Grounding deliberately limits the model to extraction. Generative payloads
  (compose an email body) need `ShapeOnly` and a deliberate decision.
- No live LLM numbers are claimed. EVAL lists the model path as offline-tested.

## Discarded

- Model chooses the target or returns a selector: violates ADR 0001.
- Built-in HTTP provider client: adds a network dependency and key handling
  to the core for an optional path; the command adapter covers it.
- Silent fallback to top-ranked / first deterministic guess after a refused
  model reply without the grounding checks.
