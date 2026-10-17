"""MiniWoB plumbing check arms (no models): ``solve`` and ``noop``.

``solve`` reads the goal (the utterance the harness read from ``#query``) and solves
click-button, enter-text and click-checkboxes with trusted CDP input on the attempt's
page; any other task gives up. ``noop`` waits a second and claims done without
acting. Neither reads the reward: the harness gets it from the page.
"""

from __future__ import annotations

import json
import re
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from arms.common import Trace, load_spec  # noqa: E402
from chrome import Cdp  # noqa: E402

QUOTED = re.compile(r'"([^"]+)"')


def main() -> None:
    spec = load_spec()
    trace = Trace(spec["trace"])
    if spec["arm_options"]["mode"] == "noop":
        time.sleep(1.0)
        trace.final("done", "noop: claimed done without acting")
        return
    cdp = Cdp(spec["page_ws"])

    def ev(expr: str):
        return cdp.call("Runtime.evaluate", expression=expr, returnByValue=True)["result"].get("value")

    deadline = time.time() + 15
    while not ev("!!(window.__wob && window.__wob.started)"):
        if time.time() > deadline:
            trace.final("error", "episode never started")
            return
        time.sleep(0.1)

    def click(selector_js: str, label: str) -> None:
        trace.action(json.dumps({"click": label}), tool="click")
        box = ev(f"(function(){{var e = {selector_js}; if (!e) return null; var r = e.getBoundingClientRect();"
                 f" return [r.left + r.width / 2, r.top + r.height / 2];}})()")
        if box is None:
            raise RuntimeError(f"no element for {label}")
        x, y = box
        cdp.call("Input.dispatchMouseEvent", type="mouseMoved", x=x, y=y)
        cdp.call("Input.dispatchMouseEvent", type="mousePressed", x=x, y=y, button="left", clickCount=1)
        cdp.call("Input.dispatchMouseEvent", type="mouseReleased", x=x, y=y, button="left", clickCount=1)

    goal, task = spec["task"]["goal"], spec["task"]["id"]
    if task == "click-button":
        want = QUOTED.search(goal)[1]
        click(f"[...document.querySelectorAll('#area button')].find(b => b.textContent.trim() === {json.dumps(want)})", want)
    elif task == "enter-text":
        want = QUOTED.search(goal)[1]
        click("document.getElementById('tt')", "text field")
        trace.action(json.dumps({"type": want}), tool="type")
        cdp.call("Input.insertText", text=want)
        click("document.getElementById('subbtn')", "Submit")
    elif task == "click-checkboxes":
        names = [n.strip() for n in re.match(r"Select (.*) and click Submit\.", goal)[1].split(",")]
        names = [n for n in names if n and n != "nothing"]
        for name in names:
            click(f"[...document.querySelectorAll('#boxes label')].find(l => l.textContent.trim() === {json.dumps(name)})"
                  f".querySelector('input')", name)
        click("document.getElementById('subbtn')", "Submit")
    else:
        trace.final("give_up", f"no script for {task}")
        return
    trace.final("done", "scripted")


if __name__ == "__main__":
    main()
