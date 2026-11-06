# TODO

What is known to be open in the engine (`engine/`), the desktop app (`ui/`), `gate.py` and CI, and why. A fixed entry
is deleted; git history and the release notes (`docs/releases/`) keep what was done. An entry that a fix only narrows
is rewritten to say what is left.

## Planned

- **The rename to Octoflow AI, the rest of it.** The product is Octoflow AI (README, window titles, pull request text)
  and the release assets, `.deb` path, Tauri bundle and crates are `octoflow`; the bundle name has no space because
  Tauri names the `.deb`, `.app` and `.dmg` after it verbatim. Still under the old name, each a decision of its own:
  `~/.config/ai-agents-orchestrator/`, the `.orchestrator/` project directory and the `orchestrator-ci/` branches, which
  are on users' disks and remotes and need a fallback that reads the old location; the engine's binary `orchestrator`
  and the `Orchestrator-*` commit trailers; the Jenkins folder `AI-Agents-Orchestrator`. The macOS app could show
  "Octoflow AI" in Finder while staying `Octoflow.app`, with `CFBundleDisplayName` in a `ui/src-tauri/Info.plist`,
  unchecked on a Mac. Agreed 2026-10-25.

- **The desktop app as the terminal client too: the macOS check.** Started with arguments, the app execs the
  orchestrator it bundles (`cli` in `ui/src-tauri/src/lib.rs`), so the AppImage is the CLI too and the README says
  how to get the CLI from each download; on macOS, where a `.app` is a directory, the CLI is a symlink to
  `Octoflow.app/Contents/MacOS/octoflow`. `build-desktop.sh`'s `smoke_dmg` mounts the `.dmg` and runs the CLI
  through such a symlink and as the app's own executable, in `.github/workflows/macos-binary.yml`'s build of each
  release and of each pull request that touches the packaging. Left: checking the README's quarantine advice for
  the `.dmg` against a downloaded copy (`.scratch/desktop-cli/issues/02-macos-cli-symlink.md`). The plain CLI
  binaries stay released: 13 MB against the AppImage's 120 MB of WebKit, and they start without FUSE on headless
  machines and in CI. Decided against: one file on every platform, by embedding the orchestrator's bytes in the
  Rust executable and unpacking them at start (PyInstaller's onefile done twice), or by Python hosting the window,
  which [ui-options.md](docs/ui-options.md) chose against. Agreed 2026-10-24.
- **The quality gate in a repository of its own: what is left.** The gate moved to
  [octoflow-gate](https://github.com/DavidSiegl/octoflow-gate) (decided 2026-11-05: the agent-estate template's
  `scripts/gate` is the second project that uses it), where the test-file patterns and the rest are `gate.toml`
  settings, and Jenkins judges this repository's changes with the release `ci/gate.pin` names
  (`.scratch/gate-standalone/`, ticket 07), whose `quality.json` the engine reads (08). Left: `--quality-gate local`
  (09), and removing `gate.py`, `ci/gate-rust/` and their tests, which Jenkins no
  longer runs to judge, from this repository (10).
- **The e2e scenarios: what they do not record.** `engine/e2e/` runs 43 scenarios against the engine as a program and
  compares each with its recording (docs/architecture.md, Tests). The recordings are the Python engine's, made before
  rust-engine ticket 11 removed it, by the Python harness octoflow-without-python ticket 02 removed; a scenario added
  since records the Rust engine, and its recording is a review of the engine's behaviour. Not covered: a secret gitleaks
  finds, at a quality round or at publish; the harnesses other than claude and codex starting an agent (only their
  arguments differ); a session log's token counts (every turn records `unknown`); `--timeout` and the stall notice,
  which take real minutes; the GUI, which the Rust engine drops; round 2 of the example workflows and a fresh session's
  prompts as sent, whose templates only the recorded `state.json` holds and whose text only golden files hold
  (docs/architecture.md, Tests); and texts the engine sends or writes that neither a scenario nor a golden file reaches:
  `CONTINUE_PROMPT`, `QUALITY_OFF_NOTE`, the pull request body of a run that ends `FINISHED` and of one whose gate was
  turned off after its last analysis, and a body section cut at `PR_SECTION_LIMIT`. Each closes with a scenario that
  reaches it, its recording made by the Rust engine and reviewed (found by orchestrator-test-methods/06 and its review
  74, 2026-10-27, against the recordings). Where gitleaks or openssl is not on PATH, every scenario fails naming it: a
  skip would pass a machine that checked nothing, and the gate's test-integrity check fails an added skip (the user's
  ruling, 2026-10-27); Jenkins's Install stage downloads the pinned gitleaks (`ci/install-tools.sh`). What a record
  holds by design, each a choice a later ticket may revisit: output a human reads (logs, errors, `show`, `list`,
  `prune`) as its facts (`normalize::facts`), not its words; a run's files listed by modification time, as `/files` and
  the event stream list them, sorted by name, since seconds of the clock decide that order; ssh's commands not at all,
  only what they did; doctor's lines without git's and Tk's, which are the host's. Two more are open issues below: the
  `state.json` saves between two commands, and serve's events between two checkpoints. One engine run takes 2 to 25
  seconds a scenario (25 for serve's event stream); when a quality build runs them is the Jenkinsfile's Test stage's to
  say. Decided against (2026-10-27): scenarios as data files the harness interprets, which needed a language of their
  own for kills, takeovers and serve's stream; a cassette of the expected requests that the fakes answer in order, which
  breaks on every count of polls; and a stub gitleaks where none is installed, which would record what no engine run
  with gitleaks does. Decided against too (2026-10-27, orchestrator-test-methods/06): golden files of the engine's
  follow-up prompts, notes, commit messages and pull request bodies, which the recordings hold as sent where a scenario
  reaches them.

- **The eval set: what it does not cover.** `cargo xtask eval` (live-checks/04, octoflow-without-python/08,
  docs/design.md, Evals) runs the engine as a program, `run --no-pr --spec`, rather than being the `orchestrator.py
  eval` command the ticket named, since the Python engine was frozen then; it runs `EVAL_ENGINE`, else the checkout's
  own build. Whether the engine gets an `eval` command of its own is open. Not covered: the quality gate, since the
  toy repositories have no quality job and their origin is a bundle no snapshot can be pushed to, so `quality rounds`
  stays 0; a `--worktree` or `--machine` run, which the runner refuses; and an interview, which `--spec` skips.
  Decided 2026-10-28. The verification run it still lacks is under Open issues.

## Open issues

- **The engine still reads a build's `quality.md` where it archived no `quality.json`.** `Workflow::quality`
  (`engine/src/workflow/quality_rounds.rs`) falls back to `parse_gate` and `gate_failures` for a build without
  `quality.json`: a quality job whose judged `main` predates gate-standalone/07, or another project's job running a
  gate release before 0.5.0. Delete the fallback for builds once no supported quality job lacks `quality.json`;
  `parse_gate` and `gate_failures` stay for the files the engine writes itself (`GATE: BUILD_FAILED`, `GATE: LEAK`)
  until those are judgements of their own, and with them `TestQualityFileContract`.

- **No `doctor` check that a harness can start in the project without asking.** Claude Code's folder-trust dialog
  stops a run before its first turn; run-usability/02 made the run say so and the README list it, but `doctor` does
  not check it. Decided against reading Claude Code's trust record (`~/.claude.json`, `projects.<path>`) on
  2026-11-05: an undocumented file of another tool, which would break silently when it changes. Closes if Claude
  Code documents a way to ask whether a folder is trusted.
- **The desktop app does not say which verdict step sent a run back.** `list` names it (`run_outcome`,
  `engine/src/cli/listing.rs`) and serve's `/runs` carries `verdicts`, but the app's `outcome` (`ui/src/runs.ts`)
  shows only `verdict`, and its `RunRecord` (`ui/src/api.ts`) has no `verdicts`. Left out of acceptance-verdict
  ticket 01 (2026-11-04): this checkout had no `node_modules` to run the app's tests with. Closes with
  `verdicts?` on `RunRecord` and `outcome` naming the last step as `run_outcome` does, with a `runs.test.ts` case.
- **The proofs note names the Reviewer whatever step wrote the proofs.** `PROOFS_NOTE` (`engine/src/prompts.rs`)
  tells the step a round loops back to "The Reviewer backed findings with tests". In a workflow whose proofs come
  from a verdict step of another role, that names a role the run does not have, the mismatch acceptance-verdict
  ticket 01 fixed in `FIX_PROMPT` (cross-review 141, F15). Raised and not recorded in cross-review 141's round 4
  (2026-11-05). Closes with a role-neutral wording, mirrored in the golden prompts, the recordings and the test in
  `engine/src/workflow/tests/proofs.rs`.
- **A non-editing step between two verdict steps is treated as having run every round.** `loop_problem` allows one
  (`engine/src/pipeline.rs`), and a round an earlier verdict step ends skips it, but `Workflow::judged_rounds`
  (`engine/src/workflow.rs`) knows the skipped rounds only of verdict steps: such a step's first turn in a later
  round gets its `again`, and its `{prev_X_path}` and `{earlier_X_paths}` name files of rounds it skipped. Closes with
  turn records (`RunState.turns`) deciding which rounds any step took a turn in.
- **No check that a verdict step's loop runs an editing step.** `Pipeline::loop_problem` (`engine/src/pipeline.rs`)
  lets a verdict step loop back to a step at or before the first verdict step, including the first verdict step itself
  or a non-editing per-round step. A round sent back that way judges the same code again. The Builder never sees
  `{findings}`, `pr_body`'s Builder report for the last round reads a `build-{n}.md` nobody wrote, and the verdict
  step's `again` names that missing file. Raised and not recorded in cross-review 141's round 6 (2026-11-05). Closes
  with `loop_problem` refusing a loop whose steps edit nothing, or with the prompts and the pull request handling a
  round without a build.
- **Writes to origin are not tried again through an outage.** `reaching_origin` (`engine/src/workflow/origin.rs`)
  wraps the run's fetches and its `ls-remote` reads only; the pushes (`engine/src/host.rs`'s `push_ref`,
  `push_branch`), the remote branch deletions (`delete_remote_branch`, `delete_remote_branch_at`) and the `gh` calls
  fail at the first refusal, as GitHub's incident of 2026-11-04 refused known keys. A write that reached origin but
  reported failure needs its own handling first: a delete tried again finds no ref, and a push under a lease sees
  the value it set. Left out of origin-retry ticket 01 (2026-11-04). Closes with each write's retry made safe to
  repeat, then routed through `reaching_origin`.
- **Workflow keeps its Collaborators field by field.** `Workflow::new` (`engine/src/workflow.rs`) takes a
  `Collaborators` apart into eight fields, so code shared between a Workflow and a CLI command that holds the
  `Collaborators` takes the pieces it needs one by one: `delete_ci_refs` (`workflow/quality_rounds.rs`) takes host,
  human, clocks and interrupt, while `turn_quality_gate_off` (`workflow/gate_switch.rs`) takes the whole. Found in
  origin-retry ticket 01 (2026-11-04). The two halves of resume's gate switch differ the same way: deferred from
  cross-review 135 (F8, nit): "turn_quality_gate_on takes `host, human` and turn_quality_gate_off takes
  `&Collaborators`" (`engine/src/cli/start.rs`). Closes with Workflow holding its `Collaborators`, the helpers and
  both gate switches taking it.
