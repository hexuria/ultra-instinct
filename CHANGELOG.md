# Changelog

One `## <version>` section per release, newest first. The release workflow
(`.github/workflows/release.yml`) publishes the section for the tagged version
as the GitHub release body (`scripts/release-notes.sh`), so a tag with no
section here does not release. A prerelease tag (`v0.1.0-rc.1`) uses the
section of its version (`0.1.0`).

## 0.1.0

First release with prebuilt binaries.

### Install

- Release archives of `aui` + `ultra-instinct` for macOS (arm64, x64) and Linux (x64, arm64; glibc and musl), each with a `.sha256` and a combined `SHA256SUMS`.

### Breaking (library)

- The crates `ultra-instinct-geometry`, `ultra-instinct-observe` and `ultra-instinct-protocol` are now modules of `ultra-instinct-core`. Replace `ultra_instinct_geometry::X` with `ultra_instinct_core::geometry::X`, and the same for `observe` and `protocol`; the item names are unchanged.

- The shell verbs, the session daemon and `batch` are now the crate `ultra-instinct-verbs`. `ultra_instinct::verbs` and `ultra_instinct::CliError` are re-exported from it, so those paths still work.

### Policy

- `aui-policy.json` (or `AUI_POLICY_FILE`): allow or deny tools by name for a project; a denied tool answers `tool_not_allowed`.

### Shell verbs

- `--proxy scheme://host:port` on `aui open --launch` and `aui run --launch`: the launched Chrome sends its requests through that proxy.

- A `--launch` session keeps its login across `aui close` and reopen: the session daemon restores `<state dir>/<session>.json` (0600, the `browser_state_*` format) when it starts and saves it when it ends. `AUI_SESSION_STATE=0` turns it off; `AUI_STATE_DIR` moves the directory.

- `aui open --record <file>` records the session: a frame of the page after each verb (the `shot` screenshot, so secret fields are hidden) lands in `<file>.frames/`, and when the daemon ends `ffmpeg` assembles them into `<file>` when it is on `PATH`; without it the frames and `index.ffconcat` stand alone.

- `aui diff`: what changed on the page since the last `aui snap`.

- `--boundary` / `AUI_CONTENT_BOUNDARIES=1`: the reading verbs print page content between marker lines that carry a random id, so page text that imitates a tool result or an instruction stays visibly inside the page.

- Every `browser_*` MCP tool now has an `aui` verb or a recorded reason for having none, checked by a test.
- New verbs: `back`, `hover`, `drag`, `upload`, `text`, `read`, `html`, `find`, `pdf`, `tab new|switch|close`, `eval`, `console`, `requests`, `dblclick`, `rightclick`, `midclick`, `tripleclick`, `hold`, `scrollto`, `zoom`, `options`.
- `aui --version` prints the version.

### Runtime

- The agent loop: observe, a finite action space, Instinct or a remote decider, the hard gate, a one-shot `ActionTicket`, re-observe and verify.
- Stale snapshots refuse, presses can require `--confirm`, `--allow-domain` fences navigation and form targets, and secrets are masked in output.
- `aui mcp` serves the same tools over stdio for MCP clients.

### Verification

- CI builds and tests on Linux and macOS.
