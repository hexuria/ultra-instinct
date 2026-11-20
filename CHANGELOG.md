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

### Shell verbs

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
