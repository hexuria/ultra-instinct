"""Counting model proxy: every model call from every arm goes through here.

Routes:
- ``/luna/...``     -> OpenCodex (``http://127.0.0.1:8080/...``), GPT 6 Luna
- ``/typesafe/...`` -> ``https://api.typesafe.ai/...``, JEV
- ``/text/v1/chat/completions`` -> Claude Haiku 5.5 on the native Anthropic Messages API,
  translated in-process by ``text_proxy.py`` (the MiniWoB race's shared text model)

The proxy counts calls and tokens per attempt from outside the arm. Once the
attempt's model-call cap is reached it answers 429 without forwarding and
flags the attempt, and the harness kills the arm (``cap_hit``). Request and
response bodies are not stored; auth headers are passed through and never
logged.
"""

from __future__ import annotations

import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx

UPSTREAMS = {
    "luna": "http://127.0.0.1:8080",
    "typesafe": "https://api.typesafe.ai",
    "text": "local:text_proxy",  # answered in-process, see _text()
}
ROUTES = list(UPSTREAMS)
HOP = {"host", "content-length", "connection", "accept-encoding", "transfer-encoding", "keep-alive"}


def _num(value) -> int:
    return int(value) if isinstance(value, (int, float)) and not isinstance(value, bool) else 0


def usage_tokens(payload) -> tuple[int, int]:
    """Input and output tokens from an OpenAI-style or TypeSafe-style usage block."""
    if not isinstance(payload, dict):
        return 0, 0
    usage = payload.get("usage") or {}
    if not isinstance(usage, dict):
        return 0, 0
    inp = _num(usage.get("prompt_tokens")) or _num(usage.get("input_tokens"))
    out = _num(usage.get("completion_tokens")) or _num(usage.get("output_tokens"))
    return inp, out


class Meter:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.reset(10**9)

    def reset(self, call_cap: int) -> None:
        with self.lock:
            self.call_cap = call_cap
            self.calls = {r: 0 for r in ROUTES}
            self.tokens_in = {r: 0 for r in ROUTES}
            self.tokens_out = {r: 0 for r in ROUTES}
            self.latency_ms: list[float] = []
            self.latency_by_route: dict[str, list[float]] = {r: [] for r in ROUTES}
            self.errors = 0
            self.cap_hit = False

    def total_calls(self) -> int:
        with self.lock:
            return sum(self.calls.values())

    def admit(self, route: str) -> bool:
        with self.lock:
            if sum(self.calls.values()) >= self.call_cap:
                self.cap_hit = True
                return False
            self.calls[route] += 1
            return True

    def record(self, route: str, payload, ms: float, ok: bool) -> None:
        inp, out = usage_tokens(payload)
        with self.lock:
            self.tokens_in[route] += inp
            self.tokens_out[route] += out
            self.latency_ms.append(ms)
            self.latency_by_route[route].append(round(ms, 1))
            if not ok:
                self.errors += 1

    def snapshot(self) -> dict:
        with self.lock:
            return {
                "calls": dict(self.calls),
                "calls_total": sum(self.calls.values()),
                "tokens_in": dict(self.tokens_in),
                "tokens_out": dict(self.tokens_out),
                "tokens_total": sum(self.tokens_in.values()) + sum(self.tokens_out.values()),
                "model_errors": self.errors,
                "model_latency_ms": list(self.latency_ms),
                "latency_ms_by_route": {k: list(v) for k, v in self.latency_by_route.items()},
                "call_cap_hit": self.cap_hit,
            }


class ProxyHandler(BaseHTTPRequestHandler):
    meter: Meter
    client: httpx.Client
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args) -> None:
        pass

    def _reply(self, code: int, body: bytes, headers: dict | None = None) -> None:
        self.send_response(code)
        for key, value in (headers or {}).items():
            if key.lower() not in HOP and key.lower() != "content-encoding":
                self.send_header(key, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _forward(self, method: str) -> None:
        parts = self.path.lstrip("/").split("/", 1)
        route = parts[0]
        if route not in UPSTREAMS:
            return self._reply(404, b'{"error":"unknown route"}', {"Content-Type": "application/json"})
        rest = "/" + (parts[1] if len(parts) > 1 else "")
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else None
        counted = method == "POST"  # GET /v1/models etc. are not model calls
        if counted and not self.meter.admit(route):
            return self._reply(429, b'{"error":"bench model-call cap reached"}', {"Content-Type": "application/json"})
        if route == "text":
            return self._text(rest, body)
        headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP}
        started = time.perf_counter()
        try:
            upstream = self.client.request(method, UPSTREAMS[route] + rest, content=body, headers=headers)
        except httpx.HTTPError as error:
            if counted:
                self.meter.record(route, None, (time.perf_counter() - started) * 1000, False)
            msg = json.dumps({"error": f"proxy upstream failure: {type(error).__name__}"}).encode()
            return self._reply(502, msg, {"Content-Type": "application/json"})
        ms = (time.perf_counter() - started) * 1000
        content = upstream.content
        if counted:
            try:
                payload = json.loads(content)
            except (json.JSONDecodeError, UnicodeDecodeError):
                payload = None
            self.meter.record(route, payload, ms, upstream.status_code < 400)
        self._reply(upstream.status_code, content, dict(upstream.headers))

    def _text(self, rest: str, body: bytes | None) -> None:
        """OpenAI chat completions -> Anthropic Messages (text_proxy). Already admitted and counted."""
        import text_proxy

        if rest.rstrip("/") not in ("/v1/chat/completions", "/chat/completions"):
            return self._reply(404, b'{"error":"text route serves /v1/chat/completions only"}',
                               {"Content-Type": "application/json"})
        started = time.perf_counter()
        try:
            request = json.loads(body or b"{}")
        except (json.JSONDecodeError, UnicodeDecodeError):
            request = None
        if not isinstance(request, dict):
            code, payload = 400, {"error": {"message": "body is not a JSON object"}}
        else:
            code, payload = text_proxy.complete(request)
        self.meter.record("text", payload, (time.perf_counter() - started) * 1000, code < 400)
        self._reply(code, json.dumps(payload).encode(), {"Content-Type": "application/json"})

    def do_POST(self) -> None:  # noqa: N802
        self._forward("POST")

    def do_GET(self) -> None:  # noqa: N802
        self._forward("GET")


class ModelProxy:
    def __init__(self, port: int) -> None:
        self.meter = Meter()
        ProxyHandler.meter = self.meter
        ProxyHandler.client = httpx.Client(timeout=120, http2=False)
        self.httpd = ThreadingHTTPServer(("127.0.0.1", port), ProxyHandler)
        self.port = self.httpd.server_address[1]
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def start(self) -> "ModelProxy":
        self.thread.start()
        return self

    def stop(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()
