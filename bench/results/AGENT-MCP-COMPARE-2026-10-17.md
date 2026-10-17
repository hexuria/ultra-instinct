# Claude Code agents: ultra-instinct MCP vs obscura + pageshot (2026-10-17)

A small side-by-side of **browser tooling for an LLM agent**, not of the
`ultra-instinct run` agent loop. The same Claude Code subagent (same model,
same prompt, same rules) checked and drove one page with different browser
tools. Only the tooling section of the agent definition differed between arms.

**n = 2 runs per arm, one site, one task.** It is a side-by-side comparison,
not statistics.

## Setup

| | |
|---|---|
| Date / host | 2026-10-17, macOS (Apple silicon), Asia/Manila |
| Agent harness | Claude Code 2.1.302, custom subagents (`~/.claude/agents/*.md`) |
| Model | Claude Haiku 5.5 at max effort, every arm |
| Browser | Google Chrome 155.0.8059.84, headless |
| Site | Local dev build of a production marketing site: a Leptos/WASM single-page app on `http://127.0.0.1:59887/`. Theme comes from a `gc-theme` cookie (dark when absent); the hero swaps screenshots per theme and per tab |
| ultra-instinct | `aui mcp`, installed from `main`: `17dc819` before the screenshot fix, `f614e64` after it (#75) |

### Arms

| Arm | Browser tooling | Notes |
|---|---|---|
| **obscura + pageshot** (baseline) | obscura MCP (lightweight headless engine, `--allow-private-network`) for DOM reads and clicks; `pageshot` (Playwright-driven headless Chrome CLI) for screenshots | obscura does not paint images and runs partial JS; `pageshot` cannot click |
| **ultra-instinct** (before #75) | `aui mcp` `browser_*` tools on a private headless Chrome the agent starts | `browser_screenshot` returned base64 inside a text item, which the agent could neither view nor save |
| **ultra-instinct + obscura** (before #75) | both of the above | the agent used ultra-instinct only in both runs |
| **ultra-instinct** (after #75) | same as the first ultra-instinct arm | `browser_screenshot` returns an MCP image the agent can see; `path` saves it |

## Task (identical prompt for every run)

1. Report the page title and the main heading.
2. Report whether the hero screenshot renders with clean corners (no black wedges), starting in light theme.
3. Report the hero image file name.
4. Click the theme toggle in the header; report the active theme and the hero file name.
5. Click the "Guided Form & Templates" tab; report the hero file name.

Plus: save a screenshot per state, and list every tool used.

**Ground truth** (recorded with Playwright on the same build, just before
scoring): title "Buwiz Forms | The Modern Alternative to eBIRForms"; h1
"Philippine tax filing. Finally modern."; corners clean; light hero
`shell_side_by_side_light.webp`; after toggle → dark, `shell_side_by_side_dark.webp`;
after tab → `shell_templates_dark.webp`.

## Results

Tokens, tool calls, and wall time are the per-subagent totals Claude Code
reports when a subagent finishes (tokens include cached context).

| Arm | Run | Correct (of 5) | Tokens | Tool calls | Wall time |
|---|---|---|---|---|---|
| obscura + pageshot | 1 | 5 | 152,138 | 42 | 540.8 s |
| obscura + pageshot | 2 | 5 | 135,267 | 41 | 442.8 s |
| ultra-instinct, before #75 | 1 | 5 | 104,797 | 18 | 250.1 s |
| ultra-instinct, before #75 | 2 | 5 | 108,591 | 31 | 266.3 s |
| ultra-instinct + obscura, before #75 | 1 | 5 | 101,576 | 23 | 239.1 s |
| ultra-instinct + obscura, before #75 | 2 | 5 | 120,890 | 25 | 310.8 s |
| **ultra-instinct, after #75** | A | **5** | **106,129** | 31 | **242.7 s** |
| **ultra-instinct, after #75** | B | **5** | **93,787** | 36 | **200.2 s** |

Means:

| Arm | Correct | Tokens | Tool calls | Wall time |
|---|---|---|---|---|
| obscura + pageshot | 10 / 10 | 143,702 | 41.5 | 8.2 min |
| ultra-instinct, before #75 | 10 / 10 | 106,694 | 24.5 | 4.3 min |
| ultra-instinct + obscura, before #75 | 10 / 10 | 111,233 | 24.0 | 4.6 min |
| **ultra-instinct, after #75** | **10 / 10** | **99,958** | 33.5 | **3.7 min** |

**ultra-instinct (after #75) vs obscura + pageshot:** same accuracy, **30%
fewer tokens**, **2.2× faster**.

### Evidence quality

Accuracy tied, so how each arm *knew* the answer matters:

- **obscura + pageshot:** real screenshots, but only of the starting state;
  `pageshot` cannot click. Clicks ran in obscura, which loaded dark despite the
  light cookie in both runs, so the agent toggled an extra time to get back to
  light; post-click results were read from obscura's DOM, never rendered.
- **ultra-instinct, before #75:** clicks in real Chrome (the cookie and the
  page's JS behave), but no viewable screenshot; corner checks came from
  reading image pixels in the page (canvas), not from a render.
- **ultra-instinct, after #75:** clicks in real Chrome **and** a screenshot of
  every state, including after each click, viewed by the agent and saved with
  `path` (the agents also zoomed corner crops and scrolled for the bottom
  corners).

## Honesty notes

- n = 2 per arm, one site, one five-part task, one model. Differences of a
  few thousand tokens between runs of the same arm are noise.
- This compares browser **tooling for an LLM agent**. Every arm's agent chose
  clicks itself through index-based `browser_click` (or obscura's click); no
  run used `aui run`, Instinct, JEV, or Clef. It says nothing about the
  `ultra-instinct run` loop.
- The baseline's weakest point (obscura ignoring the theme cookie) is partly
  site-specific: a page whose state does not depend on JS-applied cookies would
  hurt obscura less.
- Excluded: one ultra-instinct rerun (147,495 tokens, 44 calls, 405.9 s, 5/5)
  that ran against a stale `aui mcp` server still on the pre-#75 binary (Claude
  Code reuses a subagent's MCP server within a session). It had to decode
  screenshots outside the tool, so it measures the workaround, not either build.
- The site is a private local build; the agent definitions lived in the
  operator's `~/.claude/agents/`. The ultra-instinct arm is reproducible from
  this repo: give a Claude Code subagent `mcpServers: [{ultra-instinct: {command:
  aui, args: [mcp]}}]`, have it start a private headless Chrome on a free port,
  and pass `"cdp"` on every `browser_*` call.
