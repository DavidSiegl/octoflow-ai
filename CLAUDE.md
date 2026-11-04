# Releases

A merge into `main` releases the version in `pyproject.toml` as GitHub release `v<version>`, if that release
doesn't exist yet (Jenkinsfile, Release stage). A change reaches users only if it raises the version.

- Every spec for a change that reaches users (the engine, the desktop app, packaging: any path `ci/version-raised.sh`
  does not exempt) lists, as an acceptance criterion, the version bump and its level:
  `uv version --bump patch` for a fix, `minor` for a new feature or flag, `major` for a change that breaks
  existing flags, exit codes or saved run state.
- While the version is 0.x, a breaking change bumps `minor`, not `major`: `major` would make it 1.0.0, which is a
  deliberate release decision, not a side effect of a change.
- Changes only to docs, tests or CI don't bump the version; Jenkins's Version stage lets them through without a
  raise (`ci/version-raised.sh` lists which paths count as such).
- Bump with `uv version --bump <level>`, never by editing `pyproject.toml`: it also updates `uv.lock`.
- Don't create tags or releases by hand; Jenkins does that after the merge.
