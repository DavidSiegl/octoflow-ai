#!/bin/sh
# Print the release version: engine/Cargo.toml's [package] version, as found in the tree this script is in, or, when
# REV is given, at the git revision REV of the repository the current directory is in. A merge into main is released
# as v<version> (Jenkinsfile, Release stage). Exits 0 having printed it; exits 1, saying why on standard error, when
# engine/Cargo.toml cannot be read there or its [package] table has no `version = "..."` line; exits 2 on a usage
# error. Without REV it needs no git, so it works in a source archive too.
#
#   ci/release-version.sh [REV]
#
# Its readers: the Jenkinsfile's Release stage (the release's tag), ci/version-raised.sh (HEAD's and the target's)
# and packaging/build-desktop.sh (the app's version).
set -eu

[ $# -le 1 ] || { echo "usage: release-version.sh [REV]" >&2; exit 2; }
manifest=engine/Cargo.toml

if [ $# -eq 1 ]; then
    where="$1's $manifest"
    text=$(git show "$1:$manifest") || { echo "release-version.sh: cannot read $where" >&2; exit 1; }
else
    where="$(dirname "$0")/../$manifest"
    text=$(cat "$where") || { echo "release-version.sh: cannot read $where" >&2; exit 1; }
fi
# The first version line of the [package] table, up to the next table: a dependency's table has versions of its own.
# Spaces around `=`, a trailing comment and CRLF line ends are TOML that cargo reads, so they are read here too.
version=$(printf '%s\n' "$text" | tr -d '\r' |
    sed -n '/^[[:space:]]*\[package\][[:space:]]*\(#.*\)\{0,1\}$/,/^[[:space:]]*\[/{
        s/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\([^"]*\)"[[:space:]]*\(#.*\)\{0,1\}$/\1/p
    }' | head -n 1)
[ -n "$version" ] || { echo "release-version.sh: no [package] version in $where" >&2; exit 1; }
echo "$version"
