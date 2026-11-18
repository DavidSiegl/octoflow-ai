## Releases

A merge into `main` releases the version in `engine/Cargo.toml`'s `[package]` table (`ci/release-version.sh` prints
it) as GitHub release `v<version>`, if that release doesn't exist yet (Jenkinsfile, Release stage). A change reaches
users only if it raises the version.

- Every spec for a change that reaches users (the engine, the desktop app, packaging: any path `ci/version-raised.sh`
  does not exempt) lists, as an acceptance criterion, the version bump and its level: `patch` for a fix, `minor`
  for a new feature or flag, `major` for a change that breaks existing flags, exit codes or saved run state.
- While the version is 0.x, a breaking change bumps `minor`, not `major`: `major` would make it 1.0.0, which is a
  deliberate release decision, not a side effect of a change.
- Changes only to docs, tests, CI or repository tooling don't bump the version; Jenkins's Version stage lets them
  through without a raise (`ci/version-raised.sh` lists which paths count as such).
- Bump by editing `engine/Cargo.toml`'s `[package] version`, then refresh `Cargo.lock` with
  `cargo update --workspace`: Jenkins's `--locked` cargo steps fail a build whose `Cargo.lock` is stale.
  `pyproject.toml` has no version: it holds only the quality gate's toolchain.
- Don't create tags or releases by hand; Jenkins does that after the merge.
- One pull request that raises the version is open at a time: pull requests merge on their own once Jenkins passes
  them, so two would both claim the next version. Before opening one, check that no open pull request changes
  `engine/Cargo.toml`, the gate pin raise's from `gate-pin-raise` (`.github/workflows/raise-gate-pin.yml`) included.

## Formatting

Format Rust with `cargo fmt --all` (its settings: `rustfmt.toml`) after each change, rather than wrapping it by hand:
`ci/rustfmt.sh`, a `[[test]]` of `gate.toml` and a step of Jenkins's Test stage, fails a change it would reformat.
Run `git config blame.ignoreRevsFile .git-blame-ignore-revs` once per clone so `git blame` skips the reformat.

@docs/agents/README.md
