"""Report for one MiniWoB++ race run (``run_miniwob.py``).

    bench/bench miniwob-report <run-id>            # -> bench/runs/miniwob/<run-id>/REPORT.md
    bench/bench miniwob-report <run-id> --publish  # also bench/results/MINIWOB-<run-id>.{md,json}

Success is MiniWoB's own reward > 0 on the first episode end, as the page reported it.
Timing: in-loop = ``t`` of the arm's last trace row; process wall = the harness timer from
arm start to stop; time to reward = arm start to the page's end-of-episode POST. They are
reported separately, never mixed.
"""

from __future__ import annotations

import argparse
import json
import shutil
import statistics
import sys
from pathlib import Path

BENCH = Path(__file__).resolve().parent
sys.path.insert(0, str(BENCH))
from run import ARMS  # noqa: E402

RUNS = BENCH / "runs" / "miniwob"


def med(values) -> str:
    vals = [v for v in values if isinstance(v, (int, float))]
    return f"{statistics.median(vals):.2f}" if vals else "–"


def mean(values) -> str:
    vals = [v for v in values if isinstance(v, (int, float))]
    return f"{statistics.fmean(vals):.2f}" if vals else "–"


def pct(n: int, d: int) -> str:
    return f"{100 * n / d:.1f}% ({n}/{d})" if d else "–"


def load(run_id: str) -> tuple[dict, dict, list[dict]]:
    d = RUNS / run_id
    man = json.loads((d / "manifest.json").read_text())
    probe = json.loads((d / "probe.json").read_text())
    rows = [json.loads(line) for line in (d / "results.jsonl").read_text().splitlines() if line.strip()]
    # resume can append a unit twice only if a run was killed mid-write; keep the last
    by_key = {(r["arm"], r["task"], r["seed"]): r for r in rows}
    return man, probe, list(by_key.values())


