#!/bin/sh
# Print whether release v$VERSION may be GitHub's Latest: `false` when a release above $VERSION exists, `true`
# otherwise, also when there is no release yet. The releases there are come on standard input, one tag a line, and
# only a plain version tag counts (ci/release-tags.sh). Versions are compared as sort -V orders them, so 0.104.10 is
# above 0.104.9. Exits 0 having printed it; exits 1, saying why on standard error, when standard input cannot be read
# or sort fails; exits 2 on a usage error.
#
#   ci/release-latest.sh VERSION < TAGS
#
# The Jenkinsfile's Release stage passes `--latest=false` to `gh release create` when it prints `false`; GitHub marks
# a new release Latest by default. Main's builds run one at a time (todo/120), so its releases come in merge order,
# but a failed main build re-run after a later merge released makes a release below the highest, which must not move
# Latest back.
set -eu

[ $# -eq 1 ] || { echo "usage: release-latest.sh VERSION < TAGS" >&2; exit 2; }
version=$1
# shellcheck source=ci/release-tags.sh
. "$(dirname "$0")/release-tags.sh"

tags=$(cat) || { echo "release-latest.sh: cannot read the release tags from standard input" >&2; exit 1; }
released=$(printf '%s\n' "$tags" | released_versions) ||
    { echo "release-latest.sh: cannot sort the released versions" >&2; exit 1; }
# sort last, so that its own failure is the pipeline's.
sorted=$(printf '%s\n' "$released" "$version" | sed '/^$/d' | sort -V) ||
    { echo "release-latest.sh: cannot sort the versions" >&2; exit 1; }
highest=$(printf '%s\n' "$sorted" | tail -n 1)
if [ "$highest" = "$version" ]; then
    echo true
else
    echo false
fi
