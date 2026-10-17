"""Text-model proxy: OpenAI ``POST /v1/chat/completions`` in, native Anthropic Messages out.

Both race arms (jev-ultrafast and ``aui run`` with ``TEXT_MODEL_API=openai``) send the
same OpenAI-compatible request shape for TYPE_TEXT / SELECT payloads:
``response_format: {"type": "json_object"}`` plus a ``reasoning`` object. Anthropic's
own OpenAI-compatible endpoint rejects ``json_object``, so this module translates to
the Messages API through the official ``anthropic`` SDK (pinned by browser-use to
0.76.0, which predates the ``output_config`` parameter, so it rides ``extra_body``).

Mapping (identical for every caller):

- ``system`` messages -> ``system`` (joined); ``user`` / ``assistant`` -> ``messages``.
- ``max_tokens`` passes through (default 1024).
- ``reasoning`` -> ``output_config: {"effort": ...}``: ``reasoning.effort`` passes through
  when it is ``low`` / ``medium`` / ``high``; anything else, including
  ``reasoning.enabled: false``, becomes ``low`` (Haiku 5.5 thinking cannot be reliably
  disabled; low effort is the closest knob). No ``reasoning`` object -> no ``output_config``.
- ``response_format: json_object`` -> no structured-output schema; the reply text is cut
  to its first complete top-level JSON object (code fences stripped) and returned as
  ``choices[0].message.content``. A reply with no JSON object is returned as is, so the
  caller's own parser decides.
- The request's ``model`` is ignored: every call goes to ``TEXT_MODEL_UPSTREAM``
  (``claude-haiku-5-5``), so no arm can pick a different text model.

The key is read from ``ANTHROPIC_API_KEY`` in the harness process only; it is never
logged, stored, or passed to an arm.
"""

from __future__ import annotations

import json
import os
import re
import time

TEXT_MODEL_UPSTREAM = "claude-haiku-5-5"
EFFORTS = {"low", "medium", "high"}
FENCE = re.compile(r"```(?:json|JSON)?\s*")

_client = None


def client():
    global _client
    if _client is None:
        import anthropic

        _client = anthropic.Anthropic(api_key=os.environ.get("ANTHROPIC_API_KEY"), max_retries=2, timeout=60)
    return _client


def _text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):  # OpenAI content parts
        return "".join(p.get("text", "") for p in content if isinstance(p, dict))
    return "" if content is None else str(content)


def to_anthropic(body: dict) -> dict:
    """OpenAI chat-completions body -> ``messages.create`` kwargs."""
    system, messages = [], []
    for msg in body.get("messages") or []:
        role, text = msg.get("role"), _text(msg.get("content"))
        if role in ("system", "developer"):
            system.append(text)
        elif role in ("user", "assistant"):
            if messages and messages[-1]["role"] == role:
                messages[-1]["content"] += "\n\n" + text
            else:
                messages.append({"role": role, "content": text})
    kwargs: dict = {
        "model": TEXT_MODEL_UPSTREAM,
        "max_tokens": int(body.get("max_tokens") or body.get("max_completion_tokens") or 1024),
        "messages": messages,
    }
    if system:
        kwargs["system"] = "\n\n".join(system)
    reasoning = body.get("reasoning")
    if isinstance(reasoning, dict):
        effort = reasoning.get("effort")
        kwargs["extra_body"] = {"output_config": {"effort": effort if effort in EFFORTS else "low"}}
    return kwargs


def first_json_object(text: str) -> str | None:
    """The first complete top-level JSON object in ``text`` (fences stripped), re-serialized."""
    clean = FENCE.sub("", text)
    decoder = json.JSONDecoder()
    i = clean.find("{")
    while i != -1:
        try:
            obj, _ = decoder.raw_decode(clean, i)
        except json.JSONDecodeError:
            i = clean.find("{", i + 1)
            continue
        if isinstance(obj, dict):
            return json.dumps(obj, ensure_ascii=False)
        i = clean.find("{", i + 1)
    return None


FINISH = {"end_turn": "stop", "stop_sequence": "stop", "max_tokens": "length", "refusal": "content_filter"}


def to_openai(message, wants_json: bool) -> dict:
    text = "".join(getattr(b, "text", "") for b in message.content if getattr(b, "type", "") == "text")
    content = (first_json_object(text) or text) if wants_json else text
    usage = message.usage
    return {
        "id": message.id,
        "object": "chat.completion",
        "created": int(time.time()),
        "model": message.model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": content},
                     "finish_reason": FINISH.get(message.stop_reason or "", "stop")}],
        "usage": {"prompt_tokens": usage.input_tokens, "completion_tokens": usage.output_tokens,
                  "total_tokens": usage.input_tokens + usage.output_tokens},
    }


def complete(body: dict) -> tuple[int, dict]:
    """One OpenAI-shaped call -> (http status, OpenAI-shaped payload). Never raises."""
    import anthropic

    wants_json = (body.get("response_format") or {}).get("type") == "json_object"
    try:
        message = client().messages.create(**to_anthropic(body))
    except anthropic.APIStatusError as error:
        return error.status_code, {"error": {"message": f"anthropic {error.status_code}: {type(error).__name__}",
                                             "type": "upstream"}}
    except anthropic.APIError as error:
        return 502, {"error": {"message": f"anthropic: {type(error).__name__}", "type": "upstream"}}
    return 200, to_openai(message, wants_json)