def build(run_id: str) -> tuple[str, dict]:
    man, probe, rows = load(run_id)
    arms = [a for a in man["arms"] if any(r["arm"] == a for r in rows)]
    ran = {a: [r for r in rows if r["arm"] == a and "not_run" not in r] for a in arms}
    not_run = {a: [r for r in rows if r["arm"] == a and "not_run" in r] for a in arms}
    tasks = sorted({r["task"] for r in rows})
    key = {(r["arm"], r["task"], r["seed"]): r for r in rows if "not_run" not in r}
    both = [(t, s) for t in tasks for s in man["seeds"] if all((a, t, s) in key for a in arms)]
    both_pass = [(t, s) for t, s in both if all(key[(a, t, s)]["success"] for a in arms)]

    out = [
        f"# MiniWoB++ race: {' vs '.join(arms)} (run `{run_id}`)",
        "",
        f"Config `bench/race-miniwob.toml`, run seed {man['seed']}, seeds {man['seeds']} (the same for every task and arm), "
        f"`max_tries = 1`. Tasks: {len(man['tasks_included'])} of the {man['tasks_registered']} envs in MiniWoB++'s own "
        f"`miniwob/registration.py` at `{man['git']['miniwob_plusplus'][:12]}`"
        + (f" ({len(man['tasks_excluded'])} excluded mechanically, listed below)." if man["tasks_excluded"] else "."),
        "",
        "Success = MiniWoB's own reward > 0 at the first episode end, posted by the page. No episode end before the "
        "attempt stopped = fail. The harness never judges success itself. Both arms get the task's own `#query` "
        "utterance plus the shared goal suffix (`bench/arms/common.py`).",
        "",
        "| Arm | What |",
        "|---|---|",
        *[f"| {a} | {ARMS[a]['name']} |" for a in arms],
        "",
        "## Overall",
        "",
        "| Arm | Success | Mean reward (ended) | No episode end | Crashed | Cap hits | Stuck | Steps / attempt | JEV calls / attempt | Text calls / attempt | Model errors | In-loop s, median | Process wall s, median | Time to reward s, median (successes) |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for a in arms:
        rs = ran[a]
        out.append(
            f"| {a} | **{pct(sum(r['success'] for r in rs), len(rs))}** | {mean(r['reward'] for r in rs if r['episode_ended'])} | "
            f"{sum(not r['episode_ended'] for r in rs)} | {sum(r['outcome'] == 'crashed' for r in rs)} | "
            f"{sum(r['outcome'] == 'cap_hit' for r in rs)} | {sum(r['outcome'] == 'stuck' for r in rs)} | "
            f"{mean(r['steps'] for r in rs)} | {mean(r['jev_calls'] for r in rs)} | {mean(r['text_calls'] for r in rs)} | "
            f"{sum(r['model_errors'] for r in rs)} | {med(r['in_loop_s'] for r in rs)} | {med(r['wall_s'] for r in rs)} | "
            f"{med(r['reward_s'] for r in rs if r['success'])} |"
        )
    out += [
        "",
        f"On the {len(both_pass)} task-seeds both arms passed: "
        + "; ".join(f"{a} in-loop median {med(key[(a, t, s)]['in_loop_s'] for t, s in both_pass)} s, "
                    f"time to reward median {med(key[(a, t, s)]['reward_s'] for t, s in both_pass)} s, "
                    f"process wall median {med(key[(a, t, s)]['wall_s'] for t, s in both_pass)} s" for a in arms) + ".",
        "",
        "Timing: **in-loop** is the `t` of the arm's last trace row (its own loop clock); **process wall** is the "
        "harness timer from arm start to stop, including interpreter / daemon startup; **time to reward** is arm "
        f"start to the page's end-of-episode report. After an episode ends the arm gets {man['miniwob'].get('end_grace_s', 3)} s "
        "to finish, then the harness stops it (`episode_end`), so wall time after a reward is bounded the same way for both.",
        "",
        "Outcomes: " + "; ".join(
            f"{a}: " + ", ".join(f"{o} {n}" for o, n in sorted(
                {o: sum(r['outcome'] == o for r in ran[a]) for o in {r['outcome'] for r in ran[a]}}.items()))
            for a in arms) + ".",
        "",
        "## Per task (successes / seeds run)",
        "",
        "| Task | Flags | " + " | ".join(arms) + " |",
        "|---|---|" + "---:|" * len(arms),
    ]
    for t in tasks:
        info = probe.get(t, {})
        flags = ", ".join(f for f, on in (("nondeterministic (upstream flag)", info.get("nondeterministic")),
                                           (f"dom drift seeds {info.get('dom_drift')}", info.get("dom_drift"))) if on) or "–"
        cells = []
        for a in arms:
            rs = [r for r in ran[a] if r["task"] == t]
            cells.append(f"{sum(r['success'] for r in rs)}/{len(rs)}" if rs else "–")
        out.append(f"| `{t}` | {flags} | " + " | ".join(cells) + " |")
    problems = [r for a in arms for r in ran[a] if r["outcome"] in ("crashed", "cap_hit", "stuck") or not r["utterance_match"]]
    out += ["", "## Errors, crashes, caps", ""]
    if problems:
        out += ["| Arm | Task | Seed | Outcome | Final | Reward | Note |", "|---|---|---:|---|---|---:|---|"]
        for r in sorted(problems, key=lambda r: (r["arm"], r["task"], r["seed"])):
            note = (r["final_note"] or "").replace("|", "/").replace("\n", " ")[:160]
            if not r["utterance_match"]:
                note = "UTTERANCE MISMATCH vs probe; " + note
            out.append(f"| {r['arm']} | `{r['task']}` | {r['seed']} | {r['outcome']}{'/' + r['cap'] if r['cap'] else ''} | "
                       f"{r['final_status']} | {r['reward']} | {note or '–'} |")
    else:
        out.append("None.")
    if any(not_run.values()):
        out += ["", "Not run: " + "; ".join(f"{a} {len(v)} ({v[0]['not_run']})" for a, v in not_run.items() if v)]
    if man["tasks_excluded"]:
        out += ["", "## Excluded tasks (mechanical criteria)", "", "| Task | Reason |", "|---|---|"]
        out += [f"| `{t}` | {why} |" for t, why in sorted(man["tasks_excluded"].items())]
    g = man["git"]
    out += [
        "",
        "## Manifest",
        "",
        "| Item | Value |",
        "|---|---|",
        f"| Run | `{man['run_id']}`, started {man['started']} |",
        f"| ultra-instinct | `{g['ultra_instinct_head'][:12]}`{' (dirty)' if g['dirty'] else ''}; `{man['aui_binary']['path']}` "
        f"sha256 `{man['aui_binary']['sha256_16']}`, features `{man['aui_binary']['features']}` |",
        f"| jev-ultrafast | `{g['jev_ultrafast'][:12]}` (vendor, unmodified; bench monkeypatches only) |",
        f"| miniwob-plusplus | `{g['miniwob_plusplus']}` ({g['miniwob_repo']}) |",
        f"| Chrome | {man['chrome']}, headless, fresh profile per attempt, {man['viewport']['width']}×{man['viewport']['height']} |",
        f"| Python | browser-harness {man['python']['browser_harness']}, anthropic {man['python']['anthropic']} |",
        f"| Models | JEV `{man['models']['jev']}` (TypeSafe, counting proxy); text `{man['models']['text']}` -> "
        f"`{man['models']['text_upstream']}` on Anthropic Messages via `bench/text_proxy.py`, effort {man['models']['text_effort']} |",
        f"| Caps per attempt | {man['caps']['steps']} steps, {man['caps']['model_calls']} model calls (JEV + text), "
        f"{man['caps']['wall_s']} s wall (= episode time limit), stuck after {man['caps']['stuck_repeats']} identical no-DOM-change actions |",
        f"| Seeds | run seed {man['seed']} -> {man['seeds']} |",
        f"| Config hash | `{man['config_hash']}` |",
        "",
    ]
    compact = {"manifest": {k: v for k, v in man.items() if k != "order"}, "rows": rows}
    return "\n".join(out), compact


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("run_id")
    ap.add_argument("--publish", action="store_true", help="also write bench/results/MINIWOB-<run-id>.{md,json}")
    args = ap.parse_args()
    md, compact = build(args.run_id)
    path = RUNS / args.run_id / "REPORT.md"
    path.write_text(md)
    print(f"wrote {path}")
    if args.publish:
        dst = BENCH / "results" / f"MINIWOB-{args.run_id}.md"
        shutil.copyfile(path, dst)
        dst.with_suffix(".json").write_text(json.dumps(compact, indent=1))
        print(f"wrote {dst} and {dst.with_suffix('.json')}")


if __name__ == "__main__":
    main()
