#!/bin/sh
# Print the release version: engine/Cargo.toml's [package] version, in the working tree of the git checkout the
# current directory is in, or at its git revision REV when one is given. A merge into main is released as v<version>
# (Jenkinsfile, Release stage). Exits 0 having printed it; exits 1, saying why on standard error, when the current
# directory is in no checkout, engine/Cargo.toml cannot be read there, or its [package] table has no
# `version = "..."` line; exits 2 on a usage error.
#
#   ci/release-version.sh [REV]
#
# Its readers: the Jenkinsfile's Install stage (the release's tag), ci/version-raised.sh (HEAD's and the target's)
# and packaging/build-desktop.sh (the app's version).
set -eu

[ $# -le 1 ] || { echo "usage: release-version.sh [REV]" >&2; exit 2; }
top=$(git rev-parse --show-toplevel) || { echo "release-version.sh: not in a git checkout" >&2; exit 1; }
cd "$top"
manifest=engine/Cargo.toml

if [ $# -eq 1 ]; then
    where="$1's $manifest"
    text=$(git show "$1:$manifest") || { echo "release-version.sh: cannot read $where" >&2; exit 1; }
else
    where=$manifest
    text=$(cat "$manifest") || { echo "release-version.sh: cannot read $where" >&2; exit 1; }
fi
# The first version line of the [package] table, up to the next table: a dependency's table has versions of its own.
version=$(printf '%s\n' "$text" | sed -n '/^\[package\]$/,/^\[/s/^version = "\([^"]*\)"$/\1/p' | head -n 1)
[ -n "$version" ] || { echo "release-version.sh: no [package] version in $where" >&2; exit 1; }
echo "$version"
