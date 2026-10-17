"""MiniWoB++ race: the same arms, harness Chrome, caps and counting proxy as ``run.py``,
on a third-party task source (``miniwob.py``).

    bench/bench miniwob --config bench/race-miniwob.toml                  # the full race
    bench/bench miniwob --config bench/race-miniwob.toml --tasks click-button,enter-text --seeds 1
    bench/bench miniwob --arms mock-wob-solve,mock-wob-noop --tasks click-button,enter-text,click-checkboxes
    bench/bench miniwob --resume <run-id>
    bench/bench miniwob --probe-only                                      # task list + determinism, no arms
    bench/bench miniwob-report <run-id> [--publish]

Per (task, seed) unit, both arms run back to back (order shuffled per unit from the run
seed) on a fresh Chrome profile each. Success is MiniWoB's own reward > 0 on the first
episode end, reported by the page; no episode end before the attempt stops is a fail.
When the episode ends the arm gets ``end_grace_s`` to finish, then the harness stops it
(``episode_end``): the episode is over, the page shows MiniWoB's START cover.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import re
import subprocess
import sys
import time
import tomllib
from pathlib import Path

BENCH = Path(__file__).resolve().parent
ROOT = BENCH.parent
sys.path.insert(0, str(BENCH))

import miniwob  # noqa: E402
import text_proxy  # noqa: E402
from chrome import Chrome  # noqa: E402
from proxy import ModelProxy  # noqa: E402
from run import ARMS, MAIN_PY, cleanup_external, kill_group, read_trace, sh, stuck_by  # noqa: E402

RUNS = BENCH / "runs" / "miniwob"
SECRETS = ("TYPESAFE_API_KEY", "ANTHROPIC_API_KEY", "TEXT_MODEL_API_KEY", "TEXT_MODEL_BASE_URL", "TEXT_MODEL",
           "TEXT_MODEL_API", "TEXT_MODEL_REASONING")


def seeds_for(run_seed: int, n: int) -> list[int]:
    """The seed list every task and every arm uses, fixed by the run seed."""
    return random.Random(f"miniwob-{run_seed}").sample(range(1, 1_000_000), n)


def needs_text(arm: dict) -> bool:
    return arm.get("options", {}).get("text") == "model"


def file_sha(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()[:16]
    except OSError:
        return ""


def pyver(pkg: str) -> str:
    return sh([str(MAIN_PY), "-c", f"import importlib.metadata as m; print(m.version('{pkg}'))"])


def manifest(cfg: dict, run_id: str, seed: int, seeds: list[int], chrome_version: str, tasks: list[dict],
             probed: dict, order: list[str]) -> dict:
    aui = ROOT / "target/release/aui"
    return {
        "run_id": run_id,
        "started": time.strftime("%Y-%m-%d %H:%M:%S %z"),
        "git": {
            "ultra_instinct_head": sh(["git", "rev-parse", "HEAD"]),
            "dirty": bool(sh(["git", "status", "--porcelain", "--untracked-files=no"])),
            "jev_ultrafast": sh(["git", "rev-parse", "HEAD"], BENCH / "vendor" / "jev-ultrafast"),
            "miniwob_plusplus": sh(["git", "rev-parse", "HEAD"], miniwob.VENDOR),
            "miniwob_repo": miniwob.REPO,
        },
        "aui_binary": {"path": str(aui.relative_to(ROOT)), "sha256_16": file_sha(aui),
                       "features": cfg.get("aui", {}).get("features", "")},
        "chrome": chrome_version,
        "python": {"browser_harness": pyver("browser-harness"), "anthropic": pyver("anthropic")},
        "models": {**cfg["models"], "text_upstream": text_proxy.TEXT_MODEL_UPSTREAM},
        "caps": cfg["caps"],
        "viewport": cfg["viewport"],
        "miniwob": cfg["miniwob"],
        "seed": seed,
        "seeds": seeds,
        "arms": cfg["run"]["arms"],
        "tasks_registered": len(tasks),
        "tasks_included": sorted(t for t, p in probed.items() if not p["excluded"]),
        "tasks_excluded": {t: p["excluded"] for t, p in probed.items() if p["excluded"]},
        "config_hash": hashlib.sha256(json.dumps(cfg, sort_keys=True).encode()).hexdigest()[:12],
        "order": order,
        "keys_present": {k: bool(os.environ.get(k)) for k in ("TYPESAFE_API_KEY", "ANTHROPIC_API_KEY")},
    }


def run_attempt(arm_id: str, task: dict, seed: int, goal: str, out: Path, cfg: dict, server: miniwob.WobServer,
                proxy: ModelProxy, chrome_port: int) -> dict:
    arm = ARMS[arm_id]
    caps = cfg["caps"]
    grace = float(cfg["miniwob"].get("end_grace_s", 3))
    out.mkdir(parents=True, exist_ok=True)
    trace_path = out / "trace.jsonl"
    trace_path.unlink(missing_ok=True)
    attempt_id = f"{arm_id}-{task['task']}-{seed}-{time.time_ns()}"
    start_url = server.url(task, seed, attempt_id, caps["wall_s"] * 1000)
    bu_name = re.sub(r"[^A-Za-z0-9-]", "-", f"wob-{arm_id}-{task['task']}-{seed}-{os.getpid()}")
    proxy.meter.reset(caps["model_calls"])
    chrome = Chrome(chrome_port, cfg["viewport"]["width"], cfg["viewport"]["height"],
                    headless=cfg["viewport"].get("headless", True)).launch("about:blank" if arm.get("blank_start") else start_url)
    page_ws = chrome.first_page()["webSocketDebuggerUrl"]
    spec = {
        "arm": arm_id,
        "arm_options": arm.get("options", {}),
        "task": {"scenario": "miniwob", "id": task["task"], "class": "normal", "goal": goal, "start": task["path"],
                 "needs": [], "seed": seed},
        "start_url": start_url,
        "server": server.base,
        "cdp_http": chrome.http,
        "page_ws": page_ws,
        "viewport": cfg["viewport"],
        "luna_model": cfg["models"].get("luna", ""),
        "luna_effort": cfg["models"].get("luna_effort", "low"),
        "luna_base": proxy.base + "/luna/v1",
        "typesafe_base": proxy.base + "/typesafe",
        "jev_model": cfg["models"]["jev"],
        "text_base": proxy.base + "/text/v1",
        "text_model": cfg["models"]["text"],
        "caps": caps,
        "trace": str(trace_path),
        "bu_name": bu_name,
        "root": str(ROOT),
        "out": str(out),
    }
    (out / "spec.json").write_text(json.dumps(spec, indent=1))
    env = {k: v for k, v in os.environ.items() if k not in SECRETS}
    env.update({"PYTHONUNBUFFERED": "1", "ANONYMIZED_TELEMETRY": "false", "BROWSER_USE_CLOUD_SYNC": "false",
                "BU_NAME": bu_name})
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
    end_seen = None
    while True:
        time.sleep(0.25)
        rows = read_trace(trace_path)
        rec = server.store.get(attempt_id)
        fp = str(rec["v"])
        for row in rows[seen:]:
            if row.get("ev") == "action":
                trace_rows.append((row.get("sig", ""), fp))
        seen = len(rows)
        if rec["end"] and end_seen is None:
            end_seen = time.time()
        if proc.poll() is not None:
            break
        if end_seen and time.time() - end_seen > grace:
            outcome = "episode_end"
            break
        if proxy.meter.cap_hit:
            outcome, cap = "cap_hit", "model_calls"
            break
        if len(trace_rows) > caps["steps"]:
            outcome, cap = "cap_hit", "steps"
            break
        if time.time() - t0 > caps["wall_s"]:
            outcome, cap = "cap_hit", "wall"
            break
        if len(trace_rows) >= caps["stuck_repeats"] and stuck_by(trace_rows, caps["stuck_repeats"], fp):
            time.sleep(1.0)  # identical actions, no DOM change since the first of them
            if str(server.store.get(attempt_id)["v"]) == fp:
                outcome = "stuck"
                break
    wall = time.time() - t0
    kill_group(proc)
    log.close()
    time.sleep(0.6)
    rec = server.store.get(attempt_id)
    rows = read_trace(trace_path)
    finals = [r for r in rows if r.get("ev") == "final"]
    final_status = finals[-1]["status"] if finals else None
    if outcome == "finished" and (proc.returncode not in (0, None) or final_status in (None, "error")):
        outcome = "crashed"
    cleanup_external(rows, bu_name)
    if arm.get("bu_daemon"):
        try:
            subprocess.run([str(MAIN_PY), "-c", "import sys; from browser_harness.admin import restart_daemon; restart_daemon(sys.argv[1])", bu_name],
                           capture_output=True, timeout=60, env={**env, "BU_NAME": bu_name})
        except subprocess.TimeoutExpired:
            print(f"warn: restart_daemon timed out for {bu_name}; continuing", flush=True)
    chrome.kill()
    meter = proxy.meter.snapshot()
    end, start = rec["end"], rec["start"]
    # Only an end reported while the attempt was live counts (the 0.6 s settle covers the POST in flight).
    reward = end["reward"] if end else None
    result = {
        "arm": arm_id, "task": task["task"], "seed": seed, "attempt_id": attempt_id,
        "success": bool(end) and isinstance(reward, (int, float)) and reward > 0,
        "reward": reward, "raw_reward": end["raw_reward"] if end else None, "reward_reason": end["reason"] if end else None,
        "episode_ended": bool(end), "episode_started": bool(start),
        "reward_s": round(end["at"] - t0, 3) if end else None,
        "utterance_match": bool(start) and start["utterance"] == goal,
        "ends_ignored": rec["ends_ignored"],
        "outcome": outcome, "cap": cap, "final_status": final_status,
        "final_note": finals[-1].get("note", "") if finals else "",
        "exit_code": proc.returncode, "steps": len(trace_rows),
        "jev_calls": meter["calls"]["typesafe"], "text_calls": meter["calls"]["text"],
        "model_calls": meter["calls_total"], "tokens_total": meter["tokens_total"], "model_errors": meter["model_errors"],
        "jev_latency_ms": meter["latency_ms_by_route"]["typesafe"], "text_latency_ms": meter["latency_ms_by_route"]["text"],
        "in_loop_s": rows[-1].get("t") if rows else None,
        "wall_s": round(wall, 2),
    }
    (out / "result.json").write_text(json.dumps(result, indent=1))
    return result


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(BENCH / "race-miniwob.toml"))
    ap.add_argument("--seed", type=int)
    ap.add_argument("--seeds", type=int, help="seeds per task (default: config run.seeds_per_task)")
    ap.add_argument("--arms")
    ap.add_argument("--tasks", help="comma list of task names (smoke runs); default: every registered task")
    ap.add_argument("--wall", type=int, help="override the wall cap (smoke tests)")
    ap.add_argument("--resume")
    ap.add_argument("--probe-only", action="store_true", help="probe tasks x seeds (utterance, determinism) and stop")
    args = ap.parse_args()
    cfg = tomllib.loads(Path(args.config).read_text())
    seed = args.seed if args.seed is not None else cfg["run"]["seed"]
    if args.wall:
        cfg["caps"]["wall_s"] = args.wall
    if args.arms:
        cfg["run"]["arms"] = args.arms.split(",")
    arms = cfg["run"]["arms"]
    head = sh(["git", "rev-parse", "HEAD"], miniwob.VENDOR)
    if head != cfg["miniwob"]["pin"] or head != miniwob.PIN:
        raise SystemExit(f"miniwob-plusplus checkout is {head or 'missing'}; the race pins {cfg['miniwob']['pin']} "
                         f"(clone {miniwob.REPO} into {miniwob.VENDOR} and check out the pin)")
    aui = ROOT / "target/release/aui"
    if any(ARMS[a]["script"] == "arms/a9_aui.py" and needs_text(ARMS[a]) for a in arms):
        if not aui.exists() or b"TEXT_MODEL_API_KEY" not in aui.read_bytes():
            raise SystemExit("target/release/aui lacks the HTTP text model; "
                             "cargo build --release -p aui-cli --features jev,model-text")
    seeds = seeds_for(seed, args.seeds or cfg["run"]["seeds_per_task"])
    tasks = miniwob.registered_tasks()
    if args.tasks:
        keep = args.tasks.split(",")
        unknown = set(keep) - {t["task"] for t in tasks}
        if unknown:
            raise SystemExit(f"not in the pinned registration: {sorted(unknown)}")
        tasks = [t for t in tasks if t["task"] in keep]
    run_id = args.resume or time.strftime("%Y%m%d-%H%M%S")
    run_dir = RUNS / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    ports = cfg["ports"]
    server = miniwob.WobServer(ports["server"]).start()
    proxy = ModelProxy(ports["proxy"]).start()
    try:
        probe_path = run_dir / "probe.json"
        probed = json.loads(probe_path.read_text()) if probe_path.exists() else {}
        todo = [t for t in tasks if t["task"] not in probed]
        if todo:
            probed.update(miniwob.probe(todo, seeds, server, ports["probe_chrome"], cfg["caps"]["wall_s"] * 1000))
            probe_path.write_text(json.dumps(probed, indent=1))
        included = [t for t in tasks if not probed[t["task"]]["excluded"]]
        if args.probe_only:
            excluded = {t["task"]: probed[t["task"]]["excluded"] for t in tasks if probed[t["task"]]["excluded"]}
            print(f"probe: {len(included)} included, {len(excluded)} excluded {json.dumps(excluded, indent=1)}")
            return
        rng = random.Random(seed)
        units = [(t["task"], s) for t in included for s in seeds]
        rng.shuffle(units)
        plan = [(task, s, rng.sample(arms, len(arms))) for task, s in units]
        order = [f"{a}/{task}/{s}" for task, s, arm_order in plan for a in arm_order]
        man_path = run_dir / "manifest.json"
        if not man_path.exists():
            probe_chrome = Chrome(ports["chrome"]).launch("about:blank")
            version = probe_chrome.version()
            probe_chrome.kill()
            man_path.write_text(json.dumps(manifest(cfg, run_id, seed, seeds, version, tasks, probed, order), indent=1))
        results_path = run_dir / "results.jsonl"
        done = set()
        if results_path.exists():
            for line in results_path.read_text().splitlines():
                row = json.loads(line)
                done.add(f"{row['arm']}/{row['task']}/{row['seed']}")
        by_name = {t["task"]: t for t in included}
        total = len(order)
        i = 0
        for task_name, s, arm_order in plan:
            task = by_name[task_name]
            goal = probed[task_name]["seeds"][str(s)]["utterance"]
            for arm_id in arm_order:
                i += 1
                unit = f"{arm_id}/{task_name}/{s}"
                if unit in done:
                    continue
                arm = ARMS[arm_id]
                missing = []
                if arm["typesafe"] and not os.environ.get("TYPESAFE_API_KEY"):
                    missing.append("TYPESAFE_API_KEY")
                if needs_text(arm) and not os.environ.get("ANTHROPIC_API_KEY"):
                    missing.append("ANTHROPIC_API_KEY")
                if missing:
                    row = {"arm": arm_id, "task": task_name, "seed": s, "not_run": f"{', '.join(missing)} not available"}
                    print(f"[{i}/{total}] {unit}: not run ({row['not_run']})", flush=True)
                else:
                    out = run_dir / "attempts" / task_name / str(s) / arm_id
                    row = run_attempt(arm_id, task, s, goal, out, cfg, server, proxy, ports["chrome"])
                    print(f"[{i}/{total}] {unit}: {'PASS' if row['success'] else 'fail'} reward={row['reward']} "
                          f"{row['outcome']}{'/' + row['cap'] if row['cap'] else ''} final={row['final_status']} "
                          f"steps={row['steps']} jev={row['jev_calls']} text={row['text_calls']} "
                          f"loop={row['in_loop_s']}s wall={row['wall_s']}s"
                          + ("" if row["utterance_match"] else " UTTERANCE-MISMATCH"), flush=True)
                with results_path.open("a") as fh:
                    fh.write(json.dumps(row) + "\n")
    finally:
        server.stop()
        proxy.stop()
    print(f"run {run_id} -> {run_dir}")


if __name__ == "__main__":
    main()
