#!/bin/sh
# Install `aui` + `ultra-instinct` from a GitHub release (plan P1.2). No Rust
# toolchain needed:
#
#   curl -fsSL https://raw.githubusercontent.com/hexuria/ultra-instinct/main/scripts/install.sh | sh
#
# It downloads the archive for this machine, checks it against the release's
# SHA256SUMS, and copies both binaries to AUI_INSTALL_DIR (default
# ~/.local/bin). A checksum mismatch stops the install. When there is no
# release binary for this machine, or the download fails, it falls back to the
# source build of main (scripts/install-aui.sh, needs cargo) and copies the
# result to AUI_INSTALL_DIR. With AUI_VERSION set there is no fallback.
#
#   AUI_VERSION      release tag to install (default: the latest release)
#   AUI_INSTALL_DIR  where the binaries go (default: ~/.local/bin)
#   AUI_BASE_URL     release download root (default: the GitHub releases of
#                    hexuria/ultra-instinct; set by the tests)

set -eu

REPO="hexuria/ultra-instinct"
BASE="${AUI_BASE_URL:-https://github.com/$REPO/releases}"
RAW="https://raw.githubusercontent.com/$REPO/main"
DIR="${AUI_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
warn() { printf '%s\n' "$*" >&2; }

# Copy both binaries from $1 to $DIR. Copy beside the destination, then
# rename: a running `aui` keeps its file.
place() {
    mkdir -p "$DIR"
    for bin in aui ultra-instinct; do
        cp "$1/$bin" "$DIR/.$bin.new"
        chmod 755 "$DIR/.$bin.new"
        mv -f "$DIR/.$bin.new" "$DIR/$bin"
    done
    say "installed $("$DIR/aui" --version) to $DIR"
    case ":$PATH:" in
        *":$DIR:"*) ;;
        *) warn "warning: $DIR is not on PATH; add it to use aui" ;;
    esac
}

source_build() {
    if [ -n "${AUI_VERSION:-}" ]; then
        # The source build is the main branch, not the release asked for.
        warn "error: $1; AUI_VERSION=$AUI_VERSION was not installed"
        exit 1
    fi
    warn "$1; falling back to the source build"
    if ! command -v cargo >/dev/null 2>&1; then
        warn "error: no release binary was installed and cargo is not on PATH"
        exit 1
    fi
    here="$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)"
    if [ -n "$here" ] && [ -f "$here/install-aui.sh" ]; then
        builder="$here/install-aui.sh"
    else
        builder="$(mktemp)"
        if ! curl -fsSL -o "$builder" "$RAW/scripts/install-aui.sh"; then
            warn "error: could not download $RAW/scripts/install-aui.sh"
            exit 1
        fi
    fi
    # Build into a scratch root, whatever cargo's own install root is, so the
    # binaries copied to $DIR are the ones just built.
    stage="$(mktemp -d)"
    AUI_FORCE=1 CARGO_INSTALL_ROOT="$stage" bash "$builder"
    place "$stage/bin"
    rm -rf "$stage"
    exit 0
}

target() {
    case "$(uname -m)" in
        x86_64 | amd64) arch=x86_64 ;;
        arm64 | aarch64) arch=aarch64 ;;
        *) return 1 ;;
    esac
    case "$(uname -s)" in
        Darwin) say "$arch-apple-darwin" ;;
        Linux)
            # musl has no glibc loader; the musl build is static and runs on both.
            if ls /lib/ld-musl-* >/dev/null 2>&1; then
                say "$arch-unknown-linux-musl"
            else
                say "$arch-unknown-linux-gnu"
            fi
            ;;
        *) return 1 ;;
    esac
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

TARGET="$(target)" || source_build "no release binary for $(uname -s) $(uname -m)"
ARCHIVE="aui-$TARGET.tar.gz"
if [ -n "${AUI_VERSION:-}" ]; then
    URL="$BASE/download/$AUI_VERSION"
else
    URL="$BASE/latest/download"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

if ! curl -fsSL --proto '=https,file' -o "$TMP/$ARCHIVE" "$URL/$ARCHIVE" \
    || ! curl -fsSL --proto '=https,file' -o "$TMP/SHA256SUMS" "$URL/SHA256SUMS"; then
    source_build "could not download $URL/$ARCHIVE"
fi

want="$(awk -v name="$ARCHIVE" '$2 == name || $2 == "*" name { print $1 }' "$TMP/SHA256SUMS")"
got="$(sha256 "$TMP/$ARCHIVE")"
if [ -z "$want" ] || [ "$want" != "$got" ]; then
    warn "error: checksum mismatch for $ARCHIVE"
    warn "  expected: ${want:-<not listed in SHA256SUMS>}"
    warn "  got:      $got"
    warn "nothing was installed"
    exit 1
fi

tar -xzf "$TMP/$ARCHIVE" -C "$TMP"
place "$TMP/aui-$TARGET"
