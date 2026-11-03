#!/bin/sh
# Exit 0 when the release version at HEAD, pyproject.toml's [project] version, is greater than that of $CHANGE_TARGET,
# the branch a pull request build merges into, fetched from origin first, or when the change since their merge base
# touches only docs, tests or CI (NOT_RELEASED below), saying so on standard output; exit non-zero, saying why on
# standard error, when it is not, or when either version cannot be read. Versions are compared as sort -V orders them,
# number by number, so 0.10.0 is greater than 0.9.0. Jenkins's Version stage runs it on a pull request build into main:
# the Release stage releases a merge into main only when its version has no release yet, so a merge that kept main's
# version would go unreleased, which is what a change to docs, tests or CI alone should do. tests/test_invariants.py
# holds engine/Cargo.toml's version to pyproject.toml's.
set -eu
: "${CHANGE_TARGET:?is unset; only a pull request build has a target}"
git fetch --quiet origin "+refs/heads/$CHANGE_TARGET:refs/remotes/origin/$CHANGE_TARGET"

# The version of pyproject.toml's [project] table, read from standard input; nothing when it has none.
project_version() {
    sed -n '/^\[project\]$/,/^\[/s/^version = "\([^"]*\)"$/\1/p' | head -n 1
}

# The paths whose changes reach no user, as git pathspecs: docs, tests and CI. A change to any other path, or a diff
# that fails, needs the version raised.
NOT_RELEASED=":(exclude)docs :(exclude,glob)**/*.md :(exclude)tests :(exclude)engine/e2e :(exclude)engine/tests
:(exclude,glob)**/tests.rs :(exclude,glob)ui/src/**/*.test.ts :(exclude,glob)ui/src/**/*.test.tsx :(exclude)ui/src/test
:(exclude)ci :(exclude)Jenkinsfile :(exclude).github :(exclude)evals"
base=$(git merge-base HEAD "origin/$CHANGE_TARGET")
# shellcheck disable=SC2086 # NOT_RELEASED is split into one pathspec per word on purpose.
if git diff --quiet "$base" HEAD -- . $NOT_RELEASED; then
    echo "version-raised.sh: the change since $CHANGE_TARGET changes only docs, tests or CI; nothing to release"
    exit 0
fi

target_text=$(git show "origin/$CHANGE_TARGET:pyproject.toml")
target=$(printf '%s\n' "$target_text" | project_version)
head=$(project_version < pyproject.toml)
if [ -z "$target" ] || [ -z "$head" ]; then
    echo "version-raised.sh: no [project] version in pyproject.toml, here ('$head') or on $CHANGE_TARGET ('$target')" >&2
    exit 1
fi
greater=$(printf '%s\n%s\n' "$target" "$head" | sort -V | tail -n 1)
if [ "$head" = "$target" ] || [ "$greater" != "$head" ]; then
    echo "version-raised.sh: pyproject.toml's version is $head, not greater than $CHANGE_TARGET's $target; raise it," \
        "and engine/Cargo.toml's with it, or the merge goes unreleased (Jenkinsfile, Release)" >&2
    exit 1
fi
