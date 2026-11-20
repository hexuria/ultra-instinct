#!/usr/bin/env bash
# Print the CHANGELOG.md section for a release tag (plan P1.5): the lines
# under `## <version>` up to the next `## ` heading, without blank lines at
# either end. `v0.2.0-rc.1` reads the `0.2.0` section. Exits 1 when the
# section is missing or empty, so a tag without notes does not release.
#
#   scripts/release-notes.sh v0.1.0 [CHANGELOG.md]

set -euo pipefail

tag="${1:?usage: release-notes.sh <tag> [changelog]}"
changelog="${2:-CHANGELOG.md}"
version="${tag#v}"
version="${version%%-*}"

notes="$(awk -v heading="## $version" '
    $0 == heading { found = 1; next }
    found && /^## / { exit }
    found { print }
' "$changelog" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')"

if [ -z "$notes" ]; then
    echo "error: $changelog has no notes under \"## $version\" (tag $tag)" >&2
    exit 1
fi
printf '%s\n' "$notes"
