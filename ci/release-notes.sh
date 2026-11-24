#!/bin/sh
# Print the notes of release v$VERSION from the docs/releases/ under the current directory: the notes of every version
# from just above $LATEST, the latest version already released, up to $VERSION, newest first, so that a version main
# reached but never released, by a failed main build or by stacked pull requests merged in one, still has its notes on
# a release page. Each is docs/releases/v<version>.md, and a version without one is left out. When only $VERSION's
# own file is among them, it is printed as it is; when several are, each comes under a `# v<version>` heading of its
# own. Prints nothing when none of them has a file. An empty $LATEST, no release yet, takes $VERSION's file alone.
# Versions are compared as sort -V orders them, as ci/version-raised.sh does. Exits 0 having printed the notes; exits
# 1, saying why on standard error, when a notes file cannot be read; exits 2 on a usage error.
#
#   ci/release-notes.sh VERSION LATEST
#
# The Jenkinsfile's Release stage runs it from the repository's top level, giving GitHub what it prints as the
# release's own words, which it puts before the notes it generates from the pull requests. A skipped version gets no
# tag of its own: its commits were never built as a release, so there are no assets to give it.
set -eu

[ $# -eq 2 ] || { echo "usage: release-notes.sh VERSION LATEST" >&2; exit 2; }
version=$1
latest=$2

# Whether version $1 is greater than version $2.
greater() {
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

# Whether version $1 is one to take the notes of: $VERSION itself, or one between $LATEST and it.
taken() {
    [ "$1" = "$version" ] || { [ -n "$latest" ] && greater "$version" "$1" && greater "$1" "$latest"; }
}

# Those with a notes file, newest first.
versions=$(
    for f in docs/releases/v*.md; do
        [ -f "$f" ] || continue
        v=${f#docs/releases/v}
        v=${v%.md}
        ! taken "$v" || echo "$v"
    done | sort -rV
)

[ -n "$versions" ] || exit 0
if [ "$versions" = "$version" ]; then
    cat "docs/releases/v$version.md" || { echo "release-notes.sh: cannot read docs/releases/v$version.md" >&2; exit 1; }
    exit 0
fi
first=true
for v in $versions; do
    $first || echo
    first=false
    printf '# v%s\n\n' "$v"
    cat "docs/releases/v$v.md" || { echo "release-notes.sh: cannot read docs/releases/v$v.md" >&2; exit 1; }
done
