"""A9 / A10: ultra-instinct's own agent loop (`aui run`) on the harness Chrome.

- A9 = ``--policy instinct``: Instinct decides locally, no model calls.
- A10 = ``--policy jev``: JEV decides every step through the same loop
  (observe -> gate -> ticket -> verify). Calls go through the counting proxy
  (the harness sets TYPESAFE_BASE_URL).

Drives the first page target, which the harness opened at the task's start URL.
TYPE / SELECT payloads come from the goal's quoted literals (the deterministic
resolver); no text model. Needs ``cargo build --release -p aui-cli --features jev``.

A10m (``arm_options.text == "model"``, MiniWoB race): payloads come from the HTTP text
model in ``--features model-text`` builds, configured exactly like jev-ultrafast
(``TEXT_MODEL_API=openai``, ``TEXT_MODEL_BASE_URL`` = the harness text proxy, ``TEXT_MODEL``,
a placeholder ``TEXT_MODEL_API_KEY``). Needs ``--features jev,model-text``.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from arms.common import Trace, goal_text, load_spec  # noqa: E402

STEP = re.compile(r"^step (\d+) (\S+) (\".*\") -> (\S+)$")
OUTCOME = re.compile(r"^outcome (\w+): (.*)$")
STATUS = {"done": "done", "blocked": "give_up", "abstained": "give_up"}


def label_of(debug: str) -> str:
    try:
        return json.loads(debug)
    except json.JSONDecodeError:
        return debug.strip('"')


def main() -> None:
    spec = load_spec()
    trace = Trace(spec["trace"])
    policy = spec["arm_options"]["policy"]
    goal = spec["task"]["goal"] if spec["arm_options"].get("bare_goal") else goal_text(spec)
    argv = [str(Path(spec["root"]) / "target/release/aui"), "run", "--goal", goal,
            "--cdp", spec["cdp_http"], "--max-steps", str(spec["caps"]["steps"]), "--policy", policy]
    env = None
    if spec["arm_options"].get("text") == "model":
        env = {**os.environ, "TEXT_MODEL_API": "openai", "TEXT_MODEL_BASE_URL": spec["text_base"],
               "TEXT_MODEL": spec["text_model"], "TEXT_MODEL_API_KEY": "local"}
    proc = subprocess.run(argv, cwd=spec["root"], capture_output=True, text=True, env=env)
    sys.stdout.write(proc.stdout + proc.stderr)
    kind, note = None, ""
    for line in (proc.stdout + proc.stderr).splitlines():
        if m := STEP.match(line):
            trace.action(json.dumps({"action": m[2], "label": label_of(m[3]), "verification": m[4]}, sort_keys=True),
                         tool=m[2].split(":")[0])
        elif m := OUTCOME.match(line):
            kind, note = m[1], m[2]
    trace.final(STATUS.get(kind or "", "error"), f"aui {policy}: {kind}: {note} (exit {proc.returncode})")


if __name__ == "__main__":
    main()