- **A hang reaching origin is not tried again.** `reaching_origin` (`engine/src/workflow/origin.rs`) tries again only
  on what git writes when it gives up; a fetch on a network that drops packets is killed at `NETWORK_TIMEOUT`
  (300 s, `engine/src/host/process.rs`), whose error says none of that, and the run stops. Deferred from cross-review
  135 (F4, should-fix, 2026-11-04): "When the network drops packets silently, or ssh hangs after connecting, git
  does not fail on its own … That text matches no UNREACHABLE entry, so the run fails on the first try". Four tries
  at 300 s would hold a run for 20 minutes. Closes with a shorter limit for each try of a read of origin (git's
  `http.lowSpeedLimit`/`lowSpeedTime`, ssh's `ConnectTimeout` and `ServerAliveInterval`), its timeout then retried.
- **Host::fetch does not wait out an outage by itself.** Each caller wraps it in `reaching_origin`
  (`engine/src/workflow.rs`'s five, `quality_rounds.rs`'s `delete_ci_refs`, `cli/prune.rs`'s `prune_remote`), since a
  `Host` (`engine/src/host.rs`) has no clock to wait on and no human to tell; a new caller gets no retry unless it
  wraps too, and a wrapped `fast_forward` or `merge_upstream` would also retry a local merge whose error happened to
  say a phrase of `UNREACHABLE`. Deferred from cross-review 135 (F7, nit, 2026-11-04). Closes with the waiting and
  the telling reachable from `Host`, so that its origin reads retry themselves.
- **An npm package's NOTICE file ships only for a build tool.** `ui/scripts/npm-licenses.js` appends the
  `THIRD-PARTY-LICEN[CS]E*` and `NOTICE*` files of the `BUILD_CODE` packages (Vite, Rolldown) to their notices, and
  of no other; Apache-2.0 §4(d) wants a NOTICE redistributed with any work it comes with, so a production package
  under Apache-2.0 that ships one would go out without it, silently. None does today (@tauri-apps/api has none).
  Raised, not recorded, by cross-review 131 (round 4, 2026-11-04): "a production Apache package's NOTICE would be
  dropped silently". Closes with every shipped package's `NOTICE*` appended to its notice, or the build failing on a
  production package that has one.
- **The npm notices miss a build tool's third-party notices under another file name.** `withThirdPartyNotices`
  (`ui/scripts/npm-licenses.js`) takes `/^(third-party-licen[cs]e|notice)/i`, so `THIRD_PARTY_LICENSES` or
  `ThirdPartyNotices.txt` in the next `BUILD_CODE` package is left out without a word, and a `LICENSE-THIRD-PARTY`
  falls into the license pattern and fails the build. Vite and Rolldown name theirs as matched. Raised, not recorded,
  by cross-review 131 (round 4, 2026-11-04). Closes with a pattern that takes `third[-_ ]?party` in any case and
  position, or a test that lists each `BUILD_CODE` package's notice-like files and fails on one not shipped.
- **The npm notices' refusal of a multi-licensed package says it has no license text.** A package offered under a
  license `packaging/about.toml` does not accept, whose one license file names none of the licenses, fails
  `npmNotices` with "no one license text for MIT OR GPL-3.0-only", which sends the reader looking for a missing file;
  the refusal is that the text may be the unaccepted license's alone, and `npmNotices`'s `@throws` does not name the
  case. Raised, not recorded, by cross-review 131 (round 4, 2026-11-04). Closes with a message of its own for that
  case ("one license text for an offer not every license of which about.toml accepts") and the `@throws` naming it.
- **The project bar's last button runs past a narrow window's edge.** At 560 px wide the bar's fields keep their
  minimum widths and "Check setup…" wraps onto two lines and ends past the page's right padding; origin/main at
  a357407 does the same, so the logo's header (gui-logo/01) did not cause it. Seen in a headless screenshot of
  `vite preview` (2026-11-04). Closes with the bar's fields allowed to shrink or wrap (`flex: 1 1 <basis>` with a
  smaller `min-width`) so that every control stays inside the window down to the narrowest width the app supports.

- **The desktop app's dialogs do not keep keyboard focus.** `Dialog` (`ui/src/components/Dialog.tsx`), which the
  setup dialog and the run form are, sets `aria-modal` and covers the window with its backdrop, but Tab still reaches
  the project bar under it. Opening another project while the run form waits for `POST /run` remounts `ProjectRuns`
  (keyed by the project, `ui/src/App.tsx`): the run starts, but is not selected, and a failure's message is lost.
  Deferred from cross-review 124 of app-start-runs-02 (F5, should-fix, 2026-11-04): "`api.startRun(...).then(onStarted,
  ...)` is not tied to the component's lifetime … If ProjectRuns remounts while a start is pending … the run starts
  but is never selected, and a failure's message is lost." Closes with the window behind a dialog made inert
  (`inert`, or a `<dialog>` opened with `showModal()`, where the webviews and jsdom have it).
- **serve starts a run through a shell it takes for POSIX or fish.** `Launcher::launch` (`ProcessHerdr`,
  `engine/src/herdr.rs`) gives the new pane's shell each word of the `run` command as an environment variable and
  types `"$OCTOFLOW_ARG_0" "$OCTOFLOW_ARG_1" …; echo '<marker>'`, since herdr 0.9.1's `pane run` types its
  arguments into the shell. bash, zsh, dash and fish run that; a pane whose shell is nushell, xonsh or PowerShell
  does not, and POST /run answers 422 with the shell's error, or 202 when the marker never shows. Found while
  implementing app-start-runs ticket 01 (2026-11-03). Closes with a launch that does not go through the pane's
  shell: herdr starting the command itself in the new pane, if a later herdr can.
- **new-dependencies points a second source's finding at the package's first added line.** `compare_dependencies`
  (`engine/src/checks/dependencies.rs`) gives each `NewPackage` and source-changed finding the first line the change
  added to its manifest that names the package (`line_naming`), not the line of that source's entry: `foo = "1"` and
  `foo = { git = ".." }` added to one Cargo.toml both point at the first. Found while implementing ticket
  cargo-dependency-rules/01 (2026-11-04). Closes with the readers giving each source the line of its entry.
- **Readers of `state.json` still take records an earlier release wrote.** Since octoflow-without-python ticket 04
  (2026-11-03) a `state.json` without the format marker is refused (`crate::state::format_problem`), so no record an
  earlier release wrote reaches them, but `ProofRun::read` (`engine/src/proofs.rs`) still reads a `proofs` record
  without `written` as a first run, `Restored::read` (`engine/src/workflow/locks.rs`) a `restored` record without
  `paths` as restoring nothing, and the turn and agent readers a record without `kind` as `DEFAULT_AGENT`'s. Each is
  a branch no format-2 run takes. Closes with those branches removed and the records' fields required, as
  `RunState`'s are.
- **`crate::map::Map` hand-writes an insertion-ordered map.** `engine/src/map.rs`, moved out of `py::json` by ticket
  octoflow-without-python/04 for the engine's maps of values that are not JSON (`Map<Dependency>`, `Map<String>`).
  Raised by cross-review 114 (F9, nit, 2026-11-03): "`indexmap::IndexMap` is already in the build through
  `preserve_order`." Using it needs `indexmap` as a dependency of the engine's own, which the user approves; closes
  with that approval and `Map` replaced by `IndexMap`, or with the user's ruling that `Map` stays.
- **serve's event stream can still miss a finished run's last files when they take longer than a poll interval.**
  `RunWatch::poll` (`engine/src/serve/runs.rs`) takes a finished run's files as complete once two polls one
  `SERVE_POLL` apart (1 s local, 5 s remote) list the same set. Raised by cross-review 121 (F3, should-fix,
  2026-11-03): "If the host commands between the finishing save and the last write (`workflow.rs` save →
  `write_summary` → `write_metrics`; likewise for `release`) take longer than one interval, two polls list the same
  incomplete set and the run is settled without summary.md/metrics.json until its record next changes. That is the
  original symptom with a narrower window. On a remote project, `write_summary`'s `round_results` reads go over SSH,
  so this is not out of reach." Closes with a settle rule that does not depend on the interval, such as settling
  only once the finishing save is older than a bound (the record carries `heartbeat_age`), or with the engine
  writing the files before that save where a takeover allows it (`end_done_run` writes them before its release
  save since run-crash-ending/01, but after the save of phase `done`; `release` saves first on purpose).
- **No test crashes the Rust engine after each save.** `tests/test_crash_resume.py` crashed the Python engine
  in-process right after every write of `state.json` and resumed it, and went with that engine (rust-engine ticket 11,
  2026-10-30). The e2e scenarios still kill the Rust engine as a program, once per phase. Closes with a crash
  harness on the engine's `FakeHost`, a `Host` whose write of `state.json` can stop the run, in
  `engine/src/workflow/tests/`.
- **No contract test talks to the real Jenkins.** `tests/contract/test_jenkins.py` drove the Python engine's `CI`
  against the live quality job, and went with it (rust-engine ticket 11, 2026-10-30); live-checks/01 (#131) asks for one
  against the Rust engine's `Jenkins`. Closes with that ticket. Its cleanup must stop the quality build it triggers,
  which the Python test's did not: a failure or a Ctrl-C there left the build holding an executor for up to the 22
  minutes build #59 took (cross-review 73, F11, 2026-10-27). The engine aborts no build either, so its `Jenkins`
  (`engine/src/ci.rs`) has no call for it: a POST to `<job>/<number>/stop`, or the queue item's cancel, once the
  `agents` user is known to hold Jenkins's Cancel permission on the job.
- **Test code outside the places the spec names is judged as code.** The spec of the Rust checks (requirement 2)
  places Rust's tests in a `tests/` directory, a `tests.rs` and an inline `#[cfg(test)]` module; `_in_rust_tests` and
  `_rust_test_path` in `gate.py`, which both Rust checks read, follow it (test-integrity's side of this is the
  out-of-line `mod name;` item in its list below). So `cognitive-complexity` judges a file that a parent takes in
  with `#[cfg(test)] mod name;`, as the engine keeps its fakes and `test_support.rs` (`engine/src/host.rs:24` on
  rust-engine): at rust-engine `476b770`, 3 of the 21 engine functions over the limit are theirs (`FakeState::git`,
  `FakeJenkinsState::answer`, `helper_cases`). Found by gate-rust-checks ticket 03 (2026-10-30). And, as cross-review
  108 put it (F4): "A file-level `#[cfg(test)] fn` or `#[cfg(test)] impl`, or a `#[test]` fn outside the places
  requirement 2 names, is judged against 15." Closes with the user's ruling on the spec's definition, and, if those
  are tests, the helper listing each out-of-line `mod name;` with its `cfg_test`, so that the gate can follow a
  declaration to its file, and a `cfg_test` of each fn's own and its impl's attributes.
- **A macro whose arguments are no expressions separated by commas adds nothing to a Rust fn's score.** The helper's
  `flow` reads a macro's arguments only where they parse that way (`FlowWalker::visit_macro` in
  `ci/gate-rust/src/main.rs`), as its `asserts` do. Raised by cross-review 108 (F3, 2026-10-30): "A macro whose
  arguments do not parse as comma-separated expressions (`vec![if a {..} else {..}; n]`, `matches!(x, P if a && b)`,
  `tracing::info!(target: "x", ..)`, `select!{..}`) is dropped silently, so its branches, loops and closures add
  nothing to the score." Clippy's own `cognitive_complexity` reads no macro either. Closes with the helper parsing the
  common other shapes (`expr; n`, `expr, pattern if guard`, `name: expr` arguments) and a `macro` node, unread, that
  gate.py can count or report.
- **Rust's test code takes no part in pairing a function with its base.** `_rust_scored_functions` leaves the fns
  `_in_rust_tests` places among the tests out on both sides, where a Python file under `tests/` stays in
  `_base_scores`' pool as a file the check does not judge. So a fn moved unchanged into Rust's tests cannot keep its
  own base before a copy of it elsewhere takes it, and that copy passes as the original. Found by gate-rust-checks
  ticket 03 (2026-10-30); since its merge with ticket 02 it holds for test files as for inline test modules. Closes
  with a per-function "exempt" mark on `_ScoredFunction` that `_BasePool` reads where it now asks
  `_complexity_exempt` of a path.

- **The Gate stage's comment on report order describes a window that has closed.** The Jenkinsfile passes the Rust
  reports before the Python's for a main whose `gate.py` took `--tests` and `--coverage` once; main's takes them
  repeatedly since #163, so the order and its comment can go. Raised by cross-review 88 (F9, 2026-10-28).

