#!/bin/sh
# Check, from the repository's top level, that every `uses:` of a GitHub workflow (.github/workflows/*.yml, *.yaml) or
# a local action (an action.yml or action.yaml under .github) names its action by a commit sha, 40 lower-case hex
# digits after the @, and write the verdict to target/actions-pinned/junit.xml as a JUnit report of one test,
# actions-pinned.by-sha, which fails naming the file:line of each `uses:` that does not. A local action (./...) and a
# docker:// image pinned by digest (@sha256: and 64 hex digits) pass too: neither can be moved under the name the
# workflow gives. A tag, a branch, a short sha or no ref at all fails. Exits 0 whether or not one fails, the gate
# judging the report, as ci/rustfmt.sh does; exits non-zero, with no report, when the files could not be read.
#
#   ci/actions-pinned.sh
#
# The Jenkinsfile's Test stage runs it, and so does gate.toml's [[test]]: todo/65 pinned every action by sha, which
# .github/dependabot.yml keeps a sha when it raises one, but the gate's actionlint accepts any ref, and a tag can be
# moved to code nobody reviewed, run in a job that holds a token.
#
# It reads a `uses:` as YAML's block style writes it, one to a line, as a key or as a list item's first key, its value
# quoted or not and followed or not by a comment; a `uses:` in flow style ({uses: ...}) is not read.
set -eu

# -P: as ci/rustfmt.sh does, so that the files are named from the top level.
cd -P "$(dirname "$0")/.."
out=target/actions-pinned
mkdir -p "$out"
rm -f "$out/junit.xml"

# One line per unpinned `uses:`, "file:line: uses: value", in file and line order.
: > "$out/unpinned"
if [ -d .github ]; then
    find .github -type f \( \( -path '.github/workflows/*' \( -name '*.yml' -o -name '*.yaml' \) \) \
        -o -name action.yml -o -name action.yaml \) \
        -exec awk '
            # hex(s, n): whether s is exactly n lower-case hex digits; mawk has no {n} in its regular expressions.
            function hex(s, n) { return length(s) == n && s ~ /^[0-9a-f]+$/ }
            /^[ \t]*(-[ \t]+)?["\047]?uses["\047]?[ \t]*:/ {
                ref = $0
                sub(/^[ \t]*(-[ \t]+)?["\047]?uses["\047]?[ \t]*:[ \t]*/, "", ref)
                sub(/[ \t]+#.*$/, "", ref)
                sub(/[ \t]+$/, "", ref)
                if (ref ~ /^".*"$/ || ref ~ /^\047.*\047$/) ref = substr(ref, 2, length(ref) - 2)
                if (ref ~ /^\.\//) next
                if (ref ~ /^docker:\/\//) {
                    at = index(ref, "@sha256:")
                    if (at > 0 && hex(substr(ref, at + 8), 64)) next
                } else {
                    at = index(ref, "@")
                    if (at > 0 && hex(substr(ref, at + 1), 40)) next
                }
                print FILENAME ":" FNR ": uses: " ref
            }
        ' {} + > "$out/unsorted"
    # Sorted by file, then numerically by line: find lists the files in no set order.
    sort -t: -k1,1 -k2,2n "$out/unsorted" > "$out/unpinned"
fi

if [ -s "$out/unpinned" ]; then
    count=$(wc -l < "$out/unpinned" | tr -d ' ')
    echo "actions-pinned.sh: $count uses: not pinned by a commit sha:" >&2
    cat "$out/unpinned" >&2
    # Escaped for XML: & first, as the others' escapes hold one.
    escaped=$(sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's/"/\&quot;/g' "$out/unpinned")
    message="$count uses: not pinned by a commit sha: $(printf '%s\n' "$escaped" | sed 's/: .*//' | paste -sd ' ' -)"
    failure="<failure message=\"$message\">$escaped
Pin each by the commit sha of its release, with the tag as a comment: uses: owner/action@&lt;40 hex digits&gt; # v1.2.3</failure>"
else
    failure=
fi
testcase="<testcase classname=\"actions-pinned\" name=\"by-sha\">$failure</testcase>"
printf '<testsuites><testsuite name="actions-pinned" tests="1">%s</testsuite></testsuites>\n' "$testcase" > "$out/junit.xml"
