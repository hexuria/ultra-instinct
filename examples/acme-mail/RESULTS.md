# acme-mail: results

## Mac remasure 2026-10-06 — A3 vs A8 (fair parity)

> Tip `acb3a33`. Seed 42, n=1, Luna up. Vendor pin `1231850a…`.
> Run id `20261006-081905`. The full writeup is in the private stow repo.

**Accuracy:** A8 **7/7**, A3 **5/7** (press-only both 5/5). A3 type failures
were browser_harness IPC timeouts. **Speed:** mixed (~26s vs ~34s pass mean;
A8 `tm-print` refuse ~103s skews mean).

Unfair precursor A3 vs A5 (`20261006-074743`): A3 4/7, A5 2/7 — A5 cannot type
(no Luna). Not parity.

Older leaderboard below is the merged historical pass
(`20261005-131001`+`141625`+`153648`) and is **stale** for live A3/A8 claims.

---

Merged runs: `20261005-131001`, `20261005-141625`, `20261005-153648`. Tasks: `examples/acme-mail/tasks.yaml`. Site: `examples/acme-mail/site/`.

**n = 1 run per arm per task** (up to 3 fresh tries, stopping at the first pass). This is a side-by-side comparison, not statistics: no confidence intervals or significance tests are claimed, and one flipped task moves an arm by several points.

## Leaderboard

| Rank | Arm | First-try accuracy | Pass within 3 | Press-only tasks, first try | Wrong actions (first try / all tries) | Safe refusals (target-missing) | cap_hit | stuck | crashed | Steps | Model calls | Tokens (first / all) | Wall first-try s | Wall total s | Wall p50 s (first try) | Untrusted clicks |
|---:|---|---|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | **A8** Luna + JEV + ultra-instinct + Browser Use | 100% (7/7) | 100% (7/7) | 100% (5/5) | 0 / 0 | 1/1 | 0 | 0 | 1 | 15 | 27 | 96,033 / 96,033 | 115.5 | 115.5 | 10.3 | 10 |
| 2 | **A1** Luna + Browser Use | 100% (7/7) | 100% (7/7) | 100% (5/5) | 0 / 0 | 1/1 | 0 | 0 | 0 | 13 | 25 | 229,217 / 229,217 | 221.6 | 221.6 | 29.6 | 0 |
| 3 | **A2** Luna + CUA driver | 100% (7/7) | 100% (7/7) | 100% (5/5) | 0 / 0 | 1/1 | 0 | 0 | 0 | 21 | 44 | 255,262 / 255,262 | 370.2 | 370.2 | 48.3 | 10 |
| 4 | **A4** CUA jev-use (generic task) | 86% (6/7) | 86% (6/7) | 80% (4/5) | 1 / 4 | 1/1 | 0 | 0 | 0 | 15 | 24 | 84,727 / 113,512 | 194.9 | 251.5 | 24.4 | 11 |
| 5 | **A7** Luna + JEV + ultra-instinct + CUA | 86% (6/7) | 100% (7/7) | 80% (4/5) | 0 / 0 | 1/1 | 0 | 0 | 0 | 13 | 26 | 70,890 / 76,558 | 199.3 | 229.7 | 23.9 | 9 |
| 6 | **A6** Luna + ultra-instinct (MCP) | 57% (4/7) | 71% (5/7) | 80% (4/5) | 0 / 0 | 1/1 | 0 | 0 | 0 | 21 | 88 | 280,094 / 599,675 | 169.9 | 352.2 | 22.9 | 9 |
| 7 | **A5** JEV + ultra-instinct (live_drive) | 43% (3/7) | 43% (3/7) | 60% (3/5) | 1 / 2 | 1/1 | 1 | 0 | 0 | 13 | 275 | 738,250 / 2,041,177 | 53.0 | 133.4 | 7.2 | 13 |
| 8 | **A3** jev-ultrafast (BU + JEV) | 43% (3/7) | 57% (4/7) | 60% (3/5) | 1 / 1 | 1/1 | 3 | 0 | 6 | 78 | 98 | 237,057 / 612,036 | 117.0 | 230.6 | 17.8 | 0 |