- **No agent has run an eval task yet.** live-checks/04's verification asks for one real `cargo xtask eval --tasks <one>
  --repeat 1` from a herdr pane (it starts real agents, so it was left to the user), with its row in the pull
  request's description and its result line committed under `evals/results/`. Cross-review 80 (F9, 2026-10-28):
  "Finding 1 is the kind of defect only a real run's output (or a test built from one) shows": a task text or a
  hidden test that misjudges real work shows only then. Closed by that run, and by fixing what its output shows.

- **A quality build now takes longer than `QUALITY_TIMEOUT`.** The contract test's one live run (live-checks/01,
  2026-10-27) triggered build #59 of `Octoflow-AI/py-octoflow-ai-quality` with `GIT_REF=main` (`f4628cc`): it ended
  SUCCESS after 1299 s, of which Test took 888 s, UI 229 s and Gate 172 s, so `tests/contract/test_jenkins.py` failed at
  what was then its 1200 s cap (it later waited for the build to end, checked every answer, and failed last on the time
  the quality file took, until it went with the Python engine), as a quality round of the engine would: `poll`
  (`engine/src/workflow/quality_rounds.rs`) gives up at `QUALITY_TIMEOUT` (1200 s, `engine/src/constants.rs`) from the
  round's start, and Gate had not begun by then. The suite grew by the parity scenarios (now `engine/e2e`), and PR-160's
  Test stage took 645 s. A resume gets a fresh timeout and adopts the running build, so a run is not lost, but a quality
  round of this repository whose build takes as long fails once, and is judged only after a resume. Not changed then:
  the Python engine was frozen until rust-engine ticket 11 replaced it, so a larger `QUALITY_TIMEOUT` is the Rust
  engine's to set. Narrowed by PR #162 (2026-10-27) for a quality round whose change leaves what the e2e scenarios
  depend on alone (Jenkinsfile, Test stage): its build leaves them out. A change that bumps the version touches
  `engine/Cargo.toml` and `Cargo.lock`, so its build still runs them, and so does the contract test's build of main
  whenever main's last commit was a releasing merge (build #59, before #162, ran them in any case); Gate's mutmut time
  is narrowed by `gate.py --test-contexts` (gate-open-decisions ticket 06), which costs the Test stage about 7 % more.
  `cargo xtask contract` (live-checks/01's Rust port, docs/quality-gate.md, "The calls") measures it again: its step 9
  says whether a quality round could have taken its build of main within `QUALITY_TIMEOUT`. It triggers a real build,
  so it is the user's to run. Closes when a quality build that runs the scenarios fits in `QUALITY_TIMEOUT`, shown by
  that step green against one, or with a larger `QUALITY_TIMEOUT` in the Rust engine.
- **The Rust engine reads some input no run writes otherwise than the Python engine.** Each is a difference only for a
  file a human edited, or a case no test of the Python engine pins (rust-engine ticket 04, 2026-10-28): a time is read
  only with its offset, as jiff's `Timestamp` reads an ISO 8601 time (`crate::summary::epoch_time`, since
  octoflow-without-python ticket 04), where Python's `fromisoformat` takes one without an offset in the machine's zone
  and reads hour 24 and week dates, so a session log's line or a `closedAt` of such a time is skipped or refused; and a
  turn whose start is not a time is left out of `summary.md` and `metrics.json`, where the Python engine fails
  (`epoch_seconds` and `turn_starts` in `engine/src/summary.rs`). And from the machine the agents run on (rust-engine
  ticket 05, 2026-10-28): a path git prints that is not UTF-8 is refused, naming it, by `Host::lock`, `locked_changes`,
  `restore`, `changed_line_counts` and `crate::patch::added_lines` (so by the secret scan too), where Python reads it as
  `os.fsdecode` does, and a base's gitleaks rules file that is not UTF-8 fails the scan closed, where the Python engine
  crashes writing it (`engine/src/host.rs`, `utf8_path`; `engine/src/host/scan.rs`, `base_rules_file`). And from the
  agents' session logs and herdr (rust-engine ticket 06, 2026-10-28): a session log's line that holds `NaN` or
  `Infinity` is skipped, as serde_json refuses it, and a count beyond 64 bits is not an integer, where Python counts its
  tokens, and a lone surrogate in a line, as an escape or as a byte that is not UTF-8, is read as U+FFFD, so that two
  Claude Code message ids that differ only there count as one answer, where Python counts two (`Harness::session_usage`
  in `engine/src/harness.rs`, `without_lone_surrogates`); a harness that is not one of `AGENT_KINDS`, as a hand-edited
  `state.json` can name, is refused naming those there are (`crate::harness::harness`), where the Python engine fails on
  a KeyError; and an answer of herdr's that lacks what its command gives, or is not UTF-8, is a `bad_response` naming
  the command or a failure naming it (`engine/src/herdr.rs`), where Python fails on a KeyError or a UnicodeDecodeError.
  And from the Workflow's locks and proofs (rust-engine ticket 08, part 4, 2026-10-29): a `restored` or `proofs` record
  of `state.json` without a field the engines write, or with one of another type, stops the run naming it
  (`Restored::read` in `engine/src/workflow/locks.rs`, `ProofRun::read` in `engine/src/proofs.rs`), where Python fails
  on a KeyError or a TypeError, or, for a `restored` record without `paths`, restores nothing, or, for a `proofs` record
  without `said`, reads it only where it needs `said` (no release wrote one: v0.24.0, the first with proofs, writes
  `said` in every record, and only commits before it on main did not; cross-review 98, F1); and a proof's output that is
  not UTF-8 is quoted with each such byte as U+FFFD (`Workflow::run_proof`), where Python keeps it as a lone surrogate,
  which `state.json` holds as an escape and the write of the review then fails on (a UnicodeEncodeError, uncaught). And
  from drain (rust-engine ticket 09b, 2026-10-29): a name in drain's DIR that is not UTF-8 is left out of the queue
  (`crate::cli::drain::queue`, `read_queue`), where Python reads its spec.md through os.fsdecode. And from the manifests
  the new-dependencies check reads (octoflow-without-python ticket 06, 2026-11-03): their values are compared and
  written as TOML (`crate::checks::dependencies`), so a `[tool.uv.sources]` entry's `index` names only an index whose
  `name` is the same TOML value, where Python's `==` took 1, 1.0 and true for one name and datetimes for one by their
  instant; a source of a shape the check does not know is written as an inline TOML table, one holding a datetime too,
  where Python's `json.dumps` refused it and the manifest was unreadable; and a `[project] name` that is not a string is
  refused, where Python took 0, false or an empty list for no name; and an index's `url` of false, 0, an empty list or
  an empty table names that index, where Python took it for no URL and named the index by the entry's `index`. Decided
  against unless a run is found that writes such input.
- **The Rust engine's Ctrl-C, what is left.** Rust-engine ticket 08's part 6 (2026-10-29) has a SIGINT raise the
  `Interrupt`, which stops the command the engine waits on (`crate::interrupt`), and since ticket 10 every command of
  the binary takes it. Not closed: (1) A
  Ctrl-C during a Jenkins or PyPI request (`crate::http`, ureq) waits for the request to end, up to its timeout, where
  Python's KeyboardInterrupt stops `urlopen` at once: there is no process to stop, and ureq has no way to abandon a
  request from another thread. The user accepted that wait for a Ctrl-C (2026-10-29, review 100). Since ticket 08's
  follow-ups a SIGTERM or a SIGHUP waits so too (cross-review 102, F4: "a `kill <pid>` or a service manager's stop
  during the quality gate's Jenkins polling or the dependency check's PyPI request waits up to the request
  timeout"), where it ended the Python engine at once; a service manager that sends SIGKILL after a shorter grace
  kills the engine with the run unreleased. Closes with a request the Interrupt can cut short (one whose reads are
  given slices of the timeout, the Interrupt checked between them, or one run on a thread the Workflow stops waiting
  for), or with the user's ruling that the wait is acceptable for SIGTERM and SIGHUP too. (2) Under macOS, `by_signals`
  reads no `/proc/self/status` and takes the kernel to discard a signal that was ignored when the engine started, even
  while blocked, as the BSD kernels do, so `serve`, which waits for SIGINT and SIGTERM also when it started with one
  ignored, as Python's `signal.signal` replaces an ignored disposition (ticket 10, `crate::serve::serve_command`),
  does not stop at one it started ignoring there, and `run_process` gives
  a command the whole 250 ms grace, nix having no `waitid` there; unchecked on a Mac. Closes with
  `engine/tests/signals.rs`, which is Linux's (it reads `/proc` and starts the engine with GNU `env`), ported to macOS
  and run on one. A process that left the command's group, as a daemon does, is not reached, as at
  a timeout; that is no gap, as the terminal's Ctrl-C does not reach one in Python either. (3) On a Linux whose
  `/proc/self/status` cannot be read, as in a container or sandbox without `/proc`, `ignored_signals` finds nothing
  ignored, so `by_signals` waits for a signal the engine started with ignored and takes it, Linux queueing a blocked
  signal whatever its disposition (cross-review 102, F2: "an engine under `nohup` in a container or sandbox without
  /proc is hung up: the Interrupt is raised, the run released as interrupted", and `interrupt::exit`'s raise of the
  still-ignored signal returns, so the program exits 129 by `std::process::exit`). Part 6 had the gap for SIGINT;
  the follow-ups extend it to SIGTERM and SIGHUP. nix reads a signal's disposition only through `sigaction`, which
  is `unsafe`. Closes with that query, the user's choice of a second `unsafe` block, or with refusing to start on a
  Linux without `/proc`.
- **The commands the Rust engine runs outlive an engine that is SIGKILLed.** Each command runs in a process group of its
  own (rust-engine tickets 05, 06), so a SIGKILL of the engine, or of its group, ends the engine and leaves a running
  `herdr agent wait`, `gh pr create` or push to finish unwatched, where the Python engine's children are in its group
  and end with it when the group is killed. SIGINT, SIGTERM and SIGHUP now stop them (`interrupt::SIGNALS`, rust-engine
  ticket 08's follow-ups, 2026-10-29); a SIGKILL cannot be caught. The e2e scenarios' kill
  kills each process below the engine too (`Invocation::kill`, `engine/e2e/tests/e2e/world.rs`), as "a crash or a lost
  machine" does; before, `kill_in_publish` against the Rust binary got a pull request from the orphaned `gh`. Closes
  with the user's ruling that this is acceptable, or with the command's group killed when the engine dies (Linux's
  `PR_SET_PDEATHSIG`, set in `run_process`'s `pre_exec`, reaches only the command itself, not what it starts).
- **The Rust engine talks to Jenkins and PyPI otherwise than urllib in a few cases.** Its transport is ureq
  (`HttpClient`, `engine/src/http.rs`, rust-engine ticket 07, 2026-10-28), which differs from `urlopen` where no test of
  the Python engine pins it: ureq reaches an http URL through a proxy with CONNECT, as it does an https one, where
  urllib sends the request itself to the proxy, so a proxy that allows CONNECT only to port 443, as squid does by
  default, refuses a plain-http Jenkins that `http_proxy` sends through it; TLS trusts the certificates of the places
  the environment names, else of the system's as openssl-probe finds them, each read by rustls-native-certs
  (`root_certs`), which trusts otherwise than OpenSSL does for Python in five ways (the user's rulings, 2026-10-28,
  cross-review 93, F22 and F23): with `SSL_CERT_FILE` set non-empty and `SSL_CERT_DIR` unset, or the reverse, it reads
  only what is set, where OpenSSL adds the system's default directory or file (with the other set empty, OpenSSL adds
  nothing either), so an `SSL_CERT_FILE` that cannot be read, such as a removed virtualenv's, trusts none where Python
  still trusts `/etc/ssl/certs`; a variable set empty counts as unset, which departs from OpenSSL only where the other
  is unset or empty too: with `SSL_CERT_FILE` empty and `SSL_CERT_DIR` unset the system's bundle is trusted too, where
  OpenSSL reads no file for it, with `SSL_CERT_DIR` empty and `SSL_CERT_FILE` unset the system's directories are, where
  OpenSSL reads no directory for it, and with both empty the whole system store is, or Mozilla's roots on a machine with
  no certificate, where Python loads no certificate at all; it reads every file of a certificate directory, where
  OpenSSL opens only the files named by a subject's hash, from `.0` up to the first missing or broken one, so a
  certificate under another name, or behind a broken link, is trusted here only; without either variable it takes the
  system's bundle and directories from openssl-probe's list of places, not from OpenSSL's compiled-in ones, also on
  macOS, where neither engine reads the keychain; and without either variable, on a machine with no certificate at all,
  it trusts Mozilla's roots (webpki-roots), where Python trusts none and fails every https request; and the timeout
  bounds the connect and each read and write, as urllib's does, but not name resolution. In what it reads
  (`engine/src/ci.rs`, `engine/src/pypi.rs`, `engine/src/checks/dependencies.rs`): an answer of Jenkins's of another
  shape than a queue item, a build's result, a list of builds or a test report is a `CIError` that may pass, where the
  Python engine fails with an `AttributeError`, and a queue item's `executable.number` that is not an integer is one
  too, where Python keeps it; a manifest is read as TOML 1.1, which tomllib before Python 3.15 refuses in part, and why
  one is not TOML is the toml crate's words with `(at line L, column C)`; and rustls refuses a server certificate that
  is a CA's own, as a self-signed one made by `openssl req -x509` with its default extensions is (`CaUsedAsEndEntity`),
  where OpenSSL accepts it when it is trusted, so the e2e scenarios' PyPI has a leaf under a CA of its own (found by
  ticket 08's part 7, 2026-10-29). Closes, for the proxy, with a transport that sends an
  http request through a proxy as urllib does, or with `no_proxy` naming Jenkins; the rest decided against unless a
  setup is found that needs it. Decided against (2026-10-28, ticket 07, design-it-twice): `CI` and `PyPI` as traits of
  their own with in-memory fakes. The engine's tests would then word every URL and error twice, in the code and in the
  fake, and ticket 08's Workflow tests, which read the requests Jenkins got (`FakeCI.names`, `requests`, `auth`), would
  port only by re-deriving them; with the seam at `Http`, as Python's at `urlopen`, `FakeJenkins` and `FakePyPI` play
  the servers and every line of `Jenkins` and `PyPI` runs in those tests. The user kept the `Http` seam and amended the
  spec's Lane to say so (2026-10-28, cross-review 93, F1), and issue 08 says which seam it is given.
- **A Rust test's own lines count toward the coverage of new code.** cargo-llvm-cov's report lists the lines of a
  `#[cfg(test)]` module as it lists the code's (docs/quality-gate.md, "Rust tests and coverage"), and a passing
  test runs them all, so a change that adds a Rust function and a test of it is judged on both: 15 untested lines
  beside a 10-line test read as 40%, not 0%. coverage.py's report of the Python has only `gate`.
  Found while adding Rust coverage (rust-engine ticket 03, 2026-10-27). Closes with test modules left out of the
  report, by `#[coverage(off)]` once it is stable or by the tests moving to files that `--ignore-filename-regex`
  can leave out, or with `gate.py` skipping the lines a `#[cfg(test)]` item spans. The Rust engine's tests are in
  such files already (`engine/src/**/tests.rs`, `engine/src/prompt_parity.rs`, `engine/src/test_support.rs`); the
  desktop shell's are a `#[cfg(test)]` module in `ui/src-tauri/src/lib.rs`. That module also makes a change to the
  desktop shell's tests alone need a version raise, and a place in the one-bumping-PR-at-a-time queue:
  `ci/version-raised.sh` exempts `**/tests.rs`, not `lib.rs` (cross-review 117, F4, 2026-11-03). Moving it to
  `ui/src-tauri/src/tests.rs` closes both: tried by ui-test-text-file-busy/01 (2026-11-03) and dropped then because
  the gate failed the move on the tests' own `cfg`s (next entry). Their `cfg`s all name platforms, which the gate
  now weighs rather than fails (`platform-only`, gate-platform-cfg/01), so the move is open again.
