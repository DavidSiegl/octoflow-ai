# Quality gate

How Jenkins judges this repository with the quality gate, and how `orchestrator run --quality-gate` uses it: the
[gate](#the-gate), the release this repository pins and how to run it by hand, the
[local gate](#the-local-gate), which a run can have judge its changes without Jenkins, the
[report as a contract](#the-report-as-a-contract) between the gate and the orchestrator, the
[Jenkins setup](#jenkins-setup), the
[credentials and permissions](#credentials-and-permissions) the orchestrator needs, the
[orchestrator loop](#orchestrator-loop), and the [orchestrator's own checks](#the-orchestrators-checks), which
need the run's spec.

## The gate

The gate is [octoflow-gate](https://github.com/DavidSiegl/octoflow-gate), a project of its own: it judges the change
from a base commit to the working tree with linters, the tests and their coverage, and checks of what the change did,
and writes the quality file, `quality.md`, and `quality.json`. What it checks, the settings it reads and how it reads
Rust are its own docs'
([docs/quality-gate.md](https://github.com/DavidSiegl/octoflow-gate/blob/v0.10.0/docs/quality-gate.md)).
This repository was its home until 0.62.1.

What this repository gives it:

- **`ci/gate.pin`**, the release that judges this repository's changes: one line, its version and the sha256 of its
  tarball. `ci/fetch-gate.sh` downloads that release from GitHub, checks it against the sha256 and unpacks it;
  the Jenkinsfile's Gate stage runs what it unpacked ([what the Gate stage judges](#what-the-gate-stage-judges)).
- **The rules**: `pyproject.toml`'s pymarkdown table, and the top-level `Cargo.toml`'s lints for clippy. No Python
  file is tracked, so the gate runs neither ruff nor mypy, and has no Python to measure the coverage of or
  mutation-test; `pyproject.toml` and `uv.lock` hold only the gate's toolchain.
- **`gate.toml`**, the gate's settings: the files whose edits a quality build judges without (`[build-config]`), the
  commands that run the tests outside Jenkins (`[[test]]`) and the time budgets.

**Raising the pin.** Only after the release exists: download its tarball and its `.sha256` asset, check them with
`sha256sum -c`, and write the version and the sha256 into `ci/gate.pin`. The release must read this commit's
`gate.toml`, since the gate refuses a key it does not know. A pull request that raises the pin is judged by the release
it raises it to; quality builds take the new release once it is on `main`. Lowering the pin rolls back the same way,
to 0.7.0 or later: the Gate stage and the local gate run a release's `gate`, which the releases before 0.7.0 do not
ship, so rolling back below it means reverting 0.66.0's cutover, its `Jenkinsfile` with its pin.

**Running it by hand.** Fetch the pinned release and run it in the working tree:

```bash
sh ci/fetch-gate.sh ci/gate.pin ../gate-release
uv run ../gate-release/gate --base main --out quality.md
```

`uv run` puts pymarkdown, which the gate runs, on `PATH`; the gate itself is a program for Linux x86_64 and needs no
Python.

It runs the tests itself first, by `gate.toml`'s `[[test]]` commands, which need cargo-llvm-cov and cargo-nextest;
the surviving-mutants check needs cargo-mutants, and the linters shellcheck, actionlint and clippy
(`ci/install-tools.sh` fetches the pinned cargo tools). The `[[test]]` command is `ci/rust-tests.sh`, which the
Jenkinsfile's Test stage runs too, so the coverage is measured as a Jenkins build's is; the Test stage alone gives the
engine's generated-input tests a run seed (`OCTOFLOW_TEST_SEED`), so the lines only some generated cases reach can
differ between the two. Exit status 0 means `GATE: OK`, 1
`GATE: ERROR`, 2 a gate that could not judge.

## The local gate

`--quality-gate local` has the orchestrator run octoflow-gate itself in each quality round, on the machine the run's
project is on, in place of a Jenkins job. It needs no Jenkins, no credentials and no `origin`; nothing is pushed.
A top-level Jenkins job that happens to be named `local` is written `/local`.

Each round, the orchestrator:

1. snapshots the working tree as a Jenkins round does ([snapshotting](#snapshotting-the-uncommitted-tree)) and has
   gitleaks scan the change alone, the snapshot's commits that the base does not have; a snapshot holding a secret
   is not judged, and the round's file is `GATE: LEAK`;
2. reads `ci/gate.pin`, `gate.toml` and `pyproject.toml` from the run's base commit, not from the working tree, so a
   change cannot loosen how it is itself judged, as a quality build judges by `main`'s; a base without `ci/gate.pin`
   is judged by the release the engine was tested against (`BUILT_IN_PIN`, 0.10.0), one without `gate.toml` by the
   gate's defaults and one without `pyproject.toml` by the linters' own, never by the change's;
3. checks the snapshot out as a worktree of the run's repository into `gate-<n>-<q>/job-<k>/tree` under the run's
   directory, `k` counting the round's starts, as a resume starts it again: the gate judges the files the Builder
   never told git about, as a Jenkins build does, and what the gate and the tests write never reaches the run's
   worktree or the pull request. Build caches of the run's worktree, such as `target/`, are not reused;
4. starts a job of its own on the machine, detached from the orchestrator, so the run keeps its heartbeat however
   long the tests take, and the job outlives an SSH connection that drops. The job downloads the release with curl,
   once per pin, into `${XDG_CACHE_HOME:-~/.cache}/octoflow/gate/octoflow-gate-<version>-<sha256>.tar.gz`, checks
   it against the pin's sha256 each time it is used, downloading it again when it no longer matches, unpacks it
   fresh into the job's directory, and runs its `gate --base <base> --config <the base's gate.toml>` in the
   checkout. `gate` is a program for Linux x86_64, which octoflow-gate ships since 0.7.0: a base that pins an
   earlier release stops the run, before any download, with an error that says so;
5. asks after the job every `CI_POLL_SECONDS`, and stops it, its tests included, as a Ctrl-C would, when the round's
   `QUALITY_TIMEOUT` runs out, download included, or the run is interrupted. Once the job ends, its checkout, the
   unpacked release and the download go; `quality.md`, `quality.json`, what the gate said on standard error
   (`gate.err`) and what the job said (`job.log`) stay in `gate-<n>-<q>/job-<k>/`;
6. adds the orchestrator's checks to the report, as for Jenkins, and writes `quality-<n>-<q>.md`, whose line under
   `GATE:` names the release and the snapshot. A gate that could not judge, exiting 2 with its own `error: …` line,
   such as one refusing `gate.toml`, gives `GATE: BUILD_FAILED` with the end of what it said, and goes back to the
   Builder as a Jenkins build that failed before its Gate stage does. Any other exit status 2, as for an argument
   the release does not take, and any status but 0 and 1, as 127 where the machine cannot run the gate, stops the
   run with an error: nothing the Builder changes would mend the release or the machine. So does a download that
   fails or does not match the pin.

What it needs on that machine, which the preflight checks before the interview, and `orchestrator doctor` as its
optional check `local quality gate`: Linux on x86_64, and on `PATH` curl, tar, setsid, a `ps` that takes
`-ww -o stat=,args= -p` (BusyBox's does not), sha256sum (or shasum), a writable cache, and gitleaks; and, unchecked,
whatever the project's `gate.toml` has the gate run: its `[[test]]` commands, the Python a project with Python tests
runs them in (`python3`, or `gate.toml`'s `python`) and the linters its files call for, such as pymarkdown,
shellcheck, clippy or, for a project that tracks Go files, golangci-lint (without it every round fails, unless
`gate.toml` sets `[linters] golangci-lint = []`), all from `PATH`. A linter the project runs through uv, as this
repository runs pymarkdown, is there when the orchestrator itself runs under `uv run`.

`orchestrator doctor --setup-gate`, or "Set up the quality gate" in the desktop app's setup dialog, writes at the top
level of the project's checkout, on the machine its agents run on, whichever of these is missing: `ci/gate.pin`,
naming `BUILT_IN_PIN`, and a starter `gate.toml` with a `[[test]]` for each kind of project it finds there
(`Cargo.toml`: `cargo test --workspace`; pytest's settings, in `pytest.ini`, `conftest.py` or a `[tool.pytest…]`
table of `pyproject.toml`: pytest, through `uv run` when there is a `uv.lock`, writing `junit.xml`; `package.json`:
`npm test`; `go.mod`: `go test ./...`, under a comment that the gate needs golangci-lint), or none, with a comment
saying how to add one, when it finds no kind it knows. A `pyproject.toml` alone is not taken for tests, as it may
configure only other tools. Check the guessed commands. A file already there is kept.
Nothing is committed, and nothing makes the project's runs gated: commit the files for the gate to read them, and
pass `--quality-gate local` to a run.

The gate runs the project's tests as you, with no sandbox: the same trust as running them by hand, and more than a
Jenkins agent's container gives them. A local round keeps nothing in `state.json`'s `ci`: a resume in the middle of
one stops the job the interrupted round left, if its process is still that job, and runs the round again. An
orchestrator that `resume --force` took the run over from touches only its own job's files, never the new one's.
Turning the gate off deletes no branch, as there is none. A run saved by a release before 0.63.0 with a build of a
Jenkins job named `local` in flight is not resumed, as this release would take it for the local gate.

## The report as a contract

The gate writes one report two ways, and the orchestrator reads both back from Jenkins after each quality round.
`quality.json` is the contract: the gate's JSON Schema,
[schema/quality-v1.json](https://github.com/DavidSiegl/octoflow-gate/blob/v0.10.0/schema/quality-v1.json), says what
each field holds, and the gate's own docs say when its `schema_version` is raised ([the report as a
contract](https://github.com/DavidSiegl/octoflow-gate/blob/v0.10.0/docs/quality-gate.md#the-report-as-a-contract)).
From it the orchestrator takes whether the gate passed and what failed it (`quality::read_report`): the failed
conditions, the `fingerprint` of each finding that `fails`, under its `check`, and each failing test, which it
compares with the last round's for a stuck loop (`GateFailures.repeats`). It reads only `schema_version` 1 and stops
the round, naming the version, on any other. `engine/tests/data/quality/` holds the `golden/` files of the release
`ci/gate.pin` names, and the engine's tests hold its readers of both formats to them.

The quality file, quality.md, is what the Builder and the Reviewer read: the orchestrator amends it with its own
checks' findings (`judge_run`) and writes it into the run, its words unchanged, and rejects a quality.md whose gate
line is not its quality.json's gate. It reads the parts below from it only where there is no quality.json: a build
whose quality job archived none (a fallback, which TODO.md says when to delete), and the files only it writes, for a
build that ended before its Gate stage or a snapshot it did not push. For those the engine keeps its own copy of the
parts in `engine/src/quality.rs` beside `parse_gate`, and its tests hold them to the release's golden quality files.

- **The gate line.** The first non-blank line is `GATE: OK` or `GATE: ERROR`; the reader tolerates Markdown
  emphasis around it. Only the orchestrator writes `GATE: BUILD_FAILED`, for a build that ended before its Gate
  stage, and `GATE: LEAK`, for a snapshot it did not push.
- **Sections.** Each starts with a level-2 heading, a line that starts with `##` and a space, and runs to the next.
- **Failed conditions.** Under `## Failed conditions`, one bullet per condition, as the file words it.
- **Numbered findings.** Issues and findings to weigh, numbered as one list over the whole file, at most
  `QUALITY_FINDINGS_LIMIT` (50), the rest counted. Each is a line `N.`, then where, then its severity, its rule, a
  colon and its message; where is the path and line in backticks, as `` `a.py:3` ``, the path and
  `(the whole file)` in backticks for a finding on no line, or `The change as a whole:` for one on no file. The
  rule's first word names the check, or the linter, that raised it.
- **Findings to weigh.** In a section, the line `These do not fail the gate. Fix each, or say in your report why
  it should stand.` (`TO_WEIGH_NOTE`) comes before its findings to weigh; the numbered findings above it fail the
  gate.
- **Failing tests.** Under `## Failing tests`, one bullet per test, `` - `name`: detail ``.
- **One line per item.** A character that ends a line in some reader, such as `\r` or U+2028, is written as an
  escape, `\x0d` or `\u2028`, in every list item and name (`one_line`), so each item is read back as the one line
  it was written as.

Everything else in the file, such as the intro, coverage, suppressions and build configuration, is for the Builder
and the Reviewer to read; the orchestrator passes it on as it is.

## Jenkins setup

One `Jenkinsfile` serves two jobs. Every build installs, runs the Rust tests with line coverage of the Rust, checks the
desktop app's frontend, runs the gate and archives `quality.md`; a pull request or `main` build then builds the Linux
binary and desktop app, which a quality build skips (the `Jenkinsfile`'s Package stage says why), and a build of `main`
that gets that far also releases. A build of `main` whose tree a pull request build already judged skips the tests, the
desktop app's checks and the gate (see [A build of main judged at its pull
request](#a-build-of-main-judged-at-its-pull-request)).

### What Jenkins needs

- **Plugins**: Pipeline, Git, JUnit, Workspace Cleanup, Credentials Binding and Email Extension, and GitHub Branch
  Source for the multibranch job. Email Extension sends with Jenkins's own SMTP settings.
- **An agent labelled `untrusted`**, which the `Jenkinsfile` asks for. Builds run code the agents wrote, so the
  agent must hold nothing an agent should not have: in the homelab it is a container on the agents' own box,
  restricted to this folder's jobs, while the agents that deploy accept no job from this folder. A label alone
  does not enforce that - a pushed branch's `Jenkinsfile` can ask for any label - so the restriction sits on
  the nodes, with the Job Restrictions plugin.
- **`uv`, `git`, `shellcheck`, `actionlint` and clippy on the agent**: the Install stage runs `uv sync --frozen`, and
  `uv` brings the Python of `.python-version`, which pymarkdown runs on; clippy and llvm-tools (which cargo-llvm-cov
  reads its coverage data with) are rustup components of the Rust below, which the agent image must add: its rustup
  home is root's, so a build cannot, and the Install stage fails naming a missing one. The gate fails a build that
  lacks a linter, and the tests that run the real clippy fail without it.
- **`curl`, `sha256sum`, `tar` and `openssl` on the agent**: the Install stage downloads, with `ci/install-tools.sh`,
  pinned releases of the gitleaks the e2e scenarios (`engine/e2e`) run, of cargo-llvm-cov and cargo-nextest,
  for the Rust tests and their coverage and the passes-at-base check's run of the changed Rust
  tests at the base, of cargo-mutants, for the surviving-mutants check's Rust, and of cargo-about, for the Package
  stage's third-party licenses, into the workspace's `.tools/bin`,
  which the Jenkinsfile puts first on `PATH`. cargo-mutants releases only a glibc build for linux x86_64, so the agent
  image's glibc must be one it runs on. The scenarios make the fake PyPI's certificate with openssl. Without gitleaks or openssl the
  scenarios fail, naming it.
- **Node 20.19 or later and npm, Rust 1.90 or later, and Tauri 2's system packages on the agent**, for the Test stage's
  Rust tests, the UI stage's checks of the desktop app and the Package stage's `tauri build` of it. Debian 13's `nodejs`
  and `npm` packages do for the frontend (`ui/.npmrc` makes their npm 9.2 resolve the tree), but its Rust 1.85 is older
  than the shell's locked Tauri crates need; the packages are `libwebkit2gtk-4.1-dev`, `build-essential`, `pkg-config`,
  `libxdo-dev`, `libssl-dev` and `file` ([Tauri's prerequisites](https://v2.tauri.app/start/prerequisites/)). A clean
  `cargo test` took about 100 s on 2 CPUs, and the Test stage's instrumented run of the same tests about 100 s on 3 of
  the development host's CPUs; `CARGO_HOME` is kept in the workspace, so nothing is cached between builds. The UI stage
  runs `cargo check` under the same lock, since the Test stage's build of the dependencies is not the check clippy
  needs: with them checked, the Gate stage's clippy checks only the workspace's own crates, in about 2 s for the shell
  alone, against 77 s for a clean one on the development host's 6 CPUs. The gate release is one program, built.
  Package bundles the `.deb` with `dpkg-deb`, and the `.AppImage` with `linuxdeploy`, which `tauri build` downloads
  from GitHub each build.
- **`gh` on the agent, and a GitHub token** for the Release stage: a Secret text credential with the ID
  `GitHub-Agents`, holding a token with *Contents: read and write* on this repository. It is the agents' own
  `gh` token, so when that is rotated the credential must be too, or every build of `main` fails at Release.
  Only builds of `main` use it, for Release and to read pull request statuses.
- **A GitHub App of Jenkins's own, as the checkout credential**: both jobs check out with `GitHub-Agents-scm`,
  a GitHub Branch Source *GitHub App* credential, and Jenkins reports each pull request's
  `continuous-integration/jenkins/pr-head` status with it. The App needs *Contents: read*, *Pull requests:
  read* and *Commit statuses: read and write* on this repository - without the last the build passes and the
  required check waits forever, the log saying `Could not update commit status ... 403`. Its login,
  `<app-slug>[bot]`, is `PR_STATUS_CREATOR` in the `Jenkinsfile`, and the ruleset on `main` requires the
  `pr-head` check from that App, not from any source. The agents' token cannot post as the App, so a green
  required check, and the status a build of `main` trusts, show that Jenkins set it. With the agents' token as
  the checkout credential instead, as before, the agents could post that status themselves.
  Keep the credentials in this folder's own store, not the global one: any build can bind any credential it
  can see by ID, and a pushed branch's `Jenkinsfile` is a build.

### The two jobs

Both load the `Jenkinsfile` with "Pipeline script from SCM". The Checkout stage needs that: it runs
`checkout scm`, or fetches `GIT_REF` from `scm`'s remote.

- **The ordinary job**, a Multibranch Pipeline, builds `main` and every pull request. Its branch source needs:
  - **Discover branches** set to *Exclude branches that are also filed as PRs*, so an `orchestrator/*` branch
    is built once, as its pull request.
  - **Filter by name (with wildcards)** excluding `orchestrator-ci/*`. The orchestrator pushes its snapshots
    there for the quality job and deletes them afterwards; the multibranch job would otherwise build each one
    as a branch of its own.
- **The quality job**, here `AI-Agents-Orchestrator/py-ai-agents-orchestrator-quality`: a plain Pipeline job that
  loads the `Jenkinsfile` from `main`. It takes any ref as a parameter, keeps quality builds out of the ordinary
  job's history, and a change to the `Jenkinsfile` cannot alter how that same change is judged.

Jenkins learns a Jenkinsfile's `parameters` only by running it, so a new job needs one build, by hand and with an
empty `GIT_REF`, before it accepts `buildWithParameters`.

### What the Gate stage judges

| Build | `--base` | Gate and rules |
|---|---|---|
| Quality build (`GIT_REF` set) | `HEAD^`: the snapshot's only parent is the run's base | `main`'s: the gate release its `ci/gate.pin` names, its `pyproject.toml` and `gate.toml`, from the `origin/main` commit its description names (below); a commit without `ci/gate.pin` fails the stage, unjudged |
| Pull request (`CHANGE_TARGET` set) | the merge base with the target branch, fetched first | its own, `Jenkinsfile` included (below) |
| `main` | the last successful build's commit, else `HEAD^1`, as also when a force-push took that commit out of the history | its own; none when judged at its pull request (below) |

The gate is an [octoflow-gate](https://github.com/DavidSiegl/octoflow-gate) release, the program `gate` built for
Linux x86_64, which the judge's `ci/fetch-gate.sh` downloads into `$WORKSPACE_TMP/gate` and checks against the sha256
the judge's `ci/gate.pin` names; the console says which version. A release that cannot be downloaded, or does not
match, fails the build as one the gate could not judge, with no quality file. The gate takes the linters' rules from
the `pyproject.toml` beside the `gate.toml` it is given, so a quality build's rules are `main`'s as its settings are,
and it writes `quality.json` beside `quality.md`, which the stage archives with it. A change to `ci/gate.pin` therefore
judges the changes after it reaches `main`, not itself, in a quality build; the pull request that raises it is judged
by the release it raises it to. `gate.toml`'s `[build-config]` names the pin and `ci/fetch-gate.sh`, so a quality
build's `quality.md` says so of a snapshot that edits either.

Before Install runs any code of the change, a quality build's Checkout stage sets its build description to
`judged tree=<sha> parent=<sha> main=<sha>`, exactly: full lowercase shas, one space apart. `tree` is the
snapshot's tree, `parent` its only parent, the run's base, and `main` the `origin/main` commit fetched then,
whose gate and rules the Gate stage judges with. `orchestrator judged-in-quality` reads it to decide whether a
pull request's head is a tree such a build already judged. A build restarted from a later stage skips Checkout,
records nothing and so is never trusted; its Gate stage refuses to run without the recorded commit.

A pull request is judged by the gate and rules it brings, and most from a branch of this repository merge once
Jenkins's App reports `pr-head` green, with no human in between (`.github/workflows/automerge.yml` says which).
The App's status is honest about the build, but the build ran the pull request's own `Jenkinsfile` and gate. So
whoever can push a branch here, the agents included, can change the rules that branch's merge is judged by, and
an agent's pull request, the orchestrator's own included, can reach `main` without a human. A fork's pull
request still waits for one.

The repository is public, so the stage fetches `main` or the target branch without credentials. `GIT_REF` and
the base reach the shell as environment variables, never as Groovy-built shell code, and `GIT_REF` is otherwise
interpolated only into the checkout's branch spec.

### A build of main judged at its pull request

A merge whose branch was up to date with `main` leaves `main` with the tree its pull request build judged: the
same code, tests, `Jenkinsfile`, gate and rules. The "Judged at PR" stage runs `ci/judged-at-pr.sh`, and when it
finds that build, `main`'s build skips Test and Gate, archives no `quality.md`, and goes on to Package and
Release. The script's header says what counts as judged; in short, a `pr-head` success that Jenkins's App posted
on a commit with `HEAD`'s tree that contains `HEAD^1`, with `HEAD^1` built successfully as `main`. A commit
that was never built as `main`, such as one of several a rebase merge adds, or one after a failed build of
`main`, is judged in full, so the lines of a failed build are never let through by a later skip. So is every
merge whose branch was behind `main`: its merged tree was never built.

A status counts only from Jenkins's own GitHub identity because anything a pull request build writes, an archived
file included, comes from code the agents control: the `Jenkinsfile` and tests of every commit pushed to the
pull request run unreviewed. Only the status Jenkins itself posts for a commit says that this commit's build,
with this commit's `Jenkinsfile`, passed.

### What each kind of build reports

The Test stage does not stop at a failing test, so the gate reports failing tests together with the findings; it
stops only when the tests could not be built or run. The gate's exit status then decides:

- **A quality build** ends UNSTABLE on `GATE: ERROR`, SUCCESS on `GATE: OK`. FAILURE means the change never got
  as far as the gate, and its quality file is `GATE: BUILD_FAILED`.
- **A pull request or `main`** fails on `GATE: ERROR`, so a red gate blocks the merge and `main`'s release.
- **Any build** fails when the gate could not judge.

The multibranch job mails a failed build of `main` or a pull request, with the log attached, and the first build
that passes after one. The quality job mails nothing: its result goes to the orchestrator run that asked for it.

A build of `main` that passes then releases the version in `engine/Cargo.toml` (`ci/release-version.sh`): if GitHub has
no release `v<version>` yet, `gh release create` makes one, with the tag at the commit built, notes generated from the
pull requests merged since the last release, after the words of `docs/releases/v<version>.md` where there is one, and
the Linux binary and desktop app with their `.sha256` attached. To release, raise that `version` in the pull request; a
merge that leaves it alone releases nothing. A release that already exists is skipped whole, so its files are never
replaced.

## Credentials and permissions

The orchestrator talks to Jenkins itself, over HTTP from the machine it runs on, with these credentials:

| Variable | What it holds |
|---|---|
| `JENKINS_URL` | Jenkins's base URL, e.g. `https://jenkins.example/` |
| `JENKINS_USER`, `JENKINS_TOKEN` | the technical user `agents` and an API token of it |

Put them in `~/.config/ai-agents-orchestrator/ci.env` (`$XDG_CONFIG_HOME/ai-agents-orchestrator/ci.env` if that
is set), one `NAME=value` per line, and `chmod 600` it:

```bash
JENKINS_URL=https://jenkins.example/
JENKINS_USER=agents
JENKINS_TOKEN=…
```

The orchestrator reads the file itself, on `run` and `resume`, so no wrapper is needed. Comments, blank lines,
a comment after a value, and an `export` before a name are allowed, so the same file can also be sourced by a
shell, and it means the same to both. A value is quoted, or unquoted without blanks, quotes or shell syntax; a
line that sh would read otherwise is refused, and so is a file with Windows line ends, since sh would keep each
carriage return in its value.
A variable set in the environment wins over the file, and the file is read only when one is missing. The
orchestrator refuses a file that others may read.

Never export them from `~/.bashrc` or a direnv `.envrc`: every agent's shell would have them. Values from the
file stay inside the orchestrator process and are never put into its environment.

Jenkins's user `agents` needs Overall/Read, and Job/Read and Job/Build on the `AI-Agents-Orchestrator` folder.

The token travels only in the `Authorization` header, never in a URL, so no error message, `state.json` or log
can carry it; text copied from Jenkins into a quality file has it replaced with `****`. Every process the
orchestrator starts (herdr, and through `Host` git, ssh and gh) gets its environment without these three
variables, so the agents cannot inherit them.

**Known limitation**: the agents run as the same OS user as the orchestrator, so they can still read `ci.env`.
Stripping the variables from subprocesses prevents inheritance, not access.

## Orchestrator loop

With `--quality-gate JOB`, every Builder turn is followed by a quality round, in round 1 and in each round after
`CHANGES_REQUESTED`: Builder → quality gate → (if it does not pass, back to the Builder) → Reviewer.

```text
build ──▶ quality ──▶ review
  ▲          │
  └── not OK, quality round < --max-quality-rounds, and not stuck
```

A quality round snapshots the working tree, pushes the snapshot to a throwaway branch on `origin`, has the
quality job judge it, and writes `quality-<n>-<q>.md`; with `--quality-gate local`, the gate judges it here instead
([the local gate](#the-local-gate)), and the rest of the loop is the same. The gate passes when the file says `GATE: OK`. Otherwise
the Builder gets the quality file and answers it in `build-<n>-q<q>.md`, and the next quality round judges that.
After `--max-quality-rounds` rounds (default 3) the change goes to the Reviewer anyway, whose prompt names the
last quality file as unresolved findings. Three rounds are at most three builds and two fix turns, and each
review round has its own budget. Without `--quality-gate`, a run behaves as it did before.

The loop also stops early when it is stuck: when quality round Q fails on just what round Q-1 failed on, the
Builder's turn between them fixed none of it, and another turn is unlikely to. Each round's failures are those
its record in `state.json` holds, as read from the build's `quality.json` (`read_report`): its failed conditions,
by their words; its failing tests; and every finding that fails the gate, by the fingerprint the gate gives it, a
hash of its path, rule and message with the digits taken out, so a finding whose line moved is the same finding.
The orchestrator's own findings are added by `finding_fingerprint`, the same hash. Where there is no `quality.json`
([the report as a contract](#the-report-as-a-contract)), `gate_failures` reads the same from the quality file, which
lists at most `QUALITY_FINDINGS_LIMIT` findings. When round Q's failures are not empty and are round Q-1's, no more
and no fewer (`GateFailures.repeats`), the change goes to the Reviewer, whose prompt ends with `QUALITY_STUCK_NOTE`,
and the log says why. A build that failed with nothing listed has no failures to compare, so it is never stuck by
this rule; a coverage condition is, while its percentage stays the same.

### Snapshotting the uncommitted tree

The Builder leaves its work uncommitted, and Jenkins can only fetch commits. `Host.snapshot(cwd, parent,
message)` commits the working tree through a temporary index, in one `sh -c` script: it copies the real index,
runs `git add -A` and `git write-tree` on the copy, and `git commit-tree` with `state.base` as the parent. No ref
moves, and the index, HEAD and `git status` stay as they were. `git add -A` honours `.gitignore`, and
`.orchestrator/` ignores itself, so the snapshot holds what the pull request's commit would. Its only parent
being the base is what lets the Gate stage measure the whole change against `HEAD^`.

The snapshot is pushed with `--force` to `orchestrator-ci/<key>-<n>-q<q>`; `--force` replaces what a failed
attempt left. A finished round deletes its branch, and `done` deletes any the run left.

### The calls

The engine's `Jenkins` (`engine/src/ci.rs`) makes them through `Http`, its one seam to the network, so the tests
replace Jenkins. Jenkins takes HTTP Basic `JENKINS_USER:JENKINS_TOKEN` (no crumb with an API token).
Requests time out after `HTTP_TIMEOUT = 30` seconds. The job's full name may include folders: each `/` in it
becomes `/job/` in its URL, so `AI-Agents-Orchestrator/py-ai-agents-orchestrator-quality` is
`<JENKINS_URL>/job/AI-Agents-Orchestrator/job/py-ai-agents-orchestrator-quality/` (`<job>` below).

- **Preflight**, before the interview: the three variables are set, and
  `<job>/api/json?tree=property[parameterDefinitions[name]]` lists `GIT_REF`. The directory must be a git
  repository, and a `--no-pr` run also needs a clean tree and an `origin`.
- **Per quality round**: (1) POST `<job>/buildWithParameters` with `GIT_REF`, the queue item in `Location`; (2)
  poll `<JENKINS_URL>/queue/item/<id>/api/json` for `executable.number`; (3) poll
  `<job>/<number>/api/json?tree=building,result` and then `<job>/<number>/artifact/quality.md`, two requests each
  interval, until the build has archived `quality.md` or has ended. Only for a build without one: (4)
  `<job>/<number>/testReport/api/json`, and (5) the last 60 lines of `<job>/<number>/consoleText` when no test
  failed.

The engine's tests answer these calls with `FakeJenkins` (`engine/src/ci/fake.rs`), and the e2e scenarios with their
world's Jenkins (`engine/e2e/tests/e2e/servers.rs`). `cargo xtask contract` makes them against the live quality job
instead, through the engine's own `Jenkins`, and checks each raw answer has the fields and types the fakes give it; a
field it finds shaped otherwise is fixed in the fakes in the same pull request. **It triggers one real build of the
quality job**, with `GIT_REF=main`: nothing is pushed, and the Release stage does not run for a non-empty `GIT_REF`.
Run it by hand, never in CI, before a release that changes `engine/src/ci.rs` or `engine/src/http.rs`, and after a
Jenkins upgrade:

```sh
CONTRACT_JENKINS_URL=http://cicdbox0.lan:8084/ CONTRACT_JENKINS_USER=agents CONTRACT_JENKINS_TOKEN=... \
    CONTRACT_JOB=Octoflow-AI/octoflow-ai-quality cargo xtask contract
```

Its variables are named apart from `JENKINS_URL` and the others of `ci.env`, so that one set for the engine never
starts it by accident. It prints a line per step, `ok` or what differed, and exits 0 when steps 1 to 8 pass. The
ninth, reported apart, says whether a quality round could have taken the build within `QUALITY_TIMEOUT` of the start:
the build is waited for twice that, so that a slow build still has every answer checked, and it counts as taken when a
round would take it. Two runs must not overlap, nor may another build of `main` be triggered during one: step 4 takes
the newest build of `main` to be the one it triggered. `xtask/src/contract.rs`'s module comment gives the steps and the
exit status.

**Once the Gate stage has archived `quality.md`**, live or resumed, the round takes it without waiting for the stages
after Gate, which cannot change its verdict: on a build that has ended, at the first read; on one still running, once
two reads at least a poll interval apart agree and hold a whole non-blank line, since Jenkins serves an artifact while
it is still being copied into place; a copy that stalls that long past its first line and then resumes would defeat
this. Whatever the build's result, that file is the
round's, with [the orchestrator's checks](#the-orchestrators-checks) added and a line under its `GATE:` line naming the
build, the snapshot and the round, and saying that the build ended with its result or was still running. The round then
deletes its branch, which the build checked out in its first stage; a pipeline that fetches `GIT_REF` again after Gate
finds it gone. **Without `quality.md`**, from a build that ended ABORTED or one cancelled in the queue, nothing judged
the change, so the run fails with `ci` cut back to `ref` and `sha`, and the Builder gets no fix turn. Without it
otherwise: `GATE: BUILD_FAILED`, with the failing tests or the console's end, and a fix turn. A build cut back that way
is recorded in `ci.rejected`, so a resume builds the snapshot again instead of adopting that build.

**When Jenkins fails**, rather than the change: a request that got no answer, or a 5xx, is retried every
`CI_POLL_SECONDS = 10` until `QUALITY_TIMEOUT = 1200` seconds after the round started; the run then fails, naming
the step and keeping `ci`, and a resume gets a fresh timeout. Any other answer, such as 401, 403 or 404, fails the
run at once, naming the step, method, URL and status. A 404 on the trigger means the job was deleted. The poll
loops call `_heartbeat`, so a long build does not make the run look stale.

### The orchestrator's checks

Some checks need the run's spec, which lives in the run directory (`.orchestrator/`, ignored, so never in a
snapshot) and so never reaches Jenkins. The orchestrator runs these itself, in each quality round, after it reads
the build's `quality.md` and before it writes `quality-<n>-<q>.md`; a build that ended before its Gate stage gets
none of them. They run only with `--quality-gate`. The spec is the contract step's file, `spec.md` in the default
workflow, or the run's task for a workflow without one. Each check is an `OrchestratorCheck` in
`orchestrator_checks`, and judges the change from the run's base to the round's snapshot:

| Check | Fails? | Finds |
|---|---|---|
| `new-dependencies` | yes | packages the change adds that the spec does not name or their index does not know |
| `diff-budget` | no | a change over the workflow's `max_files` or `max_lines` |
| `unexpected-paths` | no | changed paths the spec never mentions |

**New dependencies** reads the project's manifests from the repository's top level only, in two ecosystems: a Python
project's `pyproject.toml` and the `uv.lock` beside it, when the change touches either, and a Cargo workspace's
`Cargo.toml`, the manifests of the members its `[workspace] members` names and the `Cargo.lock` beside it, when the
change touches any `Cargo.toml` or that `Cargo.lock`. A manifest anywhere else, such as a test fixture, is not the
project's. It reads them in the snapshot and at the base, both from the top level: a file the change moves there, such
as a fixture's `pyproject.toml`, was not the project's at the base, so what it lists is new. The direct dependencies
are `pyproject.toml`'s `[project] dependencies` and `optional-dependencies` and its `[dependency-groups]`, the project
itself left out, so `all = ["myproj[a,b]"]` adds nothing; the locked packages are `uv.lock`'s, the project and its
workspace members left out. A Cargo workspace's direct dependencies are the `[dependencies]`, `[dev-dependencies]` and
`[build-dependencies]` of the top-level manifest and each member's, with their `[target.<cfg>]` forms, and the top
level's `[workspace.dependencies]`, each by its crate's name (its `package` when the entry renames it); a member named
by a glob, such as `crates/*`, makes the manifest `unreadable`, since the check does not expand it. The locked
packages are `Cargo.lock`'s from a registry or a repository, the workspace's own crates and those from a path left
out. A package is new when the snapshot's files name it and the base's do not. Names compare as PyPI compares them
(PEP 503), which is also how crates.io compares a crate's, and the spec names a package when it holds the name as a
word, in any case, so `pytest_randomly` names `pytest-randomly` and `pytest-randomly` does not name `pytest`. The spec
was agreed with the human, so naming a package there is the approval.

Where a package comes from decides whether its index, PyPI for a Python package and crates.io for a crate, is asked
about it. A direct dependency's source is its `[tool.uv.sources]` entry (`workspace`, `path`, `git`, `url` or
`index`), else its PEP 508 URL (`name @ url`), else the `[[tool.uv.index]]` marked `default = true`, else the registry
`uv.lock` has it from; a locked package's is its `source` in `uv.lock`. A package named from several sources is a
dependency from each: a requirement with a URL beside one without, each entry of a `[tool.uv.sources]` list (each for
some platforms), each registry `uv.lock` has it from. Only a package from PyPI's own index
(`https://pypi.org/simple`) is looked up, at `https://pypi.org/pypi/<name>/json`, with a timeout of 15 seconds
(`PYPI_TIMEOUT`). A package from another index is never looked up on pypi.org, so a name someone squatted there cannot
lend it pypi.org's metadata: that is the dependency-confusion case, and the finding names the index instead. Such a
package is therefore never `not-on-pypi`; only the spec rule can fail it. A crate's source is its entry's: `path`, a
member of the workspace, which the spec need not name; `git`; `registry` or `registry-index`, an index other than
crates.io, whose crate is `other-index`; else crates.io. An entry with `workspace = true` has its
`[workspace.dependencies]` entry's source; one whose `workspace` is anything else, such as `"true"` or `false`,
makes its manifest `unreadable`, as Cargo refuses it. A locked crate has its `source` in `Cargo.lock`. A crate named
from several sources, such as `foo = "1"` in `[dependencies]` and `foo = { git = "..." }` under
`[target.'cfg(windows)'.dependencies]`, is a dependency from each, and each source of a new crate is judged on its
own. Only a crate from crates.io is looked up, at `https://crates.io/api/v1/crates/<name>`, with a timeout of 15 seconds
(`CRATES_IO_TIMEOUT`) and a User-Agent that names the project, which crates.io requires; nothing of the user's goes
with it. Its first release is the crate's creation, and its latest version crates.io's default one, the highest that
is neither yanked nor a pre-release.

- `not-in-spec` (fails): a new direct dependency the spec does not name, with where it comes from, unless it is a
  member of the project's workspace (`{ workspace = true }`): the project's own code, not a package from outside;
- `not-on-pypi` (fails): a package from PyPI that PyPI answers 404 for. That is the slopsquatting case: a name an
  agent made up, which anyone could register. `not-on-crates-io` is the same for a crate;
- `unchecked` (fails): the packages from an index that it was not asked about, one finding per index, because a
  request got no usable answer (no answer, a timeout, an HTTP error other than 404, an answer that is not the index's
  JSON), or because the quality round had too little time left. An index is asked no more after the first such
  request it gets, while the other still is, and none is started with less than 60 seconds (`SLOW_STEP_MARGIN`) of
  the round's `QUALITY_TIMEOUT` left. The run's heartbeat is kept between
  requests, so a long list of new packages cannot make a live run look stale. A package that could not be checked is
  never reported as passed, so the next quality round asks again;
- `unreadable` (fails): a manifest in the change that is not UTF-8 TOML, or not in the shape above. One that cannot
  be read at the base is a finding to weigh instead, since the Builder did not write it, and so is one that cannot be
  read in the change because it could not at the base either. Each manifest is judged by its own two sides, so a
  `pyproject.toml` the change breaks fails even beside a `uv.lock` broken all along. Either way that ecosystem's new
  dependencies are not checked;
- `added` (reports): a new direct dependency the spec names, or a workspace member, with its first release date, its
  latest version and its date, and its license, or the path, repository or URL it comes from. `young` instead when its
  first release is less than 14 days old (`YOUNG_PACKAGE_DAYS`), or it has no release files: few have used it yet.
  `other-index` instead for one from an index other than its ecosystem's, naming the index, and `unknown-source` for one whose
  `[tool.uv.sources]` entry has a shape this check does not know (none of `workspace`, `path`, `git`, `url` or
  `index`), quoting the entry: where it comes from is not checked, so it is reported rather than passed in silence;
- `pulled-in` (reports): one finding per `uv.lock` or `Cargo.lock`, listing with the same facts the packages it alone
  adds, those the new dependencies pull in, a package from several sources once for each. The spec cannot be expected
  to name them;
- `source-changed` (reports): a dependency the project already had, direct or locked, that the change takes from
  a source it did not, instead or as well, naming the sources of both sides: `requests` moved from PyPI to
  `{ index = "evil" }` is the dependency-confusion move without a new name. A new default index moves every
  dependency without a source of its own.

**Diff budget** counts the files the change touches and the lines it adds and removes, from `git diff --numstat -M`
(`Host::changed_line_counts`); a rename counts as one file, with the lines it changes, and a binary file as a file of
no lines. Over either limit it is one finding about the change as a whole, naming each count over its limit; within
both it reports nothing, so the counts of a change of usual size do not take a number the Builder must answer. The
limits are workflow keys, a whole number of at least 1 each, in a workflow file next to `description`:

```toml
max_files = 15   # the defaults
max_lines = 600
```

They belong to the workflow rather than to the quality-gated step, because the change a quality round judges runs
from the run's base, so it holds every editing step's work, a Test Writer's as well as the Builder's.

**Unexpected paths** is a finding for each path the change touches, or deletes, that the spec mentions neither as a
whole nor by its file name. A rename is judged at its new path only. A path or name counts as mentioned only where it
appears whole in the spec, as a plain substring: naming `docs/` does not cover `docs/x.md`.

The findings go into the quality file as the gate's do, with `{path, line, severity, rule, message}` and the rule
prefixed by the check's name, in one last section, `## The orchestrator's checks`, which holds both the failing ones
and those to weigh. They are numbered on from the build's last number, failing ones first, then the findings to weigh
under the same lead sentence as the gate's. The limit of 50 is the whole file's: they are listed only while the file
holds fewer, and the rest are counted. A check with failing findings is a failed condition, added to the build's, and
turns `GATE: OK` into `GATE: ERROR`. Findings to weigh never change the gate. The Builder answers them all by number,
as it answers the gate's.

### What goes to the Builder

`QUALITY_FIX_PROMPT` names the quality file and the Builder's next report, `build-<n>-q<q>.md`, whose findings it
answers by number. A Builder in a fresh session gets `BUILD_PROMPT`, `REBUILD_NOTE` with every earlier report, and
`QUALITY_FIX_PROMPT`. The Reviewer's prompt ends with `QUALITY_PASSED_NOTE`, `QUALITY_UNRESOLVED_NOTE` or
`QUALITY_STUCK_NOTE`, naming the round's last quality file, and points at the Builder's last report of the round.
The pull request's description includes the last quality file.

A snapshot in which gitleaks, by the base's rules, finds a secret or an added comment telling it to allow a line is not
pushed, and no build is triggered. Its quality file is `GATE: LEAK`, saying why, with the findings numbered under
`## Secrets found`: each one's `file:line`, rule, commit and fingerprint, never the secret, which only gitleaks'
redacted report `leaks-snapshot.json` in the run's directory describes. The Builder gets `QUALITY_FIX_PROMPT` as for
any other gate, and the round counts toward `--max-quality-rounds`; a round that finds the same secrets as the one
before it, as many of each rule in each file, stops the loop early as any repeated failure does. A finding that is not a secret
passes only once the human lists it as `file:rule:line` (the fingerprint without its commit, which is the round's
snapshot) in `.gitleaksignore` on the base branch, and only in a run started after that: a run keeps the rules of
its base. A secret in a commit the snapshot does not add, the run's base or a commit below it that origin lacks,
fails the run instead: those are the human's commits. So does one in the branch at publish.

### Run state and resume

`state.json` holds `quality_job` (None for a run without the gate, as for one saved before it; `local` for the
[local gate](#the-local-gate)),
`max_quality_rounds`, `quality_round` and `ci`, `{ref, sha, queue_url, build}`, saved key by key, with `rejected`
as above. Phase `build` with q = 0 goes to `quality` q = 1; `quality` q goes to `review` on `OK` or at q ≥
`max_quality_rounds`, else to the fix turn, `build` with the same q, then `quality` q + 1; `CHANGES_REQUESTED`
goes to `build`, round n + 1, q = 0. `list` shows the phase `quality` and, in `build` and `quality`, the quality
round.

A resume in `quality` takes the furthest step the state allows: an existing quality file (one without a valid
`GATE:` line is refused, as an empty handoff file is); `ci.build` (when the job has discarded that build, the run
fails, cut back to `ci.ref`, so the next resume builds it again); `ci.queue_url` (when Jenkins has forgotten the
queue item, the build with that `GIT_REF` among the job's last 20); `ci.ref` alone, where a build with that
`GIT_REF` is adopted before a new one is triggered; otherwise snapshot, push and trigger. A state saved while
quality rounds went through SonarQube loads too: its project and baseline are dropped, its `ci.ce_task` is
ignored, and its build, which archived no `quality.md`, counts as one that ended before the gate.

### CLI

`run` takes `--quality-gate JOB` and `--max-quality-rounds N`; `resume` takes `--max-quality-rounds N`.
`--max-quality-rounds` below 1 is refused, and so is it on `run` without `--quality-gate` or on `resume` of a run
without the gate. `resume` refuses a limit below `quality_round` in phase `quality`, or up to it in `build` with
q ≥ 1, where the Builder is answering quality round q. `resume` also takes `--quality-gate JOB` or
`--no-quality-gate`, below; `--max-quality-rounds` goes with the first and is a usage error with the second.

`judged-in-quality --quality-gate JOB --target BRANCH` exits 0 when a quality build of `JOB` already judged
`HEAD` as a pull request's build into `BRANCH` would: among the builds Jenkins keeps, it ended SUCCESS, and its
description (above) names `HEAD`'s tree, the merge base of `HEAD` and `origin/BRANCH` as its parent, and a `main`
commit at which `Jenkinsfile`, `ci/gate.pin`, `ci/fetch-gate.sh`, `gate.toml`, `pyproject.toml`, `uv.lock`,
`.python-version`, `rust-toolchain.toml` and `Cargo.lock` are as in `HEAD` and at that merge base. Such a build runs the e2e scenarios (`engine/e2e`) only if the change
touches what they depend on, where a pull request build always runs them; every path that could make the two differ
makes the quality build run them too (the Jenkinsfile's Test stage lists them). Otherwise, and on any error, such as
Jenkins unreachable, an HTTP error or a commit it cannot fetch, it exits 1 with one line on standard error naming the condition the closest build fails, or
what stopped it asking. It needs the same `JENKINS_*` credentials as `--quality-gate`. No build calls it yet.

### Turning the gate on or off on resume

`turn_quality_gate_on` or `turn_quality_gate_off` applies the flag to the saved state before the run goes on; a refusal exits 1 and leaves
`state.json` as it was.

`resume --quality-gate JOB` saves `JOB` as `quality_job`, after the preflight: in round 0, `_check_gate`, as for
`run --quality-gate`; later, an `origin`, gitleaks, the credentials and `CI.preflight`, before any turn, or for
`local`, gitleaks and the local gate's preflight. Where the gate
starts depends on where the run is:

- in round 0, or in `build`: that Builder turn is followed by quality round 1;
- in `review`, with `review-<n>.md` neither written, prompted for, asked again for its verdict nor waiting on a
  BLOCKED answer: phase `quality`, q = 1, so the Builder's report is judged before the Reviewer sees it, and a
  failure goes back to the Builder as usual;
- otherwise, a Reviewer already prompted included: at the Builder's next turn.

It is refused in `publish` or `done`, even with the run's own job, after `--no-quality-gate` (the gate cannot come
back), for a run that has another job (the same job is a no-op), in a workflow without a quality-gated step, and for
a `--no-pr` run in the checkout itself past round 0: its tree was never required to be clean, and a snapshot takes
the whole tree.

`resume --no-quality-gate` deletes the run's `orchestrator-ci/<key>-*` branches on `origin` (a run with the local
gate has none), a failure only logged, without cancelling a Jenkins build in progress, and saves `quality_job` null, `quality_gate_disabled` true
and `ci` empty. A run in `quality` goes on at `review`, dropping an analysis that has not finished; a Builder in
its fix turn writes `build-<n>-q<q>.md` and goes straight to `review`, with `quality_round` past the quality round
no build ever judges. The Reviewer reads the Builder's latest report of the round. Its prompt ends with
`QUALITY_PASSED_NOTE`, `QUALITY_UNRESOLVED_NOTE` or `QUALITY_STUCK_NOTE` for the round's last quality file that is
written; with `QUALITY_OFF_NOTE`, naming the round's quality files, when the Builder changed the code after it; and
with nothing when the round has none. The pull request carries that file. When no quality file judges the code it
publishes, because the Builder changed the code after the last one or the gate was turned off before any analysis
finished, it says so in a note and cites the run's last quality file from any round, if there is one, as `Quality
gate (round R, before the last change)`, its `Orchestrator-Gate` trailer ending `, then turned off`. A workflow
without a verdict step ends `FINISHED`. `show` prints the job on a `quality` line, or `off (turned off by resume)`.

### Edge cases

- **Concurrent runs** share only Jenkins's executors; `QUALITY_TIMEOUT` allows for a queue.
- **Two runs in one checkout**: a snapshot takes every change in the tree, so it would take the other run's
  too. A run refuses to start or resume while another run in its checkout is live, so the two never overlap.
  A `run --worktree` run snapshots its own worktree, which holds only its change, so worktree runs go in
  parallel in one checkout, beside each other and beside one run in the checkout itself.
- **No change**: the gate passes, and `_publish` refuses the empty change.
- **Cleanup**: a failed run keeps the branch in flight; `done` deletes the run's `orchestrator-ci/<key>-*`
  branches, and a failed delete is only logged.
- **Reachability under `--machine`**: HTTP runs on the orchestrator's machine and git on the agents', meeting at
  `origin`. If only the agents' machine reaches Jenkins, the calls would need to go through `Host`, with curl
  taking its config on stdin.
