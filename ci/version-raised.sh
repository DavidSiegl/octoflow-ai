#!/bin/sh
# Exit 0 when the release version at HEAD, as ci/release-version.sh reads it, is greater than that of $CHANGE_TARGET,
# the branch a pull request build merges into, fetched from origin first, or when the change since their merge base
# touches only docs, tests, CI or repository tooling (NOT_RELEASED below), saying so on standard output; exit non-zero,
# saying why on standard error, when it is not, or when either version cannot be read. Versions are compared as sort -V
# orders them, number by number, so 0.10.0 is greater than 0.9.0. Jenkins's Version stage runs it on a pull request
# build into main: the Release stage releases a merge into main only when its version has no release yet, so a merge
# that kept main's version would go unreleased, which is what a change to exempt paths alone should do.
set -eu
: "${CHANGE_TARGET:?is unset; only a pull request build has a target}"
git fetch --quiet origin "+refs/heads/$CHANGE_TARGET:refs/remotes/origin/$CHANGE_TARGET"

# The paths whose changes reach no user, as git pathspecs: docs, tests, CI and repository tooling. A change to any other
# path, or a diff that fails, needs the version raised, so a path left off errs towards a raise. Markdown counts as docs
# only at the top level and under docs/ and evals/: elsewhere it may be embedded in the engine. The engine's test-only
# Rust is every module its parent declares under #[cfg(test)]: tests.rs and tests/, the fake and fixture modules,
# test_support.rs and prompt_parity.rs. gate.toml, pyproject.toml, uv.lock and .python-version, which judge this
# repository's own changes in Jenkins (pyproject.toml holds no release's metadata, only the gate's Markdown linter and
# its rules), xtask, its chores (the eval runner and its bundles), .gitignore, which only tells git what to leave
# untracked, and .copier-answers.yml, the agent-estate template's answers that `copier update` renders docs/agents/
# from, are in no release asset.
NOT_RELEASED=":(exclude)docs :(exclude,glob)*.md :(exclude)tests :(exclude)engine/e2e :(exclude)engine/tests
:(exclude,glob)**/tests.rs :(exclude,glob)engine/src/**/tests/** :(exclude,glob)engine/src/**/fake.rs
:(exclude,glob)engine/src/**/fixture.rs :(exclude)engine/src/test_support.rs :(exclude)engine/src/prompt_parity.rs
:(exclude,glob)ui/src/**/*.test.ts :(exclude,glob)ui/src/**/*.test.tsx :(exclude)ui/src/test
:(exclude)ci :(exclude)Jenkinsfile :(exclude).github :(exclude)evals :(exclude)gate.toml
:(exclude)pyproject.toml :(exclude)uv.lock :(exclude).python-version
:(exclude)xtask :(exclude).gitignore :(exclude).copier-answers.yml"
base=$(git merge-base HEAD "origin/$CHANGE_TARGET")
# shellcheck disable=SC2086 # NOT_RELEASED is split into one pathspec per word on purpose.
if git diff --quiet "$base" HEAD -- . $NOT_RELEASED; then
    echo "version-raised.sh: the change since $CHANGE_TARGET changes only docs, tests, CI or repository tooling;" \
        "nothing to release"
    exit 0
fi

here=$(dirname "$0")
target=$(sh "$here/release-version.sh" "origin/$CHANGE_TARGET")
head=$(sh "$here/release-version.sh" HEAD)
greater=$(printf '%s\n%s\n' "$target" "$head" | sort -V | tail -n 1)
if [ "$head" = "$target" ] || [ "$greater" != "$head" ]; then
    echo "version-raised.sh: engine/Cargo.toml's version is $head, not greater than $CHANGE_TARGET's $target; raise" \
        "it, or the merge goes unreleased (Jenkinsfile, Release)" >&2
    exit 1
fi