- **The gate takes a moved Rust test's own skip for one the change added.** `_rust_skips` (`gate.py`) reports a
  skipping attribute on any line the change added, so a test moved to another file with its attributes is reported
  though nothing new is skipped: a platform `cfg` (`#[cfg(unix)]`) as a `platform-only` finding to weigh, any other
  (`#[ignore]`, `#[cfg(feature = "slow")]`) as a `test-integrity skip` error; the `removed-test` check follows the
  same move by the test's `body_digest` (docs/quality-gate.md, "Rust tests"). Found 2026-11-03 moving the desktop
  shell's 26 tests out of `ui/src-tauri/src/lib.rs` (ui-test-text-file-busy/01): six errors, one for each `cfg` the
  tests carried, which since gate-platform-cfg/01 (2026-11-04) are six findings to weigh. Closes with a skip
  attribute on a test matched to the base's test of the same body digest counted only when that test did not carry
  it.
- **A misread literal on an added attribute line can report a test's existing skip as added.** `_rust_attribute`
  (`gate.py`) lexes only plain strings, so a char literal (`'"'`) or a raw string ending in `\` (`r"C:\"`) can leave
  an attribute open until a `]` inside a later string, many lines down; `_rust_line_attributes` then reads every `#[`
  on the lines in between as the added line's. So a change that adds only `#[doc = r"C:\"]` above an existing
  `mod tests` with an `#[ignore]`d test whose body holds `"a]b"` fails the build with a `test-integrity skip` error
  naming that `#[ignore]`. It fails loudly and hides nothing. Separately, `_rust_line_attributes`'s "cannot hide a
  skip after it" holds only on the line where an attribute starts: a misread literal on a wrapped attribute's
  continuation line can still hide a skip later on that line, as `#[cfg(miri)]` in `doc = "x")] #[cfg(miri)]` after
  `#[cfg_attr(any(), doc = ']',`. Raised by cross-review 136 (R5-F1, should-fix) and deferred by the user
  (2026-11-04). Closes with either of the reviewer's fixes: read the lines in between only when the attribute's text
  holds no `"` or `'`; or bound how far an attribute may span, to the lines before the next one that starts with `#[`
  or an item keyword.
- **No build runs the Rust doctests.** cargo-nextest does not run doctests, and since the Test stage's `cargo llvm-cov
  nextest` replaced the UI stage's `cargo test`, nothing does: `octoflow_ui_lib` is an `rlib` among its crate types
  (`ui/src-tauri/Cargo.toml`), so `cargo test` ran them. The crate has no doc example today. Raised by cross-review 72
  (F3, 2026-10-27): "a doc example added later that fails to compile or asserts wrongly passes CI unnoticed".
  Decided against for now: a `cargo test --doc` beside it, which builds the dependencies a third time, uninstrumented,
  under the agent's 3 GB, for no example. Closes with `cargo test --doc --locked` in the Test stage once a doc example
  lands, or with cargo-llvm-cov's `--doctests` once it runs on a stable toolchain.
- **Jenkins never runs `ci/gate-rust`'s own tests.** The helper is a package of its own, excluded from the top-level
  workspace (`Cargo.toml`), whose tests the Test stage runs, so its `cargo test` (`ci/gate-rust/tests/schema.rs`) runs
  only by hand; its added lines are judged by clippy but count as nothing to cover. `tests/test_gate.py` builds the
  helper and runs it over Rust snippets, so the Test stage exercises its `flow` (gate-rust-checks ticket 03,
  2026-10-30), but not its own tests. Narrowed by rust-engine ticket 04, after which Jenkins no longer ran the Rust
  tests of `ui/src-tauri` alone, and the switch's merge of main (rust-engine ticket 11, 2026-10-30), which excluded the
  helper. Closes with the Test stage also running `cargo nextest` in `ci/gate-rust` and passing its report to the gate.
- **`GateChange.rust_files` starts one `git cat-file` per base `.rs` file.** `_rust_paths` in `gate.py` copies each
  base side out of git with its own process, so a change to N Rust files starts N git processes before the helper's
  one run; about 5 ms each here. Raised by cross-review 106 (F5, 2026-10-30): "one `cat-file --batch` or `git archive`
  would do." Closes with one `git cat-file --batch` over all the base paths.
- **test-integrity's Rust detector does not see some weakenings that Python's does, or that its rules name.** Found
  while writing it (gate-rust-checks ticket 02, 2026-10-30); docs/quality-gate.md, "Rust tests", lists each:
  - an expected value held in a `const` or `static` the change edits: `_changed_constants` follows Python's
    module-level constants, and the Rust side has no counterpart. Closes with the helper listing a file's `const`s
    and `static`s with a digest of their expressions, and `rust_weakened_tests` comparing those an assertion names;
  - a `Result` thrown away that holds no assertion macro and no `catch_unwind`, such as `let _ = check();` where
    `check` returns the `Err` that should fail the test, and a `catch_unwind` result bound to a name and never read.
    The helper knows no types, so `_rust_swallowed_errors` cannot tell such a value from a cleanup's. Closes with a
    rule on the text the change replaced, such as a `.unwrap()`, `.expect(...)` or `?` at the base on the same
    expression, which the helper would need to report;
  - a skipping attribute that does not start its line, or a multi-line `cfg_attr` whose `ignore` is on a later line,
    in a test's body or on a module (`_rust_skips` reads those lines' text; a test's own attributes come whole from
    the helper). Closes with the helper listing the attributes of modules and of statements in a test, which raises
    its schema.
  - a test module in a file of its own that a `#[cfg(test)] mod name;` names, where that file is neither under
    `tests/` nor `tests.rs`, as `mod cli_tests;`: `_rust_test_file` reads a path alone, as requirement 2 of the spec
    defines the files, so that file's tests are not read and its edits count as code. Raised by cross-review 107
    (2026-10-30, not recorded). Closes with the helper listing each `mod name;` with its `cfg_test`, and the check
    resolving the file it names as rustc does (`name.rs` or `name/mod.rs` beside the file, or a `#[path]`).
  - removing the first of two tests of one name in one file, as under two platforms' `cfg`s: `_RustTest.namesake`
    numbers them in order, so the second then takes the first's number and is compared with the removed one's base,
    which reads as a fewer-assertions or a changed literal on it, or as nothing, rather than as a removed test. Raised
    by cross-review 107 (round 2, not recorded). Closes with namesakes matched by their `cfg` attributes, or by body
    first as removed tests are.
- **The e2e scenarios record `state.json` at each command the fakes get, not after each save.** Ticket
  rust-engine/01 asks for "`state.json` after each save"; the ledger reads it only when herdr, gh, Jenkins or PyPI
  gets a call, and leaves out a change to `heartbeat_at` alone (`engine/e2e/src/ledger.rs`, `changed_states`). Raised
  by cross-review 65 (F2, 2026-10-27): "a Rust engine that skips or reorders saves between two of those calls, or
  writes a wrong intermediate state that a later save overwrites, still passes." Only the engine's own process sees
  every save: for the Python engine an audit hook through `sitecustomize`, or a `mv` on PATH that `Host.write` calls,
  would catch them, and the Rust engine could be held to neither. Closes with the user's ruling on the ticket's
  wording, or a way to see each write of `state.json` from outside the process (inotify has no stdlib binding).
- **serve answers some requests otherwise than Python's http.server.** The Rust engine's serve is on tiny_http
  (rust-engine ticket 10, `engine/src/serve.rs`), which reads a request otherwise in a few cases no client of serve's
  sends: a request line or header with a byte beyond ASCII gets no answer, its connection closed, where Python reads
  it as Latin-1 and answers, a token so 401 (`non_ascii_tokens`); a header's value loses its trailing blanks, so a
  `Host` or `Authorization` header with one passes where Python's fails it; a header continued on a folded line is
  refused with 400, where Python joins the lines; a netloc with one square bracket in an absolute request target is
  read as it is (`crate::serve::url::urlsplit`), where Python's handler fails on urlsplit's ValueError and answers
  nothing; tiny_http answers a bad request line, an `Expect` other than `100-continue` and HTTP/2 itself (400, 417,
  505), and neither those nor the 501 to another method are logged on stderr, where http.server logs each error it
  sends; and a reply names tiny_http in its `Server` header. An event stream to an HTTP/1.1 client is chunked, ending
  with its last chunk, where Python's HTTP/1.0 stream ends with the connection: tiny_http keeps the connection, so a
  close-delimited stream would not end for a client at a server's stop. The e2e scenarios' `EventStream` reads the
  chunked stream chunk by chunk (`engine/e2e/tests/e2e/client.rs`). Every security check refuses
  what Python's refuses; these change only how. Decided against unless a client is found that sends such a request.
- **serve has no limit on its threads, and no timeout for a client that sends nothing.** The engine's serve answers
  each request on a thread of its own, and tiny_http keeps a thread for each open connection, kept alive between
  requests (`engine/src/serve.rs`); cross-review 105, F3: "A client that sends a partial request or idles holds it for
  good." Decided against for now (2026-10-29, the user's ruling): serve binds 127.0.0.1 only and every request needs
  the token. Reopen if serve listens beyond loopback.
- **`Host::read` and `Host::read_inside` fail for a file removed between their test and their read.**
  `[ -f "$1" ] || exit MISSING; cat -- "$1"` (`engine/src/host.rs`, `fn read`) answers a file that goes away between
  the two with an error, not with None; `fn read_inside`, which serve's `/file` and `ProjectRuns::agent` read by,
  has the same test and cat, so `/file` for a file `run_files_inside` just listed, such as the secret scan's
  `leaks-publish.json`, and `ProjectRuns::agent` for a run being pruned report a failure where "missing" is the
  answer (one removed while its links are resolved is already Missing, cross-review 125 F3). Raised by cross-review 120 (F3,
  2026-11-03), against serve-run-files/03, which made the two listings skip such a file and left `read` out of its
  scope. Closes with a cat that fails on a file no longer there answering None, in both, and a test that removes the
  file between the two.
