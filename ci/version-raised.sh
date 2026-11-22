#!/bin/sh
# Exit 0 when the change of pull request $CHANGE_ID into $CHANGE_TARGET, a pull request build's HEAD, touches only
# docs, tests, CI or repository tooling (NOT_RELEASED below), saying so on standard output; or when it raises the
# release version, as ci/release-version.sh reads it, over $CHANGE_TARGET's, fetched from origin first, and no older
# pull request in $OPEN_PULL_REQUESTS raises it too. Exit non-zero, saying why on standard error, when the version is
# not raised, when a version cannot be read, when an older pull request (a lower number) changes released paths and
# its head's version is greater than $CHANGE_TARGET's, naming the oldest such, or when its head cannot be fetched.
# Versions are compared as sort -V orders them, number by number, so 0.10.0 is greater than 0.9.0.
#
# OPEN_PULL_REQUESTS is a file of the open pull requests into $CHANGE_TARGET, drafts included, one number a line,
# listed before this script fetches $CHANGE_TARGET, so that one missing from it has merged into what is fetched.
#
# Jenkins's Version stage runs it on a pull request build into main: the Release stage releases a merge into main
# only when its version has no release yet, so a merge that kept main's version would go unreleased, which is what a
# change to exempt paths alone should do; and two open pull requests that raise it would both claim the next
# version. The oldest goes first; a younger one is built again by its owner once it has merged (CLAUDE.md,
# Releases). An older one that has not raised yet does not count: once it does, its own Version stage compares it
# with the target this one leaves.
set -eu
: "${CHANGE_TARGET:?is unset; only a pull request build has a target}"
: "${OPEN_PULL_REQUESTS:?is unset; it names the file of the open pull requests into the target}"
case ${CHANGE_ID:-} in
    '' | *[!0-9]*)
        echo "version-raised.sh: CHANGE_ID, the pull request's number, is '${CHANGE_ID:-}', not a number" >&2
        exit 1
        ;;
esac
git fetch --quiet origin "+refs/heads/$CHANGE_TARGET:refs/remotes/origin/$CHANGE_TARGET"

# The paths whose changes reach no user, as git pathspecs: docs, tests, CI and repository tooling. A change to any
# other path, or a diff that fails, needs the version raised, so a path left off errs towards a raise. Markdown counts
# as docs only at the top level and under docs/ and evals/: elsewhere it may be embedded in the engine. The engine's
# test-only Rust is every module its parent declares under #[cfg(test)]: tests.rs and tests/, the fake and fixture
# modules, test_support.rs and prompt_parity.rs; the desktop shell's is its tests.rs (ui/src-tauri/src). ci/'s scripts
# and nextest.toml are CI, but not ci/gate.pin: the engine is built with its line (BUILT_IN_PIN). gate.toml,
# pyproject.toml, uv.lock and .python-version, which judge this repository's own changes in Jenkins (pyproject.toml
# holds no release's metadata, only the gate's Markdown linter and its rules), xtask, its chores (the eval runner and
# its bundles), .gitignore, which only tells git what to leave untracked, .copier-answers.yml, the agent-estate
# template's answers that `copier update` renders docs/agents/ from, rustfmt.toml, which says how the source is
# formatted, not what it builds, and .git-blame-ignore-revs, which only git blame reads, are in no release asset.
NOT_RELEASED=":(exclude)docs :(exclude,glob)*.md :(exclude)tests :(exclude)engine/e2e :(exclude)engine/tests
:(exclude,glob)**/tests.rs :(exclude,glob)engine/src/**/tests/** :(exclude,glob)engine/src/**/fake.rs
:(exclude,glob)engine/src/**/fixture.rs :(exclude)engine/src/test_support.rs :(exclude)engine/src/prompt_parity.rs
:(exclude,glob)ui/src/**/*.test.ts :(exclude,glob)ui/src/**/*.test.tsx :(exclude)ui/src/test
:(exclude,glob)ci/*.sh :(exclude)ci/nextest.toml :(exclude)Jenkinsfile :(exclude).github :(exclude)evals
:(exclude)gate.toml :(exclude)pyproject.toml :(exclude)uv.lock :(exclude).python-version
:(exclude)xtask :(exclude).gitignore :(exclude).copier-answers.yml :(exclude)rustfmt.toml
:(exclude).git-blame-ignore-revs"
# releases REV: exit 0 when the change of REV since its merge base with the target touches a path NOT_RELEASED leaves
# in; ends the script, saying why, when there is no merge base.
releases() {
    base=$(git merge-base "$1" "origin/$CHANGE_TARGET") || {
        echo "version-raised.sh: $1 and $CHANGE_TARGET have no merge base" >&2
        exit 1
    }
    # shellcheck disable=SC2086 # NOT_RELEASED is split into one pathspec per word on purpose.
    ! git diff --quiet "$base" "$1" -- . $NOT_RELEASED
}

if ! releases HEAD; then
    echo "version-raised.sh: the change since $CHANGE_TARGET changes only docs, tests, CI or repository tooling;" \
        "nothing to release"
    exit 0
fi

here=$(dirname "$0")
target=$(sh "$here/release-version.sh" "origin/$CHANGE_TARGET")

# greater VERSION: exit 0 when VERSION is greater than the target's.
greater() {
    [ "$1" != "$target" ] && [ "$(printf '%s\n%s\n' "$target" "$1" | sort -V | tail -n 1)" = "$1" ]
}

head=$(sh "$here/release-version.sh" HEAD)
if ! greater "$head"; then
    echo "version-raised.sh: engine/Cargo.toml's version is $head, not greater than $CHANGE_TARGET's $target; raise" \
        "it, or the merge goes unreleased (Jenkinsfile, Release), then refresh Cargo.lock with" \
        "\`cargo update --workspace\` in engine/: the build's --locked cargo steps fail on a stale one" >&2
    exit 1
fi

# Oldest first, so that the error names the pull request that goes next.
open=$(sort -n "$OPEN_PULL_REQUESTS")
for number in $open; do
    [ "$number" -lt "$CHANGE_ID" ] || break
    ref="refs/remotes/origin/pull/$number"
    git fetch --quiet origin "+refs/pull/$number/head:$ref" || {
        echo "version-raised.sh: cannot fetch pull request #$number's head; build this one again" >&2
        exit 1
    }
    releases "$ref" || continue
    theirs=$(sh "$here/release-version.sh" "$ref")
    if greater "$theirs"; then
        echo "version-raised.sh: pull request #$number, older than this one (#$CHANGE_ID), raises the version to" \
            "$theirs too, and goes first; once #$number merges, rebase onto $CHANGE_TARGET, raise" \
            "engine/Cargo.toml's version past $CHANGE_TARGET's new one, refresh Cargo.lock, and push" >&2
        exit 1
    fi
done