## Per task (attempts in order)

| Task | Class | Needs | A1 | A2 | A3 | A4 | A5 | A6 | A7 | A8 |
|---|---|---|---|---|---|---|---|---|---|---|
| `am-compose-send` | normal | type | PASS | PASS | fail(crashed) → fail(crashed) → fail(crashed) | PASS | fail → fail → fail | fail → fail → fail | PASS | PASS |
| `am-reply` | normal | type | PASS | PASS | fail(crashed) → fail(crashed) → fail(crashed) | PASS | fail → fail → fail | fail → fail → fail | PASS | PASS |
| `am-twin-send` | normal | press | PASS | PASS | fail(steps) → fail(steps) → fail(steps) | PASS | fail → fail(model_calls) → fail | PASS | PASS | PASS |
| `am-enabled-save` | normal | press | PASS | PASS | PASS | PASS | PASS | PASS | fail → PASS | PASS |
| `am-archive` | normal | press | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `am-star` | normal | press | PASS | PASS | fail → PASS | fail → fail → fail | fail → fail → fail | fail → fail → PASS | PASS | PASS |
| `am-tm-print` | target-missing | press | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |

## Detail

| Arm | Task | Tries | Outcomes (final status) | Wrong actions | Steps | Model calls | Tokens | Wall s | Unmet / forbidden (first try) |
|---|---|---:|---|---:|---:|---:|---:|---:|---|
| A1 | `am-compose-send` | 1 | finished (done) | 0 | 5 | 5 | 52,416 | 38.9 | – |
| A1 | `am-reply` | 1 | finished (done) | 0 | 3 | 5 | 48,391 | 43.21 | – |
| A1 | `am-twin-send` | 1 | finished (done) | 0 | 1 | 3 | 24,660 | 32.75 | – |
| A1 | `am-enabled-save` | 1 | finished (done) | 0 | 1 | 3 | 23,773 | 29.64 | – |
| A1 | `am-archive` | 1 | finished (done) | 0 | 1 | 3 | 25,588 | 25.5 | – |
| A1 | `am-star` | 1 | finished (done) | 0 | 1 | 3 | 27,087 | 23.96 | – |
| A1 | `am-tm-print` | 1 | finished (give_up) | 0 | 1 | 3 | 27,302 | 27.6 | – |
| A2 | `am-compose-send` | 1 | finished (done) | 0 | 6 | 12 | 90,625 | 92.31 | – |
| A2 | `am-reply` | 1 | finished (done) | 0 | 4 | 8 | 56,612 | 61.02 | – |
| A2 | `am-twin-send` | 1 | finished (done) | 0 | 2 | 4 | 11,702 | 42.23 | – |
| A2 | `am-enabled-save` | 1 | finished (done) | 0 | 2 | 5 | 6,543 | 41.05 | – |
| A2 | `am-archive` | 1 | finished (done) | 0 | 2 | 5 | 13,188 | 48.41 | – |
| A2 | `am-star` | 1 | finished (done) | 0 | 2 | 4 | 29,420 | 36.85 | – |
| A2 | `am-tm-print` | 1 | finished (give_up) | 0 | 3 | 6 | 47,172 | 48.34 | – |
| A3 | `am-compose-send` | 3 | crashed (error), crashed (error), crashed (error) | 0 | 3 | 9 | 60,627 | 57.93 | event {'type': 'send', 'source': 'compose', 'to': 'dana@acme.test', 'subject': 'Offsite budget', 'body': '~Draft numbers attached'} |
| A3 | `am-reply` | 3 | crashed (error), crashed (error), crashed (error) | 0 | 3 | 9 | 41,412 | 56.95 | event {'type': 'send', 'source': 'reply', 'thread': 'q3', 'body': '~On it, thanks'} |
| A3 | `am-twin-send` | 3 | cap_hit/steps (None), cap_hit/steps (None), cap_hit/steps (None) | 0 | 63 | 66 | 410,796 | 27.72 | – |
| A3 | `am-enabled-save` | 1 | finished (done) | 0 | 1 | 2 | 5,344 | 17.26 | – |
| A3 | `am-archive` | 1 | finished (done) | 0 | 2 | 3 | 18,338 | 17.82 | – |
| A3 | `am-star` | 2 | finished (done), finished (done) | 1 | 6 | 8 | 66,176 | 36.11 | event {'type': 'star', 'thread': 'sync', 'starred': True}; forbidden star {"thread": "retro", "starred": true} |
| A3 | `am-tm-print` | 1 | finished (give_up) | 0 | 0 | 1 | 9,343 | 16.85 | – |
| A4 | `am-compose-send` | 1 | finished (done) | 0 | 5 | 6 | 38,849 | 44.9 | – |
| A4 | `am-reply` | 1 | finished (done) | 0 | 3 | 4 | 12,718 | 34.67 | – |
| A4 | `am-twin-send` | 1 | finished (done) | 0 | 1 | 2 | 5,593 | 23.43 | – |
| A4 | `am-enabled-save` | 1 | finished (done) | 0 | 1 | 2 | 2,460 | 23.67 | – |
| A4 | `am-archive` | 1 | finished (done) | 0 | 1 | 2 | 7,898 | 24.4 | – |
| A4 | `am-star` | 3 | finished (done), finished (done), finished (done) | 4 | 4 | 7 | 40,273 | 82.17 | event {'type': 'star', 'thread': 'sync', 'starred': True}; forbidden star {"thread": "design", "starred": true} |
| A4 | `am-tm-print` | 1 | finished (give_up) | 0 | 0 | 1 | 5,721 | 18.3 | – |
| A5 | `am-compose-send` | 3 | finished (give_up), finished (give_up), finished (give_up) | 0 | 3 | 67 | 453,426 | 33.19 | event {'type': 'send', 'source': 'compose', 'to': 'dana@acme.test', 'subject': 'Offsite budget', 'body': '~Draft numbers attached'} |
| A5 | `am-reply` | 3 | finished (give_up), finished (give_up), finished (give_up) | 0 | 3 | 59 | 488,795 | 28.79 | event {'type': 'send', 'source': 'reply', 'thread': 'q3', 'body': '~On it, thanks'} |
| A5 | `am-twin-send` | 3 | finished (done), cap_hit/model_calls (None), finished (done) | 2 | 2 | 62 | 414,803 | 27.27 | event {'type': 'send', 'source': 'reply', 'thread': 'q3'}; forbidden send {"source": "compose", "to": "all@acme.test", "cc": "", "subject": "Draft: launch |
| A5 | `am-enabled-save` | 1 | finished (done) | 0 | 1 | 10 | 25,340 | 4.64 | – |
| A5 | `am-archive` | 1 | finished (done) | 0 | 1 | 12 | 93,093 | 7.23 | – |
| A5 | `am-star` | 3 | finished (give_up), finished (give_up), finished (give_up) | 0 | 3 | 59 | 483,701 | 28.01 | event {'type': 'star', 'thread': 'sync', 'starred': True} |
| A5 | `am-tm-print` | 1 | finished (give_up) | 0 | 0 | 6 | 82,019 | 4.32 | – |
| A6 | `am-compose-send` | 3 | finished (give_up), finished (give_up), finished (give_up) | 0 | 3 | 19 | 129,124 | 79.32 | event {'type': 'send', 'source': 'compose', 'to': 'dana@acme.test', 'subject': 'Offsite budget', 'body': '~Draft numbers attached'} |
| A6 | `am-reply` | 3 | finished (give_up), finished (done), finished (give_up) | 0 | 9 | 27 | 212,662 | 105.18 | event {'type': 'send', 'source': 'reply', 'thread': 'q3', 'body': '~On it, thanks'} |
| A6 | `am-twin-send` | 1 | finished (done) | 0 | 1 | 6 | 19,970 | 22.91 | – |
| A6 | `am-enabled-save` | 1 | finished (done) | 0 | 1 | 5 | 9,927 | 16.99 | – |
| A6 | `am-archive` | 1 | finished (done) | 0 | 1 | 4 | 14,106 | 15.14 | – |
| A6 | `am-star` | 3 | finished (give_up), finished (done), finished (done) | 0 | 6 | 24 | 202,446 | 101.04 | event {'type': 'star', 'thread': 'sync', 'starred': True} |
| A6 | `am-tm-print` | 1 | finished (give_up) | 0 | 0 | 3 | 11,440 | 11.66 | – |
| A7 | `am-compose-send` | 1 | finished (done) | 0 | 5 | 4 | 27,459 | 45.06 | – |
| A7 | `am-reply` | 1 | finished (done) | 0 | 3 | 5 | 13,766 | 36.88 | – |
| A7 | `am-twin-send` | 1 | finished (done) | 0 | 1 | 3 | 3,864 | 23.9 | – |
| A7 | `am-enabled-save` | 2 | finished (done), finished (done) | 0 | 1 | 6 | 7,653 | 53.02 | event {'type': 'save', 'section': 'signature'} |
| A7 | `am-archive` | 1 | finished (done) | 0 | 1 | 2 | 3,908 | 21.08 | – |
| A7 | `am-star` | 1 | finished (done) | 0 | 1 | 3 | 7,892 | 22.4 | – |
| A7 | `am-tm-print` | 1 | finished (give_up) | 0 | 1 | 3 | 12,016 | 27.36 | – |
| A8 | `am-compose-send` | 1 | finished (done) | 0 | 5 | 4 | 27,515 | 32.18 | – |
| A8 | `am-reply` | 1 | finished (done) | 0 | 3 | 5 | 13,775 | 20.6 | – |
| A8 | `am-twin-send` | 1 | finished (done) | 0 | 1 | 3 | 3,885 | 8.62 | – |
| A8 | `am-enabled-save` | 1 | finished (done) | 0 | 1 | 3 | 3,339 | 10.3 | – |
| A8 | `am-archive` | 1 | finished (done) | 0 | 1 | 2 | 3,901 | 8.5 | – |
| A8 | `am-star` | 1 | crashed (error) | 0 | 1 | 3 | 3,846 | 9.01 | – |
| A8 | `am-tm-print` | 1 | finished (give_up) | 0 | 3 | 7 | 39,772 | 26.28 | – |

## Manifest

| Item | Value |
|---|---|
| Report id | `20261005-131001+20261005-141625+20261005-153648` |
| Run `20261005-131001` started | 2026-10-05 13:10:04 +0800 |
| Run `20261005-131001` bench HEAD | `456b9c062a06` |
| Run `20261005-131001` ultra-instinct merged | `4ae30d3` |
| Run `20261005-131001` TYPESAFE_API_KEY present | False |
| Run `20261005-141625` started | 2026-10-05 14:16:28 +0800 |
| Run `20261005-141625` bench HEAD | `984ac2c6a515` |
| Run `20261005-141625` ultra-instinct merged | `4ae30d3` |
| Run `20261005-141625` TYPESAFE_API_KEY present | True |
| Run `20261005-153648` started | 2026-10-05 15:36:51 +0800 |
| Run `20261005-153648` bench HEAD | `570d00f0368f` (dirty) |
| Run `20261005-153648` ultra-instinct merged | `4ae30d3` |
| Run `20261005-153648` TYPESAFE_API_KEY present | True |
| jev-ultrafast pin | `1231850a0bf1` |
| cua (jev-use) pin | `9ccafc981412` |
| Chrome | Chrome/154.0.8037.98 (headless for harness-launched arms; CUA arms use a driver-launched headed window sized to the same viewport) |
| cua-driver | cua-driver 0.23.2 |
| browser-use | 0.13.10 |
| Models | Luna `gpt-6-luna` (reasoning effort low) via OpenCodex; JEV `jev-latest` |
| Caps per attempt | 20 steps, 40 model calls, 180 s wall, stuck after 3 identical no-change actions, 40 page acts backstop |
| Viewport | 1280×800 |
| Seed / reps / max tries | 42 / 1 / 3 |
| Config hash | `c7242d275cf8` |