- **serve's link checks race the agent.** `Host::read_inside` (`engine/src/host.rs`) resolves a run file's links in
  sh, then cats the resolved path in another process, `run_states_inside` does the same with a run's state.json,
  and `run_files_inside` lists a file it checked a moment before; plain sh has no open that refuses to follow a
  link. Cross-review 125, F1: "an agent loops, swapping
  `runs/<id>/x.md` between a regular file and a link to `~/.ssh/id_rsa` … A `/file?name=x.md` that finds the
  regular file at the check and the link at the open returns the key with 200." Left open: an agent that can race
  the check can as well copy the file into its run directory, which serve shows as any other; the check is against
  a link left there, not against an agent that means to show the app a file. Closes with an open that does not
  follow links (a helper on the host with `O_NOFOLLOW` per component), if a reason is found to want it.
- **The e2e scenarios compare serve's event stream at checkpoints, not event by event.** `serve_endpoints_and_events`
  keeps, at each point where the run waits on an agent and once it is done, the last `record` and `files` event of
  each run (`engine/e2e/tests/e2e/scenarios.rs`, `last_by_run`). Raised by cross-review 65 (F10, 2026-10-27), against the
  spec's "serve's JSON and SSE events" byte for byte: "missing, repeated or reordered intermediate events pass." How
  many events a step sends depends on where serve's one-second poll falls among the run's saves, so the stream
  between two checkpoints differs from run to run. Closes with a serve whose poll the harness can step, which would
  be a hook in the engine, or the user's ruling that the folded stream is the contract, as the desktop app reads it.
- **Two e2e scenarios once saw a turn end a second before it started.** In one `cargo nextest run --workspace` of
  octoflow-without-python ticket 03's branch on a loaded machine (2026-11-03), `agent_exits` recorded a turn's
  `"seconds": -1` in `state.json` and `workflow_product_team` a `-<duration>` in its turns table, where their
  recordings hold a duration of 0 or more; both passed when run again, and in the two full runs before. `no_pr_run`
  did the same once on setup-checks ticket 02's branch (2026-11-03, its spec turn's `"seconds": -1` in
  `metrics.json`), and passed in three reruns. A likely cause: `end_turn` (`engine/src/workflow/turn.rs:525`–`531`)
  takes a turn's end from the handoff file's mtime and its start from `host.now()` (`turn.rs:165`), two clocks read
  at whole seconds, so a file written in the second the turn starts can end it a second before it began. Closes with
  that cause confirmed and the turn's seconds taken from one clock, or clamped at 0.

- **`test-integrity` knows golden files only by this repository's layout.** `_golden_file` (`gate.py`) takes a
  golden file to be one under a `golden` directory inside a `tests` directory, as `tests/golden.py` keeps them, so
  another project's expected output under another name, such as `__snapshots__/`, rewritten alongside the code it
  pins gets no `expected-value-changed` finding. Left by `_changed_golden_files` (2026-10-28), which
  gave golden files that finding. Closes with the golden directory a setting, beside the test-file patterns of
  `.scratch/gate-split/issues/04-configurable-test-files.md`, which does not name it yet.

- **A quality build of a snapshot from before `gate.py` fails mypy.** A quality build judges by main's rules
  (`--config`), and since the split main's `[tool.mypy] files` names `gate.py`, which mypy reads relative to the
  judged tree; a run whose base predates the split has no `gate.py`, so mypy stops with `gate.py: error: Cannot read
  file` (status 2) and the gate is ERROR on a failed condition the Builder cannot fix. mutmut's `source_paths`
  names it too, so surviving-mutants is `not-run` there. Checked by hand against such a tree (2026-10-27). Affects
  only runs started before the split merged that reach a quality round after it; closes by itself once none is
  left, or with the gate passing mypy only the listed files the tree has. Cross-review 61 (F1, 2026-10-27): "the
  gate is ERROR on a failed condition the Builder cannot fix", against the spec's "a quality build is judged by
  main's gate and main's rules".
- **`BUILD_CONFIG_FILES` does not list `gate.py`.** A pull request or main build runs its own tree's `gate.py`, so a
  change that loosens the gate gets no "Build configuration" notice in its quality file; `orchestrator.py`, which
  held the gate, was not listed either. Raised by cross-review 61 (F7, 2026-10-27). Closes with `gate.py` listed,
  with a notice of its own wording (a quality build runs main's gate, not the edit).
- **The quality file's format has two names for each part.** The engine's copies (`QUALITY_OK`,
  `QUALITY_FAILED_CONDITIONS`, `QUALITY_FINDINGS_LIMIT` in `engine/src/quality.rs`, `Finding::numbered`,
  `Finding::warning` and `Finding::error` in `engine/src/checks.rs`, `shortened` in `engine/src/error.rs`) and
  the gate's (`GATE_OK`, `FAILED_CONDITIONS`, `QUALITY_ISSUE_LIMIT`, `_numbered`, `_finding`, `_clip`) differ, so a
  grep for one misses its twin. Most date from gate-split ticket 01. Raised by cross-review 61 (F9, 2026-10-27).
  Closes with one name per part on both sides, a change to both programs.
- **surviving-mutants takes only the top directory's pytest configuration.** Since `_pytest_groups`, flaky-tests and
  passes-at-base run each changed test under the configuration pytest finds for it below the copy's top, but
  `_pytest_config`, which surviving-mutants gives mutmut, still looks in the top directory alone, so a project whose
  configuration is in `tests/pytest.ini` or `pkg/pyproject.toml` runs its mutants without that configuration's
  `pythonpath`, `addopts` and plugins. Raised by cross-review 35 (R1-6). Closes with mutmut's runs taking the changed
  functions' tests' configurations, which mutmut's one `pytest_add_cli_args` makes one per run at most.
- **flaky-tests' reruns get one time limit a run, whatever the number of configurations.** Each pytest run of
  flaky-tests has `FLAKY_RUN_TIMEOUT` of its own, and since `_pytest_groups` a change whose tests fall under several
  configurations collects and reruns once for each, so its worst case grows with them (3 configurations and 5 seeds:
  3 x 6 x 30 s), against `QUALITY_TIMEOUT`. passes-at-base's groups share its one budget. Raised by cross-review 35
  (R1-5), which reads the spec's "the per-group runs share the check's time budget" as meant for passes-at-base,
  flaky-tests having none. Decided against (2026-10-27,
  `.scratch/gate-open-decisions/issues/01-shared-time-budget.md`): a time budget shared by the reruns, their groups
  and mutmut; the 30 s limit and mutmut's 600 s stay, and `not-rerun` showing up often would be a question of its own.
- **SSH connection sharing has not been timed on a real `--machine`.** `ssh_sharing_options` was measured against
  a user-level sshd on this host only, 20 commands in 0.24 s shared against 1.78 s unshared; no herdr machine is
  saved here, so `doctor --machine` was not timed before and after
  (`.scratch/doctor-ssh/issues/01-ssh-connection-sharing.md`, requirement 4). Raised by cross-review 33 (F7). Closes
  with `time orchestrator doctor --machine <m>` on the herdr machine, before and after, both times written into
  that ticket.
- **`--machine` over SSH assumes a POSIX login shell and a server that takes several sessions.** Since `run_tool`
  (`engine/src/cli/doctor.rs`), doctor takes a remote command's exit status 127 for not on PATH: a csh or tcsh login
  shell exits 1 ("Command not found."), so a missing gh reads as `gh auth status fails` and a missing git as a failed
  `--version`; and a tool on PATH that exits 127 itself, such as a shim whose target is gone, reads as not on PATH. And
  `ssh_sharing_options` overrides a target's own `ControlMaster no`, so a server with `MaxSessions 1` refuses the second
  of two commands run at once on the shared connection (the desktop app and `serve` run several), which ssh then fails.
  Raised by cross-review 33 (F4, F6), neither reproduced. Closes with doctor asking through `sh -c`, as `on_path` does,
  and with an explicit `ControlMaster`/`ControlPath` in `ssh -G <target>` left in place.
- **`TestGate.test_a_missing_tool_fails_naming_it` fails where shellcheck lives beside git.** `TestGate.setUp`
  (`tests/test_gate.py`) puts git's own directory on `PATH` after the stubs, so on a machine with
  `/usr/bin/shellcheck` (agentbox, Debian's package) removing the stub still finds the real one and the gate passes.
  Seen 2026-10-25 on `main` at `9448612`. On such a machine it also fails mutmut's stats run, which runs every test,
  so `surviving-mutants` is `not-run` there, "failed to collect stats" (2026-10-27). Closes with `PATH` holding a
  directory with only a link to `git`.
- **unexpected-paths reads only the contract step's file.** `unexpected_paths` judges each path the change touches
  against `RunChange.spec`, the contract step's file, so a workflow whose contract is in business terms, such as
  `examples/workflows/product-team.toml`, whose requirements name no file and whose `design.md` lists the
  components, gets a finding to weigh for every path on every quality round. Raised by review 20 of
  product-team-workflow (F4, 2026-10-25): "the gate reads file names from the spec and this spec is in business
  terms by design". The example's header says so. Closes with the check reading every non-editing step's file that
  precedes the first editing step, or with a workflow key naming the file that holds the paths.
- **Removing a tree of read-only directories is written out twice.** `chmod -R u+rwX "$dir" && rm -rf "$dir"`,
  with a warning when it fails, is in the Jenkinsfile's Test stage (emptying `TMPDIR` before the tests) and in
  `packaging/build-desktop.sh`'s `remove_temporaries`, for the AppImage extractions it removes; a fix to one does
  not reach the other. `engine/src/proofs.rs`'s `REMOVE_TREE_SCRIPT` is a looser third (`rm` runs even when `chmod`
  fails, with no warning). Raised by cross-review 132 (F6, nit, 2026-11-04): "Record or share it, as the implementer
  judges". Closes with one script both call, should a third copy be needed or the idiom change.
- **Ctrl-C during `packaging/build-desktop.sh`'s AppImage smoke test leaves `serve` running.** `smoke_appimage`
  starts the AppImage's `serve` as a background job, which a non-interactive shell starts with SIGINT ignored, and
  the INT trap kills no one, so the AppImage and the orchestrator it execs outlive the script; the trap has removed
  the smoke test's directory they run from by then. A SIGTERM to the process group (a Jenkins abort) does reach
  them, and Jenkins runs with SIGINT ignored, so this is interactive use only. Older than desktop-packaging-gaps/03,
  whose traps kill no one either. Raised by cross-review 132 (round 2, not recorded there, 2026-11-04). Closes with
  the traps killing `serve`'s processes, as `smoke_fail` does, before removing the directory.
- **`packaging/build-desktop.sh`'s macOS smoke test leaves its directory when the script is killed by a signal.**
  `smoke_dmg` removes its `mktemp -d` only in its `smoke_fail` and on success, not through `remove_temporaries`
  as `smoke_appimage`'s since desktop-packaging-gaps/03, so Ctrl-C or SIGTERM leaves it, with the .dmg possibly
  still attached at its `mnt`. It holds no AppImage extraction, so it is small. Raised by cross-review 132
  (round 2, not recorded there, 2026-11-04). Closes with `smoke_dmg`'s directory, and the detach, in
  `remove_temporaries`, checked on a Mac.
- **The Package stage still runs programs it wrote to `/tmp`, kept out of agentbox0's Falco mail only by path
  exceptions that let any binary placed under those paths run unreported.** The Test and Gate stages set `TMPDIR` to
  `$WORKSPACE_TMP/tmp`, on the agent's volume, which the rule cannot see (the Jenkinsfile's Test stage says why),
  but Package keeps `/tmp`. There the desktop AppImage that `packaging/build-desktop.sh`'s smoke test runs under
  `APPIMAGE_EXTRACT_AND_RUN` extracts itself into `$TMPDIR/appimage_extracted_<hash>`, below the test's `mktemp -d`,
  and runs from there; linuxdeploy and appimagetool extract into the script's own `build/desktop/tmp` since
  desktop-packaging-gaps/03. The homelab excepts those paths (falco/rules/homelab.yaml, `appimage_extracted`). Each
  stage now creates its own TMPDIR, so a stage added later has to copy both lines. Raised by cross-review 71 (F1,
  F3: "set TMPDIR for every stage that runs what it builds (one pipeline-level definition next to CARGO_HOME would
  do)"). Closes with that pipeline-level `TMPDIR`, created before any stage that uses it, and a Package stage seen
  to pass with it on agentbox0. The homelab can then drop `appimage_extracted`.
