"""MiniWoB++ as a third-party task source: pinned checkout, local server, injected episode harness.

The tasks are Farama-Foundation/miniwob-plusplus at ``PIN`` (cloned, gitignored, at
``bench/vendor/miniwob-plusplus``). The task list is the checkout's own gymnasium
registration (``miniwob/registration.py``), no hand-picking. Pages are served from
``miniwob/html`` by ``WobServer``; every task entry page (``miniwob/<task>.html`` and
``flight/<site>/wrapper.html``) gets one extra ``<script>`` right after MiniWoB's own
``core.js`` (served at ``/_wob/harness.js``) that, when the URL has ``seed`` and
``attempt``:

- seeds: ``Math.seedrandom(seed)`` at parse time and again just before
  ``core.startEpisodeReal()``, the same call MiniWoB's Python env makes
  (``set_seed`` then ``begin_task``), so a seed gives the same instance to every arm;
- raises ``core.EPISODE_MAX_TIME`` to the race wall cap (the page's own value is
  overwritten after its scripts ran, before the episode starts);
- auto-starts: after ``load`` (and every load handler, including the page's own
  ``core.startEpisode()`` that shows the START cover) it calls
  ``core.startEpisodeReal()``, which generates the problem and hides the cover;
- reports: ``core.endEpisode`` is wrapped; the first call that actually ends the
  live episode (MiniWoB's own guard: ``EP_TIMER !== null``) POSTs
  ``{attempt, reward, raw_reward, reason}`` to ``/_wob/end``. The harness keeps the
  first report per attempt only and never judges success itself.

It also POSTs ``/_wob/start`` (the utterance MiniWoB put in ``#query``) and a coarse
``/_wob/state`` DOM version (mutations outside MiniWoB's reward HUD, plus input /
change events) that the harness uses only for its ``stuck`` cap.
"""

from __future__ import annotations

import json
import re
import threading
import time
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

BENCH = Path(__file__).resolve().parent
VENDOR = BENCH / "vendor" / "miniwob-plusplus"
HTML = VENDOR / "miniwob" / "html"
REGISTRATION = VENDOR / "miniwob" / "registration.py"
REPO = "https://github.com/Farama-Foundation/miniwob-plusplus"
PIN = "33c3b4ddef8c6eb67c57a29663d844b1eda7e614"

REGISTER = re.compile(r'register\(\s*id="miniwob/([^"]+)-v1",(.*?)\)', re.S)
CORE_TAG = re.compile(r'<script[^>]*src="(?:\.\./)+core/core\.js"[^>]*>\s*</script>', re.I)


def task_path(name: str) -> str:
    """Server path of a task's entry page, as MiniWoB's own SeleniumInstance builds it."""
    if name.startswith("flight."):
        return "/" + name.replace(".", "/") + "/wrapper.html"
    return f"/miniwob/{name}.html"


def registered_tasks() -> list[dict]:
    """Every env in the pinned registration, in file order, with its nondeterministic flag."""
    out = []
    for m in REGISTER.finditer(REGISTRATION.read_text()):
        out.append({"task": m[1], "path": task_path(m[1]), "nondeterministic": "nondeterministic=True" in m[2]})
    return out


