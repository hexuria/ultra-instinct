#!/usr/bin/env bash
# Install the ultra-instinct CLI (`aui` + `ultra-instinct` binaries).
# Idempotent: exits early when `aui` is already on PATH (AUI_FORCE=1 to force).
#
# Sources, in order:
#   1. This repo, when the script runs from inside a checkout
#      (scripts/install-aui.sh lives one level below the root).
#   2. A cache clone of hexuria/ultra-instinct, updated via git pull,
#      when no checkout is present.
#
# Requires a Rust toolchain (rustup). Features installed: jev, clef,
# model-text — the full policy surface (instinct stays available offline).

set -euo pipefail

FEATURES="jev clef model-text"
REPO_URL="https://github.com/hexuria/ultra-instinct"
CACHE_SRC="${XDG_CACHE_HOME:-$HOME/.cache}/ultra-instinct"

if command -v aui >/dev/null 2>&1 && [ "${AUI_FORCE:-0}" != "1" ]; then
    echo "aui already installed: $(command -v aui)"
    echo "(if it lacks the jev/clef/model-text features, rerun with AUI_FORCE=1)"
    exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found — install Rust first:" >&2
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    exit 1
fi

# Locate the repo root from the script's own path; empty when piped/copied.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." 2>/dev/null && pwd || true)"
if [ -n "$HERE" ] && [ -f "$HERE/crates/ultra-instinct/Cargo.toml" ] \
    && grep -q 'name = "ultra-instinct"' "$HERE/crates/ultra-instinct/Cargo.toml"; then
    SRC="$HERE"
else
    SRC="$CACHE_SRC"
    if [ -d "$SRC/.git" ]; then
        git -C "$SRC" pull --ff-only
    else
        git clone "$REPO_URL" "$SRC"
    fi
fi

# cargo install resolves the rustup toolchain from the invocation cwd, not
# the source tree — cd into the source so its rust-toolchain.toml (1.99.0)
# applies even when the caller's default toolchain is older.
cd "$SRC"
cargo install --locked --path crates/ultra-instinct --features "$FEATURES"

BIN_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "warning: $BIN_DIR is not on PATH — add it to use aui" >&2 ;;
esac

echo "installed: $($BIN_DIR/aui --help 2>/dev/null | head -1 || echo 'aui')"