- **No build has yet been seen waiting for `ci/cargo-lock.sh` on the Jenkins agent.** PR #88's builds ran both
  cargo steps through it on agentbox0 (builds 1-3, 2026-10-25), so the agent image has `flock` and the lock file's
  directory is writable, and a quality-style build (`GIT_REF` set) ran `--no-wait`; but the two run side by side
  reached their cargo steps 15 s apart, the lock free each time, so the wait and its message were seen only on the
  development host. Raised by cross-review 25 (F3). Closes with two builds whose cargo steps overlap on agentbox0,
  one saying that it waits for the lock and both passing.
- **Is 3 GB the right memory limit for the agentbox0 Jenkins agent?** All builds on the node share its container's
  `mem_limit: 3g` (Homelab `jenkins/agentbox0.yaml`), which this repo cannot change. Since `ci/cargo-lock.sh`, two of
  this repository's builds compile Rust at once only when a quality build starts its `cargo llvm-cov nextest` or
  `cargo check` beside another build's cargo step, or when the Gate stage's clippy, outside the lock, checks the desktop
  shell's own crate, the one crate the UI stage's `cargo check` leaves it; but everything else they run (pytest, mutmut,
  npm) still shares the 3 GB beside a locked rustc, as do other jobs' builds on the node. Open in
  `.scratch/ci-cargo-memory/issues/02-agent-container-memory.md`; closes with the Homelab keeping or raising it.
- **Under the gate's `--cap-lints warn`, an `#[allow]` overrides a workspace's `forbid`.** `Clippy._lint_workspace` in
  `gate.py` caps every lint at warn so that a lint at deny cannot stop cargo before a compile error in a later crate
  (cross-review 66, F1); rustc's cap also lowers `forbid`, so an added `#[allow(unsafe_code)]` under `unsafe_code =
  "forbid"` no longer gives E0453 and its unsafe block is no finding: the gate is OK on a change the workspace's rules
  make uncompilable, and only "Suppressions added" lists the allow, and not even that when the change adds the lint as
  one line inside an existing wrapped `#[allow(\n    a,\n    b,\n)]`, since `_RUST_SUPPRESSION` reads one added line at
  a time (F15). Nothing in this repository sets `forbid` (the top-level `Cargo.toml`'s `[workspace.lints]` are all
  `warn`), and the Test stage's uncapped `cargo llvm-cov nextest` and the UI stage's `cargo check` would still fail
  such a build of the workspace.
  Raised by cross-review 66 (F11, 2026-10-27): "The gate is OK on a change that the workspace's rules make
  uncompilable". Closes with a second, uncapped clippy run whose rustc errors (codes `E` and four digits, such as E0453)
  are compile errors, or with the forbidden lints read from the manifests and an added allow of one failing the gate.
- **A quality build judges Rust by the snapshot's own clippy rules.** `Clippy` in `gate.py` chooses no lints (it caps
  their levels at warn, `--cap-lints`), so clippy reads each workspace's `[lints]`, `clippy.toml`, `.cargo/config.toml`
  rustflags and `rust-toolchain.toml` from the judged tree, where a change can loosen them; `--config` replaces only
  `pyproject.toml`. The quality file lists such an edit under build configuration (`CLIPPY_CONFIG_NAMES`), so the
  Reviewer sees it, but the gate does not prevent it. Found implementing rust-engine ticket 02 (2026-10-27). Closes with
  the gate taking those files from the commit `--config`'s rules come from, or with the Reviewer's prompt naming the
  listing as a finding to judge.
- **herdr 0.9.1 lists no `gemini` integration.** `herdr integration status` on this host lists `antigravity-cli`
  (whose hook is under `~/.gemini/`) but no `gemini`, so `doctor` warns that it does not list gemini, and the
  README's `herdr integration install gemini` may name a kind herdr no longer has. Found by the doctor run
  (2026-10-24); not checked whether `agent start --kind gemini` still works. Closes once `AGENT_KINDS` and the README
  name the kind herdr installs for Gemini, or herdr is shown to accept `gemini`.
- **The desktop app's setup dialog shows doctor's fixes in the CLI's words.** `check_setup`
  (`engine/src/cli/doctor.rs`) words its fixes for a command line: "run from a herdr pane, or pass --machine", "pass
  --cwd a git checkout". `/setup` (`engine/src/serve/setup.rs`) passes them on as they are, so the dialog
  (`ui/src/components/SetupDialog.tsx`) names flags where the app has the project bar's Machine and Project folder
  fields. Found while porting setup-checks ticket 02 (2026-11-03). Closes with the two fixes worded for both, or
  check_setup told which it is wording them for.
- **No test reaches the setup marker's real path.** serve's tests build their `Setup` with `Setup::new` and a marker
  path of their own (`engine/src/serve/tests.rs`); the e2e scenario that starts the real serve, and with it `Setup::of`,
  never asks for `/setup` or `/setup/checked`; and `path_follows_xdg_config_home` (`engine/src/env/tests.rs`) asserts
  every `ConfigDir` accessor but `setup_checked_path`. So `setup_checked_path` returning an empty path, or one outside
  the configuration directory, fails no test, and the dialog would open on every start or the marker go elsewhere
  unnoticed. Found on setup-checks ticket 02's branch (2026-11-03). Closes with `setup_checked_path` asserted beside the
  other accessors in that test.
- **`doctor` checks gitleaks's `git` subcommand, not its version.** README asks for gitleaks 8.19 or later; the
  check reuses `Host.gitleaks_problem`, the run's pre-push check, which asks `gitleaks git --help` because a
  distribution's build may print no version number. So `doctor` shows no gitleaks version. Kept by choice
  (2026-10-24).
- **A spec left `ready-for-human` by a conflict stays there when `refresh` finds its pull request merged or closed.**
  `refresh_conflict` records `conflict_cleared` only for an open pull request; one merged or closed meanwhile has only
  its conflicts forgotten, so a later drain does not ask gh about it, and a `ready-for-human` spec whose pull request
  `refresh` found merged is never marked `done`. drain's own refresh of the same run does mark it `done`
  (`settled_header`, step 3). Raised while implementing drain-refresh-cleared ticket 01 (2026-11-04). Closes with
  refresh recording the cleared conflict whatever the pull request's state, and drain following that record's pull
  request for a `ready-for-human` spec.
- **Proofs run after the last round's review too.** No Builder turn follows to read them; only the draft pull
  request's review shows them, and each may take `PROOF_TIMEOUT`. Kept: a demonstrated defect is what the human
  most needs to see in a draft. Raised by the same review.
- **An in-place run puts back what the human changes in the checkout while a proof runs.** `Workflow::run_proof`
  snapshots the working tree before each proof and `Host::restore_tree` puts back every path that differs afterwards,
  whoever changed it. Raised by cross-review 143 (F2, 2026-11-04) against proof-runs/03: "In an in-place run the
  working tree is the user's checkout. A file the human saves during a proof is overwritten. A non-ignored file they
  create is `rm -f`'d. Neither gets a warning." The cost of the close the user chose for proof-runs/03 (undo the
  proof's writes); a worktree run is not exposed, since nobody else edits its tree. Closes with proofs running in a
  scratch worktree of the tree under test, or with the run warning, and keeping the human's version, when a file it
  would put back changed after the proof had ended.
- **Each proof snapshots the whole working tree twice.** `Workflow::run_proof` takes `Host::tree_snapshot` before the
  proof and `Host::restore_tree` another after it, each a `git add -A` of every untracked, non-ignored file under
  `DEFAULT_TIMEOUT` (60 s): on a tree with large untracked files a snapshot over the limit makes every proof "did not
  run" or "did not run cleanly". Raised by cross-review 143 (F9, nit). Kept: one snapshot before a batch of proofs
  (`judge_proofs`, `rerun_proofs`) would save one of the two, but changes `run_proof`'s interface and every test that
  calls it, and leaves the limit in place. Closes with that, and with a snapshot timeout that scales, or the snapshot
  taking only the paths `git status` reports changed.
- **`expected-value-changed` misses an inline literal's sign, type or structure.** `_changed_literals` pairs
  assertions by `_shape` and compares `_literals`, which drops types and operators: `assert f() == -5` becoming
  `== 5` changes the shape (no pair, no finding), and `== 1` becoming `== True` gives equal literal lists
  (`[1] == [True]`). Raised by the cross-review of `test-integrity-constants` (F4, 2026-10-27): "`_literals`
  equality misses sign, type and structure changes"; the constant path compares `_tree_fingerprint`s instead.
  Closes with the inline path comparing typed literals and pairing a unary minus with its operand.
- **A run that is never closed keeps its local refs.** A locking step's commit stays under
  `refs/orchestrator/<key>/locked/` (`RunState::lock_ref`) until the run ends `done` (`Workflow::clean_up_locks`) or
  is closed (`close_run`, `engine/src/cli/close.rs`). Left: the refs of a failed or stale run nobody closes, of one
  closed by a release whose `close` left them, and of one whose run directory was deleted by hand, which `close`
  cannot find; their locked snapshots are never collected by `git gc`. `git for-each-ref refs/orchestrator/` lists
  them, and `git update-ref -d` deletes one. Decided against on 2026-10-25: `prune` deleting them, and a sweep of
  `refs/orchestrator/<key>/` whose run directory is gone.
- **An agent's pull request can reach `main` on rules it wrote itself, with no human.** Jenkins's App reports
  `continuous-integration/jenkins/pr-head`, which the agents' token cannot post, but a pull request build runs
  the pull request's own `Jenkinsfile` and gate (`docs/quality-gate.md`, What the Gate stage judges), and since
  `automerge.yml` a pull request into `main` merges once that status is green. Raised by the cross-review of
  `pr-automerge` (F5, 2026-10-24): "A branch the agents push under any other name, once a pull request is open on
  it, merges into `main` (and so into a release) without a human". The orchestrator's own pull requests, under
  `orchestrator/`, were held for a human until the user decided on 2026-10-24 that every pull request of this
  repository merges on a green build; so a run's change now reaches `main` and a release unreviewed too. Closes
  with an approving review required on `main` that the agents cannot give (Homelab `.scratch/agent-identity`,
  ticket 03), or a Jenkinsfile and gate a pull request build takes from `main` rather than from the pull request.
- **A pull request moved off `main` can merge unchecked before `automerge.yml` turns auto-merge off.** No ruleset
  requires a check outside `main`, so once a pull request with auto-merge on targets another branch and has no
  conflict, GitHub merges it at once, racing the run that would turn auto-merge off; and a conflict cleared from
  the base side starts no run. Raised by the cross-review of `pr-automerge` (R3-1(b), 2026-10-24): "After the push
  that resolves the conflict, the PR is CLEAN against a base with no required check, so GitHub merges it at once
  and unchecked", "In (b) that run still races GitHub's immediate merge. A conflict cleared by a change to the base
  rather than the head raises no pull-request event at all." Closes with a ruleset that requires the check on
  every branch, or one that restricts merges outside `main`.
- **The secret scan does not look inside a change git diffs as binary.** Neither gitleaks nor `allow_comments`
  (`engine/src/host/scan.rs`) reads a change `git log -p` shows as "Binary files ... differ"; which changes those are
  is in `Host::scan_unpublished`'s doc. gitleaks skips one before any rule runs, path rules and its own `pkcs12-file`
  included (tested with gitleaks 8.30.1, cross-review 147), so a secret in it is pushed unreported. That includes a
  text file replacing a binary one. Decided against (2026-11-03, kept 2026-11-05 after the path-rule fallback was
  found not to work, secret-scan-gaps/02): scanning such changes as text with `--text`. Reopen it if such a secret is
  ever pushed; a check of file names (`.p12`, `.pfx`, `.jks`) would catch keystores but not the text-replaces-binary
  case.
- **The quality file's intro still says `orchestrator.py gate` judged the tree.** `judge_change` writes that intro,
  though since the split the program is `gate.py`. Left as it was on purpose (2026-10-27): `judge_change` scores 24,
  over the cognitive-complexity limit of 15, and the check pairs a function moved into a new file with its base only
  by an unchanged body, so editing it in the pull request that moves it fails the gate as a new function over the
  limit. Cross-review 61 (F3, 2026-10-27): "a Builder or Reviewer following the quality file's own words to
  reproduce the result is sent to a dead command". Closes with the intro naming `gate.py`, in a later change, where
  `judge_change` pairs with its base in `gate.py` by name and is no worse than 24.