HARNESS_JS = r"""
(function () {
  var q = new URLSearchParams(location.search);
  var seed = q.get('seed'), attempt = q.get('attempt');
  if (seed === null || attempt === null) return;
  var maxMs = parseInt(q.get('max_ms') || '180000', 10);
  var W = window.__wob = {seed: seed, attempt: attempt, started: false, ended: false, v: 0};
  function post(path, body) {
    body.attempt = attempt;
    try {
      fetch(path, {method: 'POST', headers: {'Content-Type': 'application/json'},
                   body: JSON.stringify(body), keepalive: true});
    } catch (e) {}
  }
  Math.seedrandom(seed);  // parse-time randomness (top-level task scripts) is seeded too
  function hud(node) {
    for (; node; node = node.parentNode) {
      var id = node.id;
      if (id === 'reward-display' || id === 'click-canvas' || id === 'attention-canvas') return true;
    }
    return false;
  }
  var posted = -1;
  function bump() { W.v++; }
  setInterval(function () { if (W.v !== posted) { posted = W.v; post('/_wob/state', {v: W.v}); } }, 250);
  window.addEventListener('load', function () {
    setTimeout(function () {
      var endEpisode = core.endEpisode;
      core.endEpisode = function () {
        var live = core.EP_TIMER !== null;
        var out = endEpisode.apply(this, arguments);
        if (live && !W.ended) {
          W.ended = true;
          post('/_wob/end', {reward: WOB_REWARD_GLOBAL, raw_reward: WOB_RAW_REWARD_GLOBAL,
                             reason: WOB_REWARD_REASON == null ? null : String(WOB_REWARD_REASON)});
        }
        return out;
      };
      core.EPISODE_MAX_TIME = maxMs;
      Math.seedrandom(seed);
      core.startEpisodeReal();
      W.started = true;
      post('/_wob/start', {utterance: core.getUtterance(), max_ms: core.EPISODE_MAX_TIME});
      new MutationObserver(function (records) {
        for (var i = 0; i < records.length; i++) { if (!hud(records[i].target)) { bump(); return; } }
      }).observe(document.body, {subtree: true, childList: true, attributes: true, characterData: true});
      document.addEventListener('input', bump, true);
      document.addEventListener('change', bump, true);
    }, 0);
  });
})();
"""


