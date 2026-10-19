#!/usr/bin/env bash
# Point the release URLs in docs/src/ to the current `cheadergen_cli` version,
# i.e. what `check_docs_version.sh` expects. Run on release PRs by
# `.github/workflows/release-pr.yml`, after release-plz bumps the version.
#
# Usage:
#   scripts/bump_docs_version.sh           # auto-detect version from Cargo.toml
#   scripts/bump_docs_version.sh 0.3.0     # override the version

set -euo pipefail

# The cheadergen project root (one level above this script), not the git root.
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
docs_dir="${repo_root}/docs/src"

if [[ $# -ge 1 ]]; then
    version="$1"
else
    version=$(cargo pkgid -p cheadergen_cli --manifest-path "${repo_root}/cheadergen_cli/Cargo.toml" \
        | awk -F'#' '{print $2}')
fi

if [[ ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: unexpected version '${version}'" >&2
    exit 2
fi

# The same URLs `check_docs_version.sh` checks: officina's `cheadergen_cli-vX.Y.Z`
# tags, and the archived cheadergen repository's `X.Y.Z` tags (moved to officina's).
{ grep -rlE '(officina/releases/(download|tag)/cheadergen_cli-v|cheadergen/releases/(download|tag)/)[0-9]' "${docs_dir}" || true; } \
    | while read -r file; do
        updated=$(sed -E \
            -e "s#(officina/releases/(download|tag)/cheadergen_cli-v)[0-9]+\.[0-9]+\.[0-9]+#\1${version}#g" \
            -e "s#cheadergen/releases/(download|tag)/[0-9]+\.[0-9]+\.[0-9]+#officina/releases/\1/cheadergen_cli-v${version}#g" \
            "${file}")
        if [[ "${updated}" != "$(cat "${file}")" ]]; then
            printf '%s\n' "${updated}" > "${file}"
            echo "Updated ${file#"${repo_root}/"}"
        fi
    done