- **The gate's intro writes the rules path (`--config`) as it is.** `judge_change`'s intro names the rules file without
  `one_line` (`gate.py`, `intro = ...` in `judge_change`), so a rules path holding a line breaker splits the
  quality file's third line, and `docs/quality-gate.md`'s "wherever it appears in a list item or a name" says more
  than holds for it. `gate_failures` does not read the intro, so no round is misjudged. Raised by cross-review 38
  (round 3, 2026-10-26). Closes with the intro written through `one_line`, or with each quality-file writer escaping
  once where it joins its lines.
- **Small leftovers in the changed-file walker and its decorators.** Raised by cross-review 45 (F2-F5, 2026-10-26),
  none giving a wrong result: `_ChangedFunction.decorators` unparses each decorator only for `_unmutatable`'s
  `staticmethod`/`classmethod` test, which an `ast.Name` check would make without `_unparsed`; `_parsed_changed_files`
  computes `_removal_points` for `_judged_files` and `vague_names`, which never read them; `_ChangedPythonFile` is a
  frozen dataclass with `set` fields, so its generated `__hash__` raises; `vague_names` unpacks three fields and reads
  `.why` beside them. Closes with the four changed, in one commit, when the walker is next touched.
- **A flaky-tests rerun sometimes waits out `FLAKY_RUN_TIMEOUT` on Jenkins.**
  `TestFlakyTests.test_a_server_a_test_leaves_holding_the_output_does_not_hold_up_the_reruns` takes 8.0 s on
  Jenkins (PR-98, PR-99/3, PR-100), but 14.4 s on PR-101 build 1 and 31.0 s on build 3, failing its 30 s bound: one
  of its six pytest runs waited about the whole `FLAKY_RUN_TIMEOUT`. PR-101 changes only the tests' git fixture.
  Twenty runs here under the load of a parallel suite, with SIGINT ignored, all passed. A rerun that hangs gives a
  `not-rerun` warning, not a wrong verdict. Closes when the run that waits is caught, e.g. with each pytest run's
  wall time and exit in the test's failure message, and its cause fixed. A second cause, of a different kind: on
  PR-190 build 3 (gate-rust-checks 04–06) the reruns of the changed tests did not finish within the 30 s at all,
  since those tests include dozens that build a small Cargo package with real cargo, cargo-nextest or
  cargo-mutants, each from a cold target directory in the rerun's copy. A change that adds many such tests is not
  rerun; that one closes with a limit that grows with the tests rerun (a time budget shared with mutmut was decided
  against, "flaky-tests' reruns get one time limit a run", above).
- **The docs check reads `gate.py` as text.** `engine/e2e/tests/docs/` takes `gate.py`'s flags from the
  `add_argument("--…"` calls in its `parse_args` (`cli.rs`, `gate_flags`), not hiding any with `argparse.SUPPRESS`,
  and its names from lines at the left margin and one indent into a class, its strings only from those that open and
  close on a line (`names.rs`, `gate_names`): a flag added otherwise, or a name defined in a `for`, a `with` or an
  `import`, is not seen. It replaced `tests/test_docs.py`, which built the argparse parser by patching
  `parse_args` (octoflow-without-python ticket 07). The same reader takes no string of a Rust macro whose arguments
  are not expressions, such as `json!`. Closes with gate.py leaving the repository (gate-repo-split/05), or a
  `--help` the check runs once the gate is a program of its own.