class WobStore:
    """Per-attempt records posted by the injected script. First episode end wins."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.rows: dict[str, dict] = {}
        self.t0 = time.time()

    def _row(self, attempt: str) -> dict:
        return self.rows.setdefault(attempt, {"start": None, "end": None, "ends_ignored": 0, "v": 0, "v_at": None})

    def add(self, kind: str, body: dict) -> None:
        attempt = str(body.get("attempt", ""))
        now = time.time()
        with self.lock:
            row = self._row(attempt)
            if kind == "start":
                if row["start"] is None:
                    row["start"] = {"utterance": body.get("utterance"), "max_ms": body.get("max_ms"), "at": now}
            elif kind == "end":
                if row["end"] is None:
                    row["end"] = {"reward": body.get("reward"), "raw_reward": body.get("raw_reward"),
                                  "reason": body.get("reason"), "at": now}
                else:
                    row["ends_ignored"] += 1
            elif kind == "state":
                v = body.get("v")
                if isinstance(v, int):
                    row["v"], row["v_at"] = v, now

    def get(self, attempt: str) -> dict:
        with self.lock:
            return json.loads(json.dumps(self._row(attempt)))


class WobHandler(SimpleHTTPRequestHandler):
    store: WobStore
    entry_paths: set[str]

    def log_message(self, *_args) -> None:
        pass

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def _send(self, code: int, data: bytes, ctype: str) -> None:
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            body = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            body = {}
        kind = {"/_wob/start": "start", "/_wob/end": "end", "/_wob/state": "state"}.get(self.path.split("?")[0])
        if kind is None or not isinstance(body, dict):
            return self._send(404, b'{"error":"unknown"}', "application/json")
        self.store.add(kind, body)
        self._send(200, b'{"ok":true}', "application/json")

    def do_GET(self) -> None:  # noqa: N802
        path = self.path.split("?")[0]
        if path == "/_wob/harness.js":
            return self._send(200, HARNESS_JS.encode(), "application/javascript")
        if path in self.entry_paths:
            file = HTML / path.lstrip("/")
            html = file.read_text(encoding="utf-8", errors="replace")
            html, n = CORE_TAG.subn(lambda m: m[0] + '\n<script src="/_wob/harness.js"></script>', html, count=1)
            if n:
                return self._send(200, html.encode("utf-8"), "text/html; charset=utf-8")
        super().do_GET()


class WobServer:
    def __init__(self, port: int) -> None:
        self.store = WobStore()
        WobHandler.store = self.store
        WobHandler.entry_paths = {t["path"] for t in registered_tasks()}
        self.httpd = ThreadingHTTPServer(("127.0.0.1", port), partial(WobHandler, directory=str(HTML)))
        self.port = self.httpd.server_address[1]
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def url(self, task: dict, seed: int, attempt: str, max_ms: int) -> str:
        return f"{self.base}{task['path']}?seed={seed}&attempt={attempt}&max_ms={max_ms}"

    def start(self) -> "WobServer":
        self.thread.start()
        return self

    def stop(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()


# Hash of the rendered instance (MiniWoB's reward HUD and click canvas removed), to
# check that one seed renders one instance. Flight tasks render inside a same-origin iframe.
DOM_HASH_JS = r"""
(function () {
  var b = document.body.cloneNode(true);
  ['reward-display', 'click-canvas', 'sync-task-cover'].forEach(function (id) {
    var n = b.querySelector('#' + id); if (n) n.remove(); });
  var s = b.innerHTML;
  var f = document.getElementById('wrap');
  if (f && f.tagName === 'IFRAME') { try { s += f.contentDocument.body.innerHTML; } catch (e) {} }
  var h = 2166136261;
  for (var i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619) >>> 0; }
  return h.toString(16);
})()
"""


def probe(tasks: list[dict], seeds: list[int], server: WobServer, chrome_port: int, max_ms: int,
          log=print) -> dict:
    """Load every task x seed twice (a fresh tab each) in one harness Chrome; read the utterance MiniWoB put in
    ``#query`` (posted by the injected script) and an instance hash.

    Returns ``{task: {"seeds": {seed: {...}}, "excluded": reason | None}}``. Mechanical
    exclusion only: the page never started an episode, the utterance is empty, or the same
    seed gave two different utterances.
    """
    from chrome import Cdp, Chrome

    chrome = Chrome(chrome_port, headless=True).launch("about:blank")
    out: dict = {}

    def load(url: str, attempt: str) -> dict | None:
        """One fresh tab per load: the episode must start and post its utterance within 10 s."""
        browser = Cdp(chrome.browser_ws())
        target = browser.call("Target.createTarget", url=url)["targetId"]
        try:
            started = None
            deadline = time.time() + 10
            while time.time() < deadline and not started:
                started = server.store.get(attempt)["start"]
                time.sleep(0.05)
            if not started:
                return None
            time.sleep(0.4)  # let the instance render (some tasks finish building after start)
            page = Cdp(f"ws://127.0.0.1:{chrome_port}/devtools/page/{target}")
            try:
                dom = page.call("Runtime.evaluate", expression=DOM_HASH_JS, returnByValue=True)["result"].get("value")
            finally:
                page.close()
            return {"utterance": started["utterance"], "dom": dom}
        finally:
            try:
                browser.call("Target.closeTarget", targetId=target)
            finally:
                browser.close()

    try:
        for task in tasks:
            info: dict = {"seeds": {}, "excluded": None, "dom_drift": [], "nondeterministic": task["nondeterministic"]}
            for seed in seeds:
                reads = []
                for k in range(2):
                    attempt = f"probe-{task['task']}-{seed}-{k}-{time.time_ns()}"
                    try:
                        reads.append(load(server.url(task, seed, attempt, max_ms), attempt))
                    except Exception as error:  # noqa: BLE001 - a page that breaks CDP is excluded, not fatal
                        log(f"probe {task['task']} seed {seed}: {type(error).__name__}: {error}")
                        reads.append(None)
                if None in reads:
                    info["excluded"] = "page did not start an episode within 10 s"
                elif not (reads[0]["utterance"] or "").strip():
                    info["excluded"] = "empty utterance (#query)"
                elif reads[0]["utterance"] != reads[1]["utterance"]:
                    info["excluded"] = "same seed gave different utterances"
                if info["excluded"]:
                    break
                if reads[0]["dom"] != reads[1]["dom"]:
                    info["dom_drift"].append(seed)
                info["seeds"][str(seed)] = {"utterance": reads[0]["utterance"], "dom": reads[0]["dom"]}
            out[task["task"]] = info
            log(f"probe {task['task']}: {info['excluded'] or 'ok'}"
                + (f" (dom drift on seeds {info['dom_drift']})" if info["dom_drift"] else ""))
    finally:
        chrome.kill()
    return out
