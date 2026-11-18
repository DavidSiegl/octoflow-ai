#!/bin/sh
# Check, from the repository's top level, that the workspace's Rust is as `cargo fmt --all` formats it (rustfmt.toml),
# and write the verdict to target/rustfmt/junit.xml as a JUnit report of one test, rustfmt.formatted, which fails
# naming each file `cargo fmt --all` would change. Exits 0 whether or not one would change, the gate judging the
# report, as ci/rust-tests.sh does; exits non-zero, with no report, when rustfmt could not check the code, such as a
# file that does not parse or a toolchain without rustfmt.
#
#   ci/rustfmt.sh
#
# The Jenkinsfile's Test stage runs it, and so does gate.toml's [[test]], so a gate run outside Jenkins, such as
# octoflow's `--quality-gate local`, lists an unformatted file among the failing tests as a Jenkins build does: the
# file the Builder of an octoflow run reads. A test of the workspace's own could not check this: the gate's
# surviving-mutants check runs the tests on code cargo-mutants rewrote, which rustfmt would reformat.
set -eu

cd "$(dirname "$0")/.."
out=target/rustfmt
mkdir -p "$out"
rm -f "$out/junit.xml"
# --check exits 1 both when a file would change, listing it on standard output (-l), and when rustfmt could not
# check, saying why on standard error; so what it wrote to standard error tells the two apart.
status=0
cargo fmt --all --check -- -l > "$out/files" 2> "$out/errors" || status=$?
if [ -s "$out/errors" ] || { [ "$status" -ne 0 ] && [ ! -s "$out/files" ]; }; then
    echo "rustfmt could not check the code (cargo fmt exited $status):" >&2
    cat "$out/errors" >&2
    exit 1
fi
if [ -s "$out/files" ]; then
    # The paths from the top level, escaped for XML: & first, as the others' escapes hold one.
    files=$(sed -e "s|^$PWD/||" -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's/"/\&quot;/g' "$out/files")
    count=$(printf '%s\n' "$files" | wc -l | tr -d ' ')
    message="cargo fmt --all would change $count file(s): $(printf '%s\n' "$files" | paste -sd ' ' -)"
    failure="<failure message=\"$message\">Run cargo fmt --all.</failure>"
    echo "rustfmt.sh: $message" >&2
else
    failure=
fi
printf '<testsuites><testsuite name="rustfmt" tests="1"><testcase classname="rustfmt" name="formatted">%s</testcase></testsuite></testsuites>\n' \
    "$failure" > "$out/junit.xml"