- **README and docs left over from the README rewrite (PR #90).** Raised by cross-review 50 (2026-10-26), in what
  PR #90 brought, not in code:
  - The Jenkins job path still uses the old folder name, as
    `--quality-gate AI-Agents-Orchestrator/py-ai-agents-orchestrator-quality` (README.md, the `--quality-gate`
    example; also docs/quality-gate.md), where the folder is `Octoflow-AI` and the job `py-octoflow-ai-quality`.
    The Planned rename entry above still lists the folder as not renamed.
  - docs/architecture.md's endpoint table gives `/files?run=ID` as a list of `{"name", "mtime"}`, while
    `RunServer`'s doc comment (`engine/src/serve.rs`), the authority, gives `{"run", "files"}`.
  - The README's `gate.py` example says exit 0 for OK and 1 for ERROR, but not 2, unjudged, when no quality file is
    written.
  - Smaller: a short line mid-paragraph in the rename entry above; the README no longer says which commands take only
    `--machine`/`--cwd`; the `gate.py` example no longer names the tools it needs (ruff, mypy, pymarkdown, shellcheck,
    actionlint).
  Closes with those lines corrected, or the endpoint table replaced by a pointer to that doc comment.
- **Gaps in reading a running build's quality.md (PR #105).** Raised by cross-review 58 (2026-10-27):
  - Each poll of a running build also asks for `artifact/quality.md`, a 404 until Gate has run; asking for
    `artifacts[relativePath]` in the result's `api/json` tree would halve the requests (F2).
  - No test covers a transient `CIError` between two reads, or a build that ends with a file other than the one read
    while it ran (F5).
- **`vague-names` does not split CamelCase in Python.** `vague_names` matches a whole Python name or its last
  `_`-separated word, so `user_data` is found but a class `InfoManager` or `UserData` is not; in Rust the last CamelCase
  word is matched too (`_is_vague_in_rust`, gate-rust-checks ticket 04), which is Rust's counterpart. Closes, if the run
  metrics show such names recur, with Python's matched the same way.
- **`vague-names` reports a Rust trait impl's associated types and constants.** Only the name and parameters of a `fn`
  an `impl Trait for T` holds are skipped (`_rust_fixed_bindings`), as gate-rust-checks ticket 04 asked; a `type Value`
  or `const STATUS` the trait fixes is reported, as in serde's `impl Visitor for V { type Value = ...; }`. The helper's
  `bindings` give no holder, so the gate cannot tell them apart from an inherent `impl`'s. Found by ticket 04
  (2026-10-30). Closes with the helper marking each binding an `impl Trait for T` holds directly, a schema change.
- **`passes-at-base` builds a Rust package's dependencies anew on every run.** `_rust_base_run_findings` gives each run
  a target directory in its scratch directory, outside the copy, so every gate run compiles the base's dependencies
  before it runs a test, within `--base-run-budget`; on a package of many crates that may use the whole budget, which
  is a `budget-reached` finding. Not measured on Jenkins: gate-rust-checks ticket 05 asked for the Gate stage's added
  time, which needs a Jenkins build of a Rust change on the branch. Decided against for now: the working tree's own
  `target/`, where the base's build would overwrite the binaries Cargo copies up to `target/debug`. Closes, if the
  budget runs out on real changes, with a target directory that lasts between gate runs outside every workspace, as
  `ci/cargo-lock.sh`'s lock file does.
- **`surviving-mutants` splits its budget by language, not by work, and builds Rust anew.** `surviving_mutants` gives
  mutmut half the `--mutation-budget` when the change has Rust to mutate too, and cargo-mutants
  (`_cargo_mutants_on_copy`) what is left by the check's deadline, however many functions each language has; and
  cargo-mutants runs `--in-place` in a copy of its own without `target/`, beside mutmut's, so each run copies the
  tree again and compiles the workspace and its dependencies before its baseline. Not measured on Jenkins: gate-rust-checks ticket 06 asked for the Gate stage's added time on a
  Rust change, which needs a Jenkins build on the branch. Found by ticket 06 (2026-10-30). Closes, if the run metrics
  show either language starved, with the budget split by each language's changed functions, one copy for both tools,
  and a target directory that lasts between gate runs (the same as passes-at-base's above).
- **cargo-mutants is pinned by the sha256 of its archive as downloaded, not as published.** Its releases publish no
  checksums file (`ci/install-tools.sh`), so the pin of v27.1.0 holds the archive the gate's author downloaded on
  2026-10-30, trusted on first use. Closes with a release that publishes checksums, or the asset digest GitHub shows on
  the release page, checked by hand as cargo-llvm-cov's was.
- **`passes-at-base` places a Rust test's target by Cargo's default layout only.** `_rust_target` reads the file's
  place: a target a manifest puts elsewhere (`[[test]] path = ...`, `autotests = false`), a module a `#[path]`
  attribute names, a module of `src/main.rs` in a package that also has `src/lib.rs` (taken for the library's), and a
  file under `tests/x/` that `tests/x/main.rs` does not declare are named wrong, so nextest reports no such test and
  the check gives nothing; a file under `benches/` or `examples/`, or under a `tests/x/` with no `main.rs`, as a
  shared `tests/common/mod.rs`, is a `not-run` finding. Found by gate-rust-checks
  ticket 05 (2026-10-30). Closes with the targets read from `cargo metadata` and the module tree from the helper's
  `mod name;` items.
- **`passes-at-base` cannot tell a Rust test that needs the change from a base that does not build.** A target whose
  tests do not compile at the base counts as failing there, whatever rustc's error is (`_NEXTEST_BUILD_FAILED` and
  `error: could not compile` in `_rust_target_findings`): the test reaching for what the change adds, as the ticket
  meant, but also a base that does not compile on the gate's machine at all. Any other cargo error, as a lock file
  `--locked` refuses, is a `not-run` finding. Python's run tells the two apart by whether the failure passes through
  a file the change edits (`_outside_failed_conftest`). The ticket asked for the build failure to be said "in the
  finding-free summary, not as a finding"; the quality file has no such summary for a check, so docs/quality-gate.md
  says it instead. Found by ticket 05 (2026-10-30). Closes with cargo's JSON messages read for the files the errors
  are in, and the user's ruling on where the summary goes.
- **`vague-names` skips the parameters of a Rust trait impl's `fn`, which Rust lets the impl rename.** Unlike Python's
  keyword arguments, a Rust caller never names a parameter, so the trait fixes only the `fn`'s name; gate-rust-checks
  ticket 04 asked for the parameters to be skipped too, as `@override`'s are, and clippy's `renamed_function_params`
  (a restriction lint) holds an impl to the trait's names. So a `raw_data` parameter of an `impl Opener for Door` `fn`
  is not reported. Raised by the code review of ticket 04 (2026-10-30). Closes with the user's ruling on whether the
  parameters count as fixed.
- **`vague-names` reports a name an undecorated override cannot change.** A `def handle(self)` in a
  `socketserver.BaseRequestHandler` subclass, or a parameter the overridden signature names, has no fix; only a
  method marked `@override` is skipped (`_bound_names`). Raised by the cross-review of `written-rules` (F6,
  2026-10-24): "names fixed by a parent class or framework are reported, though they cannot be renamed". Closes, if
  the run metrics show it is common, with methods whose class has a base from outside the repository left out.
- **`passes-at-base` lists the whole tree of the base, and of each submodule, to find its submodules.**
  `_unpack_commit` runs `git ls-tree -r -z` on every run, even in a repository without `.gitmodules`, and splits the
  whole listing, only to find the entries of mode 160000. Raised by cross-review 41 (F9, 2026-10-26): "a full
  recursive `ls-tree` on every run, only to find gitlinks". Checking `<commit>:.gitmodules` first would skip a gitlink
  that has no `.gitmodules` entry, such as a nested repository added by mistake, which is filled today. Closes, if run
  metrics show the listing costs, by listing only the paths `.gitmodules` names, with such gitlinks left out.
- **The copy's `src/` on `PYTHONPATH` comes before the standard library in `passes-at-base`'s run at the base.** A
  module there named as one of the standard library's, such as `src/token.py`, shadows it, as one at the copy's top
  already did through `python -m`. Raised by cross-review 41 (round 1, not recorded, 2026-10-26). Not seen failing.
  Closes, if it is, by putting the roots on the path after the standard library and before site-packages.
- **A test file that cannot be imported at the base hides all its changed tests from `passes-at-base`.** A
  module-level import of something the change adds makes pytest's collection of the whole file fail at the base,
  which `_collect_at_base` takes as the expected outcome for every changed test in it, including ones that would pass
  there. Accepted: the check reports only, and splitting the file's tests from its imports would mean rewriting
  them. Closes, if its run metrics show these files are common, by importing such names inside the tests that use
  them, or by running each test with the failing import stubbed.
- **A submodule under `tests/` whose commit the change moves makes `passes-at-base` not-run.** `_beside_tests`
  takes the submodule's path for a file that goes with the tests, so `_copy_base_with_tests` removes it with
  `os.remove`, which refuses a directory, and the copy fails as a whole (`could not copy the base`). It was so before
  submodules were filled (`_unpack_commit`). Found while filling them (2026-10-26), not seen failing. Closes when the
  overlay leaves a path the base records as a submodule (mode 160000) to `_unpack_commit`.
- **A test file whose name is not UTF-8 is never rerun by `flaky-tests`.** pytest-randomly 5.0.0 hashes each node
  id as strict UTF-8 (`pytest_randomly._crc32`, called from `pytest_runtest_setup` and its neighbours) and stops
  with an internal error, exit status 3. `_pytest` already lets pytest print such an id
  (`PYTHONIOENCODING=utf-8:surrogateescape`), and the collection runs with `-p no:randomly`, so the id is found;
  each rerun then crashes, and `flaky_tests` reports `not-rerun` quoting the `UnicodeEncodeError`. Raised by the
  cross-review of `flaky-tests` (F10, 2026-10-23). Closes when pytest-randomly hashes the id's bytes, or when the
  reruns can seed such a test without it.
- **The reruns' time limit is too short for slow tests on the Jenkins agent.** `FLAKY_RUN_TIMEOUT` is 30 s a run. On
  `flaky-tests`' own change each run took about 9 s on an idle laptop and about twice that under load. Measured on
  Jenkins on 2026-10-24 (PR #50, surviving-mutants' change): the first rerun did not finish within 30 s, so the check
  reported `not-rerun` and judged no test; the tests that change edits start mutmut, and rerunning their four test
  classes once takes about 50 s on the laptop. A limit that covers them (about 90 s, five seeds) would push the quality
  build past `QUALITY_TIMEOUT` beside the mutation budget, so the user kept 30 s (2026-10-24). `not-rerun` is a finding
  to weigh, never a wrong verdict. Closes with lighter tests for the gate's own checks; one time budget the reruns and
  mutmut share was decided against ("flaky-tests' reruns get one time limit a run", above).
- **A running build's reports are taken as whole once two reads agree.** The quality round's `gate_or_end`
  (`engine/src/workflow/quality_rounds.rs`) reads a still running build's `quality.md`, and its `quality.json` with it,
  each poll and takes the pair when two reads at least `CI_POLL_SECONDS` apart are equal, because Jenkins serves an artifact while `archiveArtifacts` is still
  copying it. Raised by the cross-review of `quality-round-early-read` (F1, 2026-10-27): "Equal reads show only that the
  copy made no progress between them, not that it finished." Two such reads are taken only once they hold a whole
  non-blank line (review 58, F1), so a copy stalled that long past its first line and then resumed still hands the round
  a cut file, which shortens the Builder's findings and `quality_rounds`' fingerprints. Before 2026-10-27 the file was
  read only once the build ended; a copy that failed partway, ending the build, was taken cut then too, and still is.
  Since gate-standalone/08 the same holds for the pair: a `quality.json` cut that way stops the round as not following
  its schema, and one not yet copied when two reads agree leaves the round on the fallback below, reading `quality.md`.
  Closes with a fixed last line that `gate.py` writes and the round checks before taking the file (older gates, which
  the quality build of another project may run, write none), or with asking Jenkins whether the Gate stage has
  completed.
- **The mutation budget leaves most of a large change's mutants untried.** `MUTATION_BUDGET` is 600 s. Measured on
  Jenkins on 2026-10-24: on PR #50 (surviving-mutants' change) mutmut tried 756 of 3079 mutants and the Gate stage took
  683 s, the build about 850 s; on PR #51, 677 of 801 in a 638 s Gate stage. That fits `QUALITY_TIMEOUT` (1200 s,
  `engine/src/constants.rs`) with about 350 s to spare, but not after a wait in Jenkins' queue, which counts against a
  quality round (PR #52 waited about 860 s behind the others). Those were pull request builds; a quality build has
  skipped the Package stage since 2026-10-24, so it has more to spare than that. The user kept 600 s (2026-10-24): a
  stopped run says `budget-reached`, never "none survived". Closes with mutants chosen by the changed lines rather than
  whole functions (a time budget shared with the reruns was decided against, "flaky-tests' reruns get one time limit a
  run", above). On PR #113 (2026-10-27) mutmut was stopped at 600 s after trying 0 of 253 mutants: its three runs of the
  whole suite before the first mutant took the budget. Narrowed (2026-10-28, gate-open-decisions ticket 06): the
  Jenkinsfile's Test stage records a coverage context per test, and `gate.py --test-contexts` has mutmut run only the
  tests that reach the changed functions and its selection collects (`_recorded_tests`, `_narrowed_table`); replayed
  locally, PR #113's change took mutmut 77 s instead of 709 s, with the same verdicts. Still open: a change to a
  function nearly every test reaches, such as `tool_env`, still costs about the whole suite three times, and a large
  change still has more mutants than the budget tries. Not yet shown on Jenkins: the first pull request build with the
  flag.
- **`--test-contexts` collects mutmut's selection in a fuller copy than mutmut runs it in.** `_narrowed_table` runs
  `pytest --collect-only` in the gate's copy of the tree, which holds every tracked file; mutmut's own `mutants/` holds
  only `source_paths`, `also_copy`, `tests/`, `test/` and a few top-level files. A selection that reaches outside those,
  such as none in a project with tests under `other/`, can name a recorded test there that mutmut's pytest cannot find,
  and mutmut's run fails as `not-run`. This repository's selection is `tests/`, so it is not affected. Closes with the
  collection run in a copy laid out as mutmut lays out `mutants/`, or by mutmut itself.
- **Mutation-testing the code that runs mutmut leaves processes behind.** When mutmut stops a mutant at its own
  timeout it kills the worker, and a real mutmut that `TestSurvivingMutants` started in its own session lives on as
  an orphan group of one process per CPU. It only happens when a change edits that code. Jenkins' process-tree
  killer should end them with the build; not checked. Closes with a check on a PR build, or with that test run
  without its own session under mutmut.
- **The surviving-mutants check mutates Rust by added lines only.** cargo-mutants runs `--in-diff`, which mutates only
  what the diff's added lines hold, so a Rust function whose edit only removes lines, such as a guard clause, is not
  mutation-tested; Python's functions count removals (`_edits_function`, gate-open-decisions ticket 03). Closes with the
  Rust half choosing whole functions, as `_edits_function` does, and passing them to cargo-mutants with `--re` or
  `--file` instead of the diff.
- **mutmut reads a symlinked `.py` file in the surviving-mutants copy.** `_copy_tracked` copies a tracked symlink
  as a link (`gate.py`), `_mark_for_mutmut` leaves links unmarked, and mutmut 3.8.0's `walk_all_files` lists
  a symlink to a file and `create_mutants_for_file` opens it. So once any changed function starts mutmut, a change
  adding `x.py -> calc.py` has every function of `calc.py` mutated a second time, as `x.py`, spending the budget on
  mutants no finding reports; and `x.py -> /dev/zero` or a FIFO reads without end or blocks until the budget runs
  out. The gate's own selection skips such links (`_parsed_changed_files`). Raised by the cross-review of
  `gate-python-files` (R2-F1, 2026-10-26); rests on mutmut's source, not on a run. Closes with the copy or
  `_mark_for_mutmut` keeping links out of mutmut's walk, without breaking a tracked link the tests import through,
  and a test that also edits a real function.
- **A body moved into a file hides a function with that body moved out of it, which is then judged as new.** The
  cognitive-complexity check takes a base function as gone from its file when the file now holds its body fewer times
  than the base did (`_BasePool._gone`), without asking where the working tree's copies came from. So when `calc.py`'s
  `big` moves unchanged to `lib.py` and `other.py`'s `dup`, with the same body, moves unchanged into `calc.py`, `dup`
  takes `big`'s base in `calc.py` and `big` is "a new function": two unchanged moves fail the gate (review 01 round 7,
  R7-F1, a regression from the round-6 rule; deferred by the user, 2026-10-24, as rare: it needs two over-limit
  functions with one body in two of the change's files). Closes with a count that pairs each working copy with the
  base it came from, so an arrival does not make up for a departure.
- **A copy of a function whose original the change edits keeps the old score and passes.** The cognitive-complexity
  check pairs identical bodies before names (`_base_scores`), so that a function renamed unchanged keeps its score
  and a new function under its old name is new. A change that edits `big` in place and adds `copied`, a copy of
  `big`'s old body, has the same shape: `copied` takes `big`'s score and passes, and the edited `big` is judged as
  new, so nothing is reported when the edit takes `big` below the limit. The same holds for a copy in a file the
  check does not judge, such as one under `tests/`, since a function moved unchanged there must keep its base from
  a new function under its old name (review round 6, R6-F1). Decided by the user in review round 4 of ticket 06.
  Closes with a way to tell a copy from a rename, such as git's copy detection (`git diff -C`) on the change's
  files.
- **A resume finds an agent alive whose start never finished and only waits for it.** `start`
  (`engine/src/workflow/turn.rs`) saves the agent's record, may wait without a deadline for a startup dialog the
  human answers (`wait_for_startup`), and only then saves the session. An orchestrator stopped in between leaves an
  agent that `agent` finds alive (`How::Alive`), so `prompt_for` treats it as one that took its prompt. Two cases,
  raised by cross-review 128 (F1, 2026-11-04) against run-crash-ending/02: "If `start(role, pane, Some(session))`
  relaunched a dead agent while `prompted == file` (the `How::Resumed` path, which is owed `ContinuePrompt`) and the
  orchestrator stopped before [the session was saved], the resume gets `Alive`. `prompt_for` then returns `None`, so
  the relaunched agent is never told to continue and sits idle until the stall notice"; and a fresh start stopped
  during `wait_for_startup` has its step prompt typed into the startup dialog, which `taken` may count as taken if
  herdr reports the dialog as `blocked` (unconfirmed). Answering `Resumed` for an alive agent without a session is not
  the fix: a harness that reports no session would then be told to continue a prompt it is still working on. Closes
  with `start` recording, before its first save, that the agent's start is unfinished, and `agent` restarting or
  re-prompting such an agent; or with the first save moved after the startup wait.
- **`agent`'s herdr failure on an alive agent is untested.** `agent` (`engine/src/workflow/turn.rs`) asks herdr for
  an agent it knows and stops the run when herdr fails; the fake herdr (`engine/src/herdr/fake.rs`) cannot make
  `agent` fail, so no test holds the `?` there against becoming a silent skip. Raised by cross-review 128 (F4,
  2026-11-04): "The `?` could become a silent skip ... without any test failing." Closes with a failure the fake's
  `agent` can be set to give, and a resume test that expects the run to stop with it.
- **Four copies of a scratch directory removed on drop, each named its own way.** The `TempDir`s of
  `engine/src/test_support.rs` and `ui/src-tauri/src/lib.rs` name theirs by pid and a counter,
  `engine/src/serve/tests.rs`'s by the test's name and pid under a TMPDIR it reads itself, and `xtask/src/process.rs`'s
  by a prefix, pid, time and counter, retried until new; serve's drops a failed removal silently, and the others only
  report it on standard error, which libtest hides on a passing test. Raised by cross-review 117 (F3, 2026-11-03):
  "A fix to one, such as making names unique, has to be repeated in each." Closes with one shared type: a `tempfile`
  dev-dependency, or a test-support crate the engine, the desktop shell and xtask all use.
- **The e2e scenarios' scratch directories collide between test runs that share `/tmp`.** `Scratch::new`
  (`engine/e2e/src/scratch.rs`) names a scenario's directory `octoflow-e2e-<name>-<pid>` and first removes whatever is
  there, so two runs whose test processes have the same pid, as processes in different PID namespaces over one `/tmp`
  can, delete and overwrite each other's worlds. Seen 2026-11-04 (live-checks/01): with several agents testing their
  worktrees at once, `kill_in_design` and `kill_in_tests` failed on an exit status and `workflows_listed` printed
  another worktree's `examples/workflows/product-team.toml`; with a TMPDIR of its own the same build passed all 50.
  Closes with a name no other process can take (made with `create_dir` and retried until new, as xtask's
  `make_temp_dir` does), or with the shared type the entry above would make.
- **Four copies of the test writer of an executable script.** A test's script is written by a child process, so that
  no test thread's fork holds it open for writing when it runs (ETXTBSY; ui-test-text-file-busy/01, 2026-11-03). The
  same `sh` line is in `tool` (`engine/src/host/tests.rs`, which says why), `Dir::tool` (`engine/tests/program.rs`),
  `World::make_bin` (`engine/e2e/tests/e2e/world.rs`) and `put_executable` (`ui/src-tauri/src/lib.rs`). Raised by
  cross-review 123 (F2, nit): "A correction to the writer has to land in all four, and a copy that is missed brings
  the flake back in that crate." Closes with one writer in the test-support crate the entry above would make.
