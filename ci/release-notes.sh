#!/bin/sh
# Print the notes of release v$VERSION from the docs/releases/ under the current directory: the notes of every version
# above the highest version released yet, up to $VERSION, newest first, so that a version main reached but never
# released, by a failed main build or by stacked pull requests merged in one, still has its notes on a release page.
# The releases there are come on standard input, one tag a line; only a plain version tag, such as v0.9.0, counts, so
# that a prerelease's or another tag does not sort above the real ones, and none at all means no release yet, when
# $VERSION's notes are taken alone. A version's notes are docs/releases/v<version>.md, and a version without that file
# is left out. When only $VERSION's own file is taken, it is printed as it is, but for blank lines at its end; when
# several are, each comes under a `# v<version>` heading of its own, and a last heading, `# Pull requests`, stands over
# what GitHub appends. Prints nothing when none of them has a file, or only an empty one. Versions are compared as
# sort -V orders them, as ci/version-raised.sh does. Exits 0 having printed the notes; exits 1, saying why
# on standard error, when a notes file or standard input cannot be read or sort fails; exits 2 on a usage error.
#
#   ci/release-notes.sh VERSION < TAGS
#
# The Jenkinsfile's Release stage runs it from the repository's top level, giving GitHub what it prints as the
# release's own words, which it puts before the notes it generates from the pull requests. A skipped version gets no
# tag of its own: its commits were never built as a release, so there are no assets to give it. The highest release
# counts, not the latest made: two main builds may release out of order, and then the later one already carries the
# notes of the versions below it.
set -eu

[ $# -eq 1 ] || { echo "usage: release-notes.sh VERSION < TAGS" >&2; exit 2; }
version=$1

tags=$(cat) || { echo "release-notes.sh: cannot read the release tags from standard input" >&2; exit 1; }
released=$(printf '%s\n' "$tags" | sed -n 's/^v\([0-9][0-9]*\(\.[0-9][0-9]*\)*\)$/\1/p' | sort -V) ||
    { echo "release-notes.sh: cannot sort the released versions" >&2; exit 1; }
latest=$(printf '%s\n' "$released" | tail -n 1)

# Every version with notes, the highest released and $VERSION, oldest first, each once.
listed=$(
    for f in docs/releases/v*.md; do
        [ -f "$f" ] || continue
        v=${f#docs/releases/v}
        echo "${v%.md}"
    done
    [ -z "$latest" ] || echo "$latest"
    echo "$version"
)
sorted=$(printf '%s\n' "$listed" | sort -uV) || { echo "release-notes.sh: cannot sort the versions" >&2; exit 1; }

# Those with notes above the highest released, up to $VERSION, newest first; with no release yet, $VERSION alone.
versions=
above=false
for v in $sorted; do
    if [ "$v" = "$version" ] || $above; then
        [ ! -f "docs/releases/v$v.md" ] || versions="$v $versions"
    fi
    [ "$v" != "$version" ] || break
    [ "$v" != "$latest" ] || above=true
done

# The notes of version $1; exits 1 when they cannot be read. Each is read before anything is printed, so that a
# file that cannot be read prints no notes.
notes_of() {
    cat "docs/releases/v$1.md" || { echo "release-notes.sh: cannot read docs/releases/v$1.md" >&2; exit 1; }
}
case "$versions" in
"") ;;
"$version ")
    notes=$(notes_of "$version")
    [ -z "$notes" ] || printf '%s\n' "$notes"
    ;;
*)
    notes=$(
        for v in $versions; do
            printf '# v%s\n\n' "$v"
            notes_of "$v"
            echo
        done
        echo "# Pull requests"
    )
    printf '%s\n' "$notes"
    ;;
esac
