"""Run the uniform benchmark: every arm, every task, same Chrome, same server, same caps.

    bench/bench run --mock                      # oracle/saboteur/spinner/... (no keys)
    bench/bench run --config bench/bench.toml --seed 42
    bench/bench run ... --arms A1,A6 --tasks am-star,hd-star --resume <run-id>
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import signal
import subprocess
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

BENCH = Path(__file__).resolve().parent
ROOT = BENCH.parent
sys.path.insert(0, str(BENCH))

from checker import check  # noqa: E402
from chrome import Chrome  # noqa: E402
from proxy import ModelProxy  # noqa: E402
from server import BenchServer  # noqa: E402
from tasks import EXAMPLES, load_all  # noqa: E402

MAIN_PY = BENCH / ".venv/bin/python"
CUA_PY = BENCH / "envs/cua/.venv/bin/python"
ULTRA_INSTINCT_PIN = "4ae30d3"

ARMS = {
    "A1": {"name": "Luna + Browser Use", "python": MAIN_PY, "script": "arms/a1_browser_use.py", "browser": "harness", "typesafe": False},
    "A2": {"name": "Luna + CUA driver", "python": CUA_PY, "script": "arms/a2_luna_cua.py", "browser": "cua", "typesafe": False},
    "A3": {"name": "jev-ultrafast (BU + JEV)", "python": MAIN_PY, "script": "arms/a3_jev_ultrafast.py", "browser": "harness", "typesafe": True,
           "blank_start": True, "bu_daemon": True},
    "A4": {"name": "CUA jev-use (generic task)", "python": CUA_PY, "script": "arms/a4_jev_use.py", "browser": "cua", "typesafe": True},
    "A5": {"name": "JEV + ultra-instinct (live_drive)", "python": MAIN_PY, "script": "arms/a5_live_drive.py", "browser": "harness", "typesafe": True},
    "A6": {"name": "Luna + ultra-instinct (MCP)", "python": MAIN_PY, "script": "arms/a6_luna_ultra_instinct.py", "browser": "harness", "typesafe": False},
    # Combined arms (bench/arms/COMBO.md): Luna plans, ultra-instinct observes/presses, JEV breaks ties, executor types/selects.
    "A7": {"name": "Luna + JEV + ultra-instinct + CUA", "python": MAIN_PY, "script": "arms/a7_combo_cua.py", "browser": "cua", "typesafe": True},
    "A8": {"name": "Luna + JEV + ultra-instinct + Browser Use", "python": MAIN_PY, "script": "arms/a8_combo_bu.py", "browser": "harness", "typesafe": True},
}
# Head-to-head race (bench/race.toml): text from quoted goal literals on every arm, no text model.
ARMS["A3q"] = {**ARMS["A3"], "name": "jev-ultrafast (quoted-literal text)", "options": {"text": "quoted"}}
ARMS["A9"] = {"name": "ultra-instinct aui run (Instinct)", "python": MAIN_PY, "script": "arms/a9_aui.py", "browser": "harness",
              "typesafe": False, "options": {"policy": "instinct"}}
ARMS["A10"] = {"name": "ultra-instinct aui run (--policy jev)", "python": MAIN_PY, "script": "arms/a9_aui.py", "browser": "harness",
               "typesafe": True, "options": {"policy": "jev"}}
# Diagnostic only (not a race arm): A9 without the shared GOAL_SUFFIX.
ARMS["A9b"] = {**ARMS["A9"], "name": "ultra-instinct aui run (Instinct, bare goal; diagnostic)",
               "options": {"policy": "instinct", "bare_goal": True}}
ARMS["A11"] = {"name": "ultra-instinct aui run (--policy clef-flash)", "python": MAIN_PY, "script": "arms/a9_aui.py", "browser": "harness",
               "typesafe": False, "options": {"policy": "clef-flash"}}
ARMS["A12"] = {"name": "ultra-instinct aui run (--policy clef)", "python": MAIN_PY, "script": "arms/a9_aui.py", "browser": "harness",
               "typesafe": False, "options": {"policy": "clef"}}
# MiniWoB++ race (bench/race-miniwob.toml, run_miniwob.py): both arms type with Claude Haiku 5.5
# through the harness text proxy; JEV decides on both.
ARMS["A3m"] = {**ARMS["A3"], "name": "jev-ultrafast (text: Haiku 5.5 via text proxy)", "options": {"text": "model"}}
ARMS["A10m"] = {**ARMS["A10"], "name": "ultra-instinct aui run --policy jev (text: Haiku 5.5 via text proxy)",
                "options": {"policy": "jev", "text": "model"}}
for mode in ["solve", "noop"]:
    ARMS[f"mock-wob-{mode}"] = {"name": f"mock MiniWoB {mode}", "python": MAIN_PY, "script": "arms/wob_mock.py",
                                "browser": "harness", "typesafe": False, "options": {"mode": mode}, "mock": True}
for mode in ["oracle", "saboteur", "spinner", "wanderer", "sleeper"]:
    ARMS[f"mock-{mode}"] = {"name": f"mock {mode}", "python": MAIN_PY, "script": "arms/mock.py", "browser": "harness",
                            "typesafe": False, "options": {"mode": mode}, "mock": True}


def sh(cmd: list[str], cwd: Path = ROOT) -> str:
    try:
        return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=20).stdout.strip()
    except Exception:
        return ""


def manifest(cfg: dict, seed: int, chrome_version: str, run_id: str, order: list[str]) -> dict:
    vendor = BENCH / "vendor"
    cfg_hash = hashlib.sha256(json.dumps(cfg, sort_keys=True).encode()).hexdigest()[:12]
    return {
        "run_id": run_id,
        "started": time.strftime("%Y-%m-%d %H:%M:%S %z"),
        "git": {
            "bench_uniform_head": sh(["git", "rev-parse", "HEAD"]),
            "dirty": bool(sh(["git", "status", "--porcelain", "--untracked-files=no"])),
            "aui_merged": ULTRA_INSTINCT_PIN,
            "aui_merged_full": sh(["git", "rev-parse", ULTRA_INSTINCT_PIN]),
            "jev_ultrafast": sh(["git", "rev-parse", "HEAD"], vendor / "jev-ultrafast"),
            "cua": sh(["git", "rev-parse", "HEAD"], vendor / "cua"),
        },
        "chrome": chrome_version,
        "cua_driver": sh([os.path.expanduser("~/.local/bin/cua-driver"), "--version"]),
        "browser_use": sh([str(MAIN_PY), "-c", "import importlib.metadata as m; print(m.version('browser-use'))"]),
        "models": cfg["models"],
        "caps": cfg["caps"],
        "viewport": cfg["viewport"],
        "seed": seed,
        "reps": cfg["run"]["reps"],
        "max_tries": cfg["run"]["max_tries"],
        "config_hash": cfg_hash,
        "order": order,
        "typesafe_key_present": bool(os.environ.get("TYPESAFE_API_KEY")),
    }


def read_trace(path: Path) -> list[dict]:
    if not path.exists():
        return []
    out = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            out.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    return out


def journal(server: BenchServer) -> dict:
    return server.journal.snapshot()


def stuck_by(rows: list[tuple[str, str]], repeats: int, current_fp: str) -> bool:
    """rows: (signature, fingerprint_before). Stuck = last N identical, none changed state."""
    if len(rows) < repeats:
        return False
    tail = rows[-repeats:]
    return len({sig for sig, _ in tail}) == 1 and len({fp for _, fp in tail} | {current_fp}) == 1


def kill_group(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=4)
    except Exception:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except Exception:
            pass


def cleanup_external(trace_rows: list[dict], bu_name: str) -> None:
    for row in trace_rows:
        if row.get("ev") == "browser" and row.get("pid"):
            try:
                os.kill(int(row["pid"]), signal.SIGTERM)
            except Exception:
                pass
    # browser-harness daemons detach from the arm's process group
    subprocess.run(["pkill", "-f", f"BU_NAME={bu_name}"], capture_output=True)
    subprocess.run(["pkill", "-f", bu_name], capture_output=True)


def run_attempt(arm_id: str, task, attempt: int, out: Path, cfg: dict, server: BenchServer, proxy: ModelProxy,
                chrome_port: int) -> dict:
    arm = ARMS[arm_id]
    caps = cfg["caps"]
    out.mkdir(parents=True, exist_ok=True)
    trace_path = out / "trace.jsonl"
    trace_path.unlink(missing_ok=True)
    start_url = server.base + task.start
    bu_name = f"hub-{arm_id}-{task.id}-{attempt}-{os.getpid()}"
    server.journal.reset(f"{arm_id}/{task.id}/{attempt}")
    proxy.meter.reset(caps["model_calls"])
    chrome = None
    page_ws = None
    if arm["browser"] == "harness":
        chrome = Chrome(chrome_port, cfg["viewport"]["width"], cfg["viewport"]["height"], headless=cfg["viewport"].get("headless", True)).launch("about:blank" if arm.get("blank_start") else start_url)
        page_ws = chrome.first_page()["webSocketDebuggerUrl"]
    spec = {
        "arm": arm_id,
        "arm_options": arm.get("options", {}),
        "task": task.spec(),
        "task_steps": {"oracle": task.oracle, "saboteur": task.saboteur},
        "start_url": start_url,
        "server": server.base,
        "cdp_http": chrome.http if chrome else None,
        "page_ws": page_ws,
        "viewport": cfg["viewport"],
        "luna_model": cfg["models"]["luna"],
        "luna_effort": cfg["models"]["luna_effort"],
        "luna_base": proxy.base + "/luna/v1",
        "typesafe_base": proxy.base + "/typesafe",
        "jev_model": cfg["models"]["jev"],
        "caps": caps,
        "trace": str(trace_path),
        "bu_name": bu_name,
        "root": str(ROOT),
        "out": str(out),
    }
    (out / "spec.json").write_text(json.dumps(spec, indent=1))
    env = {k: v for k, v in os.environ.items() if k != "TYPESAFE_API_KEY"}
    env.update({"PYTHONUNBUFFERED": "1", "ANONYMIZED_TELEMETRY": "false", "BROWSER_USE_CLOUD_SYNC": "false",
                "BROWSER_USE_LOGGING_LEVEL": "info", "BU_NAME": bu_name})
    if arm["typesafe"] and os.environ.get("TYPESAFE_API_KEY"):
        env["TYPESAFE_API_KEY"] = os.environ["TYPESAFE_API_KEY"]
        env["TYPESAFE_BASE_URL"] = proxy.base + "/typesafe"
    log = open(out / "arm.log", "w")
    t0 = time.time()
    proc = subprocess.Popen([str(arm["python"]), str(BENCH / arm["script"]), str(out / "spec.json")],
                            cwd=str(BENCH), env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    outcome, cap = "finished", None
    trace_rows: list[tuple[str, str]] = []
    seen = 0
    while True:
        time.sleep(0.25)
        elapsed = time.time() - t0
        rows = read_trace(trace_path)
        fp = server.journal.fingerprint()
        for row in rows[seen:]:
            if row.get("ev") == "action":
                trace_rows.append((row.get("sig", ""), fp))
        seen = len(rows)
        snap = journal(server)
        acts = snap["acts"]
        if proc.poll() is not None:
            break
        if proxy.meter.cap_hit:
            outcome, cap = "cap_hit", "model_calls"
            break
        if len(trace_rows) > caps["steps"]:
            outcome, cap = "cap_hit", "steps"
            break
        if len(acts) > caps["page_acts"]:
            outcome, cap = "cap_hit", "page_acts"
            break
        if elapsed > caps["wall_s"]:
            outcome, cap = "cap_hit", "wall"
            break
        page_rows = [(f"{a.get('kind')}:{a.get('target')}", a.get("fingerprint_before", "")) for a in acts]
        last_act_age = (time.time() - server.journal.started) - (acts[-1]["received"] if acts else 0)
        if acts and last_act_age > 1.0 and stuck_by(page_rows, caps["stuck_repeats"], fp):
            outcome = "stuck"
            break
        if trace_rows and stuck_by(trace_rows, caps["stuck_repeats"], fp) and len(trace_rows) >= caps["stuck_repeats"]:
            # trace-level repeats with no page change at all (also covers actions the page never saw)
            time.sleep(1.0)
            if server.journal.fingerprint() == fp:
                outcome = "stuck"
                break
    wall = time.time() - t0
    kill_group(proc)
    log.close()
    time.sleep(0.6)
    snap = journal(server)
    rows = read_trace(trace_path)
    finals = [r for r in rows if r.get("ev") == "final"]
    final_status = finals[-1]["status"] if finals else None
    if outcome == "finished" and (proc.returncode not in (0, None) or final_status in (None, "error")):
        outcome = "crashed"
    viewports = [p.get("viewport") for p in snap["pages"].values() if p.get("viewport")]
    cleanup_external(rows, bu_name)
    if arm.get("bu_daemon"):
        try:
            subprocess.run([str(MAIN_PY), "-c", "import sys; from browser_harness.admin import restart_daemon; restart_daemon(sys.argv[1])", bu_name],
                           capture_output=True, timeout=60, env={**os.environ, "BU_NAME": bu_name})
        except subprocess.TimeoutExpired:
            # Mac flakiness: daemon restart can hang; do not drop a completed attempt.
            print(f"warn: restart_daemon timed out for {bu_name}; continuing", flush=True)
    if chrome:
        chrome.kill()
    meter = proxy.meter.snapshot()
    verdict = check(task, snap, final_status, outcome)
    result = {
        "arm": arm_id, "scenario": task.scenario, "task": task.id, "class": task.cls, "needs": task.needs,
        "attempt": attempt, "outcome": outcome, "cap": cap, "final_status": final_status,
        "final_note": finals[-1].get("note", "") if finals else "",
        "exit_code": proc.returncode, "steps": len(trace_rows), "page_acts": len(snap["acts"]),
        "model_calls": meter["calls_total"], "calls": meter["calls"], "tokens_in": sum(meter["tokens_in"].values()),
        "tokens_out": sum(meter["tokens_out"].values()), "tokens_total": meter["tokens_total"],
        "model_errors": meter["model_errors"], "wall_s": round(wall, 2), "viewport": max(viewports, key=lambda v: v[0] * v[1]) if viewports else None,
        **{k: verdict[k] for k in ("pass", "unmet", "forbidden", "wrong_actions", "safe_refusal", "events")},
    }
    (out / "journal.json").write_text(json.dumps(snap, indent=1))
    (out / "result.json").write_text(json.dumps(result, indent=1))
    return result


def summarize_task(attempts: list[dict]) -> dict:
    first = attempts[0]
    return {
        "arm": first["arm"], "scenario": first["scenario"], "task": first["task"], "class": first["class"],
        "needs": first["needs"], "tries_used": len(attempts),
        "pass_first": bool(first["pass"]), "pass_within": any(a["pass"] for a in attempts),
        "attempt_outcomes": [{"outcome": a["outcome"], "cap": a["cap"], "pass": a["pass"], "final": a["final_status"]} for a in attempts],
        "wrong_actions": sum(a["wrong_actions"] for a in attempts),
        "wrong_actions_first": first["wrong_actions"],
        "safe_refusal_first": first["safe_refusal"],
        "cap_hits": sum(a["outcome"] == "cap_hit" for a in attempts),
        "stuck": sum(a["outcome"] == "stuck" for a in attempts),
        "crashed": sum(a["outcome"] == "crashed" for a in attempts),
        "steps": sum(a["steps"] for a in attempts), "steps_first": first["steps"],
        "model_calls": sum(a["model_calls"] for a in attempts), "model_calls_first": first["model_calls"],
        "tokens": sum(a["tokens_total"] for a in attempts), "tokens_first": first["tokens_total"],
        "wall_s": round(sum(a["wall_s"] for a in attempts), 2), "wall_s_first": first["wall_s"],
        "viewport": first["viewport"],
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(BENCH / "bench.toml"))
    ap.add_argument("--seed", type=int)
    ap.add_argument("--mock", action="store_true", help="run the mock arms only (no keys)")
    ap.add_argument("--arms")
    ap.add_argument("--tasks")
    ap.add_argument("--scenarios")
    ap.add_argument("--reps", type=int)
    ap.add_argument("--max-tries", type=int)
    ap.add_argument("--wall", type=int, help="override the wall cap (mock smoke tests)")
    ap.add_argument("--resume")
    args = ap.parse_args()
    cfg = tomllib.loads(Path(args.config).read_text())
    seed = args.seed if args.seed is not None else cfg["run"]["seed"]
    if args.reps:
        cfg["run"]["reps"] = args.reps
    if args.max_tries:
        cfg["run"]["max_tries"] = args.max_tries
    if args.wall:
        cfg["caps"]["wall_s"] = args.wall
    arms = args.arms.split(",") if args.arms else (["mock-oracle", "mock-saboteur"] if args.mock else cfg["run"]["arms"])
    scenarios = args.scenarios.split(",") if args.scenarios else cfg["run"]["scenarios"]
    tasks = load_all(scenarios)
    if args.tasks:
        keep = set(args.tasks.split(","))
        tasks = [t for t in tasks if t.id in keep]
    run_id = args.resume or time.strftime("%Y%m%d-%H%M%S") + ("-mock" if args.mock else "")
    run_dir = BENCH / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    units = [(a, t.id, rep) for a in arms for t in tasks for rep in range(cfg["run"]["reps"])]
    random.Random(seed).shuffle(units)
    order = [f"{a}/{t}/{r}" for a, t, r in units]
    by_id = {t.id: t for t in tasks}
    ports = cfg["ports"]
    server = BenchServer(EXAMPLES, ports["server"]).start()
    proxy = ModelProxy(ports["proxy"]).start()
    probe = Chrome(ports["chrome"]).launch("about:blank")
    chrome_version = probe.version()
    probe.kill()
    man_path = run_dir / "manifest.json"
    if not man_path.exists():
        man_path.write_text(json.dumps(manifest(cfg, seed, chrome_version, run_id, order), indent=1))
    results_path = run_dir / "results.jsonl"
    done = set()
    if results_path.exists():
        for line in results_path.read_text().splitlines():
            row = json.loads(line)
            done.add(f"{row['arm']}/{row['task']}/{row.get('rep', 0)}")
    key_present = bool(os.environ.get("TYPESAFE_API_KEY"))
    try:
        for i, (arm_id, task_id, rep) in enumerate(units, 1):
            unit = f"{arm_id}/{task_id}/{rep}"
            if unit in done:
                continue
            task = by_id[task_id]
            arm = ARMS[arm_id]
            if arm["typesafe"] and not key_present:
                row = {"arm": arm_id, "scenario": task.scenario, "task": task_id, "rep": rep, "class": task.cls,
                       "needs": task.needs, "not_run": "TYPESAFE_API_KEY not available (JEV calls need it)"}
            elif not (ROOT / "bench" / arm["script"]).exists():
                row = {"arm": arm_id, "scenario": task.scenario, "task": task_id, "rep": rep, "class": task.cls,
                       "needs": task.needs, "not_run": f"arm script missing: {arm['script']}"}
            else:
                attempts = []
                for attempt in range(1, cfg["run"]["max_tries"] + 1):
                    out = EXAMPLES / task.scenario / "runs" / run_id / arm_id / task_id / f"rep-{rep}" / f"attempt-{attempt}"
                    res = run_attempt(arm_id, task, attempt, out, cfg, server, proxy, ports["chrome"])
                    attempts.append(res)
                    print(f"[{i}/{len(units)}] {unit} try {attempt}: {'PASS' if res['pass'] else 'fail'} "
                          f"{res['outcome']}{'/' + res['cap'] if res['cap'] else ''} steps={res['steps']} "
                          f"calls={res['model_calls']} {res['wall_s']}s {res['unmet'] or ''} {res['forbidden'] or ''}", flush=True)
                    if res["pass"]:
                        break
                row = {**summarize_task(attempts), "rep": rep, "attempts": attempts}
            with results_path.open("a") as fh:
                fh.write(json.dumps(row) + "\n")
            if "not_run" in row:
                print(f"[{i}/{len(units)}] {unit}: not run ({row['not_run']})", flush=True)
    finally:
        server.stop()
        proxy.stop()
    print(f"run {run_id} -> {results_path}")


if __name__ == "__main__":
    main()
