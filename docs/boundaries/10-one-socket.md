# Anti-drift 10: One blocking CDP socket per agent

Rule 10 of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**One blocking CDP socket per agent.** Parallelism means separate agents
in separate tabs with no shared state. Formal tools stay unjustified for
this path (no `unsafe`, no atomics shared across threads, no recovery
protocol): Loom / Kani / TLA+ / Miri / Lean are **NOT JUSTIFIED**. The
owners are unit tests, `proptest`, adversarial fixtures, replay fixtures,
cargo-mutants (nightly), and cargo-fuzz on the untrusted-input parsers
(nightly). `aui daemon --session <name>` (a socket file on unix, a named
pipe on Windows; single thread) owns one MCP `Server`: one session is one
CDP socket; one connection and one line at a time, no thread pool. `aui
open --launch` makes the daemon own its Chrome for the session's life.
`aui open --attach` (needs `--allow-domain`; unix only for now) makes that
one socket the browser
websocket of the user's own Chrome (pages as flat sessions on it, no
reader thread); the daemon works in a tab it opens (a new window) and
sees only the tabs it opened (`SessionBackend` refuses any other
target); on close or idle exit it closes only those, never the user's
Chrome.
`Server::with_attach` never serves `ATTACH_DENIED` (cookies, state,
storage, download: profile-wide).
