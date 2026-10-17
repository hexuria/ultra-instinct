#!/usr/bin/env bash
# Repository rules that are cheaper to check with a script than with lints:
#   1. rust-toolchain.toml pins 1.99.0 (AGENTS.md) and the workspace rust-version is <= 1.99;
#   2. every crate root carries #![forbid(unsafe_code)] (belt and braces next to the workspace lint);
#   3. every crate stays under its line ceiling (scripts/crate-ceilings.txt);
#   4. workflows never install a different stable toolchain (nightly only with a pinned date).
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0

channel=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
if [[ "$channel" != "1.99.0" ]]; then
  echo "toolchain: rust-toolchain.toml channel is '$channel', owner rule requires 1.99.0"; fail=1
fi
rv=$(sed -n 's/^rust-version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
if [[ -z "$rv" ]] || ! printf '%s\n%s\n' "$rv" "1.99" | sort -V -C; then
  echo "toolchain: workspace rust-version '$rv' must be set and <= 1.99"; fail=1
fi

default_ceiling=$(awk '$1=="default"{print $2}' scripts/crate-ceilings.txt)
for manifest in crates/*/Cargo.toml experiments/*/*/Cargo.toml; do
  dir=$(dirname "$manifest"); name=$(basename "$dir")
  root="$dir/src/lib.rs"
  if [[ -f "$root" ]] && ! grep -q '^#!\[forbid(unsafe_code)\]' "$root"; then
    echo "unsafe: $root lacks #![forbid(unsafe_code)]"; fail=1
  fi
  ceiling=$(awk -v n="$name" '$1==n{print $2}' scripts/crate-ceilings.txt)
  ceiling=${ceiling:-$default_ceiling}
  lines=$(find "$dir/src" -name '*.rs' -print0 | xargs -0 cat | wc -l)
  if (( lines > ceiling )); then
    echo "ceiling: $name has $lines src lines (> $ceiling)"; fail=1
  fi
done

# Only pinned nightlies (nightly-YYYY-MM-DD) may appear besides the rust-toolchain.toml pin.
if grep -rnE 'toolchain: *(stable|beta|nightly$|nightly *$|1\.[0-9]+)' .github/workflows; then
  echo "workflows: unpinned or non-1.99.0 toolchain found above"; fail=1
fi
if grep -rnE '\+nightly( |$)' .github/workflows scripts | grep -v 'NIGHTLY'; then
  echo "workflows: use \$NIGHTLY (a dated nightly), not bare +nightly"; fail=1
fi

# Product code stays benchmark-neutral: no benchmark names, task names, or
# page classes under crates/ (fixes must be justified by general web behavior).
if grep -rniE 'miniwob|mind2web|webarena|\balink\b|subbtn' crates --include='*.rs' --include='*.toml' --include='*.json'; then
  echo "benchmark-neutral: benchmark-specific names found in crates/ above"; fail=1
fi

if (( fail )); then exit 1; fi
echo "repo rules: ok"
