#!/bin/sh
# Download the octoflow-gate release that <pin> names from its GitHub release, check it against the pinned sha256,
# and unpack it into <dir>, made if missing: <dir>/gate and the rest that octoflow-gate's README, "Running the gate in
# a project", lists, and print the release's version. Exits 0 when <dir> holds the release; exits non-zero, saying why
# on standard error, when <pin> cannot be read or has no `<version> <sha256>` line, the download fails, or the tarball
# does not match the sha256, in which case nothing is unpacked. What <dir> held before is overwritten where the
# release has the same file and otherwise left in place.
#
#   ci/fetch-gate.sh <pin> <dir>
#
# <pin> is a file like ci/gate.pin: lines starting with # are comments, and the first other line is the release's
# version and the sha256 of its tarball. The Jenkinsfile's Gate stage fetches the release that judges this
# repository's changes with it, by the steps that README section gives a project, as install-tools.sh's fetch_tool
# fetches a tool. Needs curl, sha256sum and tar. A copy of octoflow-gate's ci/fetch-gate.sh, as
# xtask/tests/fetch_gate.rs is of its test: a fix to either goes to both repositories.
set -eu

[ $# -eq 2 ] || { echo "usage: fetch-gate.sh <pin> <dir>" >&2; exit 2; }
pin=$1
dir=$2

[ -r "$pin" ] || { echo "fetch-gate.sh: cannot read the pin $pin" >&2; exit 1; }
line=$(grep -v '^#' "$pin" | head -n 1)
version=${line%% *}
sha256=${line#* }
# The version goes into the download URL, so it may hold only what a version does.
case $version in '' | *[!0-9A-Za-z.+-]*) version= ;; esac
case $sha256 in *[!0-9a-f]*) sha256= ;; esac
if [ -z "$version" ] || [ ${#sha256} -ne 64 ]; then
    echo "fetch-gate.sh: $pin's first line '$line' is not '<version> <sha256>'" >&2
    exit 1
fi

tarball=$(mktemp)
trap 'rm -f "$tarball"' EXIT
url=https://github.com/DavidSiegl/octoflow-gate/releases/download/v$version/octoflow-gate-$version.tar.gz
curl -fsSL -o "$tarball" "$url" || { echo "fetch-gate.sh: cannot download $url" >&2; exit 1; }
echo "$sha256  $tarball" | sha256sum -c --quiet - || {
    echo "fetch-gate.sh: $url does not match the sha256 $pin pins" >&2
    exit 1
}
mkdir -p "$dir"
tar -xzf "$tarball" -C "$dir"
echo "$version"
