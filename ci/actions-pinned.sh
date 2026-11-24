#!/bin/sh
# Check, from the repository's top level, that every `uses:` GitHub runs names its action by a commit sha, 40
# lower-case hex digits after the @, and write the verdict to target/actions-pinned/junit.xml as a JUnit report of one
# test, actions-pinned.by-sha, which fails naming the file:line of each `uses:` that does not. It reads the workflows
# (.github/workflows/*.yml, *.yaml, the top level only, as GitHub does) and every action.yml or action.yaml git tracks,
# wherever a workflow's ./ may name it; only tracked files, so that what a build or a tool puts in the checkout (the
# Jenkinsfile's CARGO_HOME, whose crates ship actions of their own) is not judged, and only those on disk. A local
# action (./...) and a docker:// image pinned by digest (@sha256: and 64 hex digits) pass too: neither can be moved
# under the name the workflow gives. A tag, a branch, a short sha or no ref at all fails. Exits 0 whether or not one
# fails, the gate judging the report, as ci/rustfmt.sh does; exits non-zero, with no report, when the files could not
# be read.
#
#   ci/actions-pinned.sh
#
# The Jenkinsfile's Test stage runs it, and so does gate.toml's [[test]]: todo/65 pinned every action by sha, which
# .github/dependabot.yml keeps a sha when it raises one, but the gate's actionlint accepts any ref, and a tag can be
# moved to code nobody reviewed, run in a job that holds a token.
#
# A `uses:` GitHub runs is a step's (a key of an item of a `steps:` list, in a workflow's job or a composite action's
# runs) or a job's (a key of a job under the top-level `jobs:`, calling a reusable workflow); a key named uses anywhere
# else, such as a with: input, and a line of a block or multi-line scalar (a run: | script) is not one. It reads only
# block style: a step, a steps: list, a job, jobs: or an action's runs: written in flow style ({...} or [...]) fails as
# not read, naming its line, since reading flow style by line is where a tag would slip through.
set -eu

# -P: as ci/rustfmt.sh does, so that the files are named from the top level.
cd -P "$(dirname "$0")/.."
out=target/actions-pinned
mkdir -p "$out"
rm -f "$out/junit.xml"

# The awk program that prints, for each file it is given, one line per unpinned `uses:`, "file:line: uses: value", and
# one per step or job in flow style, "file:line: <key> in flow style ...".
# Its $0 and $... are awk's, not the shell's.
# shellcheck disable=SC2016
unpinned='
    # hex(s, n): whether s is exactly n lower-case hex digits; mawk has no {n} in its regular expressions.
    function hex(s, n) { return length(s) == n && s ~ /^[0-9a-f]+$/ }

    # Report ref, the value of the uses: on this line, unless it is pinned.
    function judge(ref,    at) {
        sub(/[ \t]+#.*$/, "", ref)
        sub(/[ \t]+$/, "", ref)
        if (ref ~ /^".*"$/ || ref ~ /^\047.*\047$/) ref = substr(ref, 2, length(ref) - 2)
        if (ref ~ /^\.\//) return
        if (ref ~ /^docker:\/\//) {
            at = index(ref, "@sha256:")
            if (at > 0 && hex(substr(ref, at + 8), 64)) return
        } else {
            at = index(ref, "@")
            if (at > 0 && hex(substr(ref, at + 1), 40)) return
        }
        print FILENAME ":" FNR ": uses: " ref
    }

    # Whether a uses: key at this place is one GitHub runs: that of a step, the open key being steps, or that of a
    # job calling a reusable workflow, the open keys being jobs at the top level and the job.
    function runs() {
        return (depth >= 1 && key[depth] == "steps") || (depth == 2 && key[1] == "jobs" && col[1] == 0)
    }

    # Report the step or job, or the list or mapping of them, written in flow style on this line, as not read.
    function flow(what) {
        print FILENAME ":" FNR ": " what " in flow style, which this check does not read: write it in block style"
    }

    FNR == 1 { depth = 0; scalar = -1 }
    {
        line = $0
        sub(/\r$/, "", line)
        if (line ~ /^[ \t]*(#.*)?$/) next
        match(line, /^ */)
        at = RLENGTH
        # The lines of a scalar, block (|, >) or multi-line, sit deeper than the key it is the value of.
        if (scalar >= 0 && at > scalar) next
        scalar = -1
        text = substr(line, at + 1)
        # A list item: its content starts after "- ", a column deeper.
        while (text ~ /^-([ \t]|$)/) {
            match(text, /^-[ \t]*/)
            at += RLENGTH
            text = substr(text, RLENGTH + 1)
        }
        # Close the keys this line is not inside: those at its column or deeper.
        while (depth > 0 && col[depth] >= at) depth--
        if (text ~ /^[{[]/) {
            if (runs()) flow("a step")
            next
        }
        if (!match(text, /^("[^"]*"|\047[^\047]*\047|[^ \t#"\047:][^:]*)[ \t]*:([ \t]|$)/)) next
        k = substr(text, 1, RLENGTH)
        sub(/[ \t]*:[ \t]*$/, "", k)
        gsub(/^["\047]|["\047]$/, "", k)
        v = substr(text, RLENGTH + 1)
        sub(/^[ \t]+/, "", v)
        # A comment, after blanks or alone, is no value: jobs: # all jobs opens a mapping.
        sub(/(^|[ \t]+)#.*$/, "", v)
        if (k == "uses" && runs()) judge(v)
        # Where steps and jobs sit: the steps: of a job or of runs:, a job, and jobs: and runs: themselves.
        if (v ~ /^[{[]/) {
            if (k == "steps" && ((depth == 2 && key[1] == "jobs") || (depth == 1 && key[1] == "runs")))
                flow("steps:")
            else if (depth == 1 && key[1] == "jobs" && col[1] == 0)
                flow("job " k)
            else if (depth == 0 && (k == "jobs" || k == "runs"))
                flow(k ":")
        }
        depth++
        key[depth] = k
        col[depth] = at
        # A value on the line of the key itself, other than an anchor or tag, is a scalar or a flow collection, and the
        # deeper lines after it belong to it, not to keys of their own.
        if (v != "" && v !~ /^[&!][^ \t]*$/) scalar = at
    }
'
# The workflows GitHub reads, at the top level of .github/workflows, and every action a ./ can name. :(glob): a * that
# does not cross a slash. -r: no awk at all, which would read standard input, when git tracks none.
# Listed to a file first, not piped, so that set -e stops a git that fails (sh has no pipefail). Then those on disk
# only: a tracked file deleted, or left out of a sparse checkout, is no workflow this tree runs.
git ls-files -z -- ':(glob).github/workflows/*.yml' ':(glob).github/workflows/*.yaml' ':(glob)**/action.yml' \
    ':(glob)**/action.yaml' > "$out/tracked"
# shellcheck disable=SC2016
xargs -0 -r sh -c 'for f; do if [ -f "$f" ]; then printf "%s\0" "$f"; fi; done' sh < "$out/tracked" > "$out/files"
xargs -0 -r awk "$unpinned" < "$out/files" > "$out/unsorted"
# Sorted by file, then numerically by line, whatever order git lists the files in and xargs hands them to awk.
sort -t: -k1,1 -k2,2n "$out/unsorted" > "$out/unpinned"

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
