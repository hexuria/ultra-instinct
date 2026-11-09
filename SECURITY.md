# Security policy

## Reporting a vulnerability

Please report security issues privately. Do not open a public issue, discussion or pull request
for a vulnerability.

1. **Preferred: GitHub private vulnerability reporting.**
   Open a private report at <https://github.com/hexuria/ultra-instinct/security/advisories/new>
   (repository **Security** tab → **Report a vulnerability**). The report stays private between you
   and the maintainer until a fix is published.
2. **Email:** codeitlikemiley@gmail.com (Uriah Galang, maintainer), if you cannot use GitHub.

Please include:

- the affected commit (or "latest `main`") or crate version,
- steps or a proof of concept to reproduce,
- the impact you observed or expect.

You should get an acknowledgement within 7 days. Please give us a reasonable time to fix the issue
before any public disclosure.

## Supported versions

| Version | Supported |
| --- | --- |
| latest commit on `main` | yes |
| latest `0.1.x` release of the `ultra-instinct-*` crates on crates.io | yes |
| anything older | no |
| `ultra-instinct-resonance` and `ultra-instinct-hyper` on crates.io | no: removed from the tree and no longer published |

Fixes land on `main` and ship in the next crates.io release; there are no maintained release
branches. The public API is unstable until 1.0.

## What counts as a vulnerability

Ultra-Instinct drives a real browser for an agent, so these are in scope:

- **A press without a valid ticket.** Any page input that skips the hard gate or the one-shot
  `ActionTicket` (`execute_ticketed` / `consume_ticket_once`), a ticket used twice, or a stale
  ticket pressing a different element than the one it was issued for.
- **A guard policy bypass.** A navigate, press, key or form submit that gets past
  `--allow-domain`, `--confirm-press` or `--confirm` (or their `ULTRA_INSTINCT_*` environment
  variables, or a host's `Server::set_policy`).
- **A snapshot-bound press on the wrong element.** An index press given `snapshot` that presses
  anything other than the element that snapshot showed.
- **An embedding host's limits bypassed.** A tool off `with_tool_allowlist` that runs, a
  relative file path accepted, or a file read or written outside `with_uploads` /
  `with_downloads`.
- **Secret leaks.** Password, hidden or file input values read from the page; text typed into a
  sensitive field showing up in any later tool output, error, HAR file or capture (including its
  percent-encoded or HTML-escaped forms); cookie, storage or header values shown unmasked
  without `reveal`; secrets in diaries or logs; state files written readable by other users.
- **Local access by another user.** Another local user reaching an `aui` session socket
  (`$TMPDIR/aui/`, directory 0700, socket 0600) or a saved state file.
- **Model replies widening what runs.** A remote or text-model reply that leads to an action, a
  selector, a coordinate or script text that was not on the offered menu, or text typed into a
  secret field.
- **Untrusted input crashes or hangs.** The fixture grammar, CDP replay scripts, CDP pipe
  frames, MCP stdin lines, remote and model-text replies, and state files.

Out of scope: what a site you chose to visit does inside your browser, bugs in Chrome itself, and
pointing Ultra-Instinct at a CDP endpoint you do not control (anyone with that endpoint already
controls the browser).

## Automated scanning

Reports from automated scanners (for example Anthropic's OSS Scanner) go to the same address above.
