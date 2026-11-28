# Sourced, not run: what counts as a release among a repository's tags, for ci/release-notes.sh and
# ci/release-latest.sh. Only a plain version tag, such as v0.9.0, does, so that a prerelease's or another tag does not
# sort above the real ones.

# released_versions: print the versions of the plain version tags on standard input, one tag a line, oldest first as
# sort -V orders them (as ci/version-raised.sh compares versions), without the `v`; nothing for none. Returns non-zero
# when sort fails.
released_versions() {
    sed -n 's/^v\([0-9][0-9]*\(\.[0-9][0-9]*\)*\)$/\1/p' | sort -V
}
