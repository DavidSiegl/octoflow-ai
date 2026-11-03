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
- **The quality gate in a `gate.py` of its own: what is left.** The cut is made (gate-split ticket 01), and the
  Jenkinsfile's cutover fallback is gone. Left: making the test-file patterns a setting,
  for the gate's use in other projects (`.scratch/gate-split/issues/04-configurable-test-files.md`). Decided 2026-10-24
  that the gate moves to its own repository only once a second project uses it; `.scratch/gate-repo-split/` plans
  that move.
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

- **new-dependencies on Cargo: two readings of a manifest that its doc does not state.** In
  `engine/src/checks/dependencies.rs`, `cargo_manifest_dependencies` keeps the first entry of a crate across its tables,
  as `add_named` does across manifests: `foo = "1"` in `[dependencies]` and `foo = { git = "..." }` under
  `[target.'cfg(windows)'.dependencies]` record foo as from crates.io, and its git source is never compared with the
  spec. And `cargo_member_entry` takes only boolean `true` as `workspace = true`: `workspace = "true"`, `1` or
  `false` falls through to `cargo_entry` as a crates.io dependency, rather than being refused as an entry of another
  shape. Raised, not recorded, by cross-review of 20aa212 (2026-10-30); both predate it. Closes with the rule chosen and
  stated on `cargo_dependencies`'s doc: the first entry, or every source a crate is named with; and a non-boolean
  `workspace` refused.
  Ticket: `.scratch/cargo-dependency-rules/issues/01-one-rule-per-crate-entry.md` (#237).
- **`Map` has no first-wins insert.** `cargo_manifest_dependencies` and `add_named`
  (`engine/src/checks/dependencies.rs`) each spell out `if !out.contains_key(&key) { out.insert(..) }`, a branch that
  put the first at 16 of the gate's cognitive-complexity limit of 15 (PR #214), which 20aa212 met by moving the
  `workspace = true` resolution into `cargo_member_entry` instead. Deferred from cross-review of 20aa212 (F1, nit,
  2026-10-30): "A method there used at both sites would remove that branch from `cargo_manifest_dependencies` and keep
  inheritance inline." Ticket octoflow-without-python/04 kept that map as `crate::map::Map` (`engine/src/map.rs`), since
  serde_json's map holds only JSON values and these hold `Dependency`s; closes with a first-wins insert there, both
  sites taking it.
- **Readers of `state.json` still take records an earlier release wrote.** Since octoflow-without-python ticket 04
  (2026-11-03) a `state.json` without the format marker is refused (`crate::state::format_problem`), so no record an
  earlier release wrote reaches them, but `ProofRun::read` (`engine/src/proofs.rs`) still reads a `proofs` record
  without `written` as a first run, `Restored::read` (`engine/src/workflow/locks.rs`) a `restored` record without
  `paths` as restoring nothing, and the turn and agent readers a record without `kind` as `DEFAULT_AGENT`'s. Each is
  a branch no format-2 run takes. Closes with those branches removed and the records' fields required, as
  `RunState`'s are.
- **`a_server_that_stops_accepting_is_started_again_on_the_port` fails about one run in four.** The engine's lib tests
  (`engine/src/serve/tests.rs`) got an empty-bodied 500 from the port in 3 of 10 runs on main at 6bc3bb3 and 1 of 4
  on ticket octoflow-without-python/04's branch, both under `cargo test`'s parallel threads; alone it passed every time.
  serve answers every error with a JSON body, so the 500 is likely another test's server that took the port the
  stopped one freed. Found 2026-11-03 while verifying ticket 04. Closes with the test holding its port, or another
  check that the new server answers, and a run of the suite repeated without failure.
- **`py::toml` keeps a port of Python's `json.dumps`.** Ticket octoflow-without-python/04 moved `json.dumps`'s spacing,
  `ensure_ascii` and float `repr` from `py::json` into `engine/src/py/toml.rs` (`json_dumps`), for the one message
  that quotes an unknown package source (`unknown_source`, `engine/src/checks/dependencies.rs`). Raised by
  cross-review 114 (F8, nit, 2026-11-03): "This is drift from 'py::json … becomes serde_json'. Ticket 06 (py::toml)
  may be its home." Closes with ticket octoflow-without-python/06, which replaces `py::toml`.
- **`crate::map::Map` hand-writes an insertion-ordered map.** `engine/src/map.rs`, moved out of `py::json` by ticket
  octoflow-without-python/04 for the engine's maps of values that are not JSON (`Map<Dependency>`, `Map<String>`).
  Raised by cross-review 114 (F9, nit, 2026-11-03): "`indexmap::IndexMap` is already in the build through
  `preserve_order`." Using it needs `indexmap` as a dependency of the engine's own, which the user approves; closes
  with that approval and `Map` replaced by `IndexMap`, or with the user's ruling that `Map` stays.
- **serve's event stream can miss a finished run's last files.** `end_done_run` (`workflow.rs`) saves state.json
  with the owner cleared, which makes the run finished, before it writes summary.md and metrics.json, and `release`
  saves before summary.md likewise; `RunWatch::changes` (`serve/runs.rs`) reads a finished run's files again only
  when its record changes. A poll that lands between the save and the writes keeps a file list without them for
  good, so the desktop app never shows them. The e2e scenario `serve_endpoints_and_events` fails when it does:
  PR-213's pr-head build, whose final checkpoint lacked metrics.json, where the branch build of the same commit
  passed (2026-10-30). Closes with the files written before the save that finishes the run, or with the watch
  re-reading a finished run's files until they stop changing.
  Ticket: `.scratch/serve-run-files/issues/01-finished-run-files-reread.md` (#230).
- **The desktop app ships its npm packages' code without their licenses.** `octoflow-third-party-licenses.html`
  (`packaging/about.toml`) lists the Rust crates only, as cargo-about reads Cargo's graph; the app's web view bundles
  `ui/package.json`'s runtime dependencies, React, react-markdown, remark-gfm and @tauri-apps/api with what they pull
  in, minified into `ui/dist/` without their notices. All were MIT when the project took its license (2026-10-30).
  Closes with their license texts generated from `ui/package-lock.json` at build time, e.g. by a Vite license
  plugin, and bundled with the app beside the crates'.
  Ticket: `.scratch/desktop-packaging-gaps/issues/01-npm-licenses-shipped.md` (#239).
- **A release that adds a placeholder to a built-in prompt would refuse every run that saved an override of it.**
  `saved_prompts` (`engine/src/prompts.rs`) holds a run's saved prompt overrides to `prompt_problem` on resume, whose
  rule that an override keep every placeholder of the built-in text never refused a resume before. No run can meet it
  today, since overrides came with that rule (fb87a8f) and every built-in placeholder is older. Raised by cross-review
  111, round 2 (2026-11-03): "if a later release adds a placeholder to a built-in text, every run that saved an
  override of that text will be refused on resume, where it could still have been filled." Closes with the resume
  check limited to what cannot be filled in, the template's form and unknown names, or with the release that adds
  such a placeholder saying so in its notes.
  Ticket: `.scratch/prompt-override-resume/issues/01-resume-refuses-only-unfillable-overrides.md` (#238).
- **The Rust engine names itself `orchestrator.py`.** Its messages and hints were ported word for word, so the
  e2e recordings, made by the Python engine, hold them: `resume: orchestrator.py resume <id>` in `list` and the
  interrupted note (`cli/listing.rs`, `cli.rs`'s `resume_command`), `` `orchestrator.py workflows default` `` in
  `workflows` (`cli/workflows.rs`), and `orchestrator.py` among `judged-in-quality`'s `JUDGING_FILES`, a file the
  tree no longer has, while the files the build's Rust depends on, `rust-toolchain.toml` and `Cargo.lock`, are not
  among them, so a quality build of another toolchain counts as having judged the tree. Found by rust-engine ticket 11
  and its code review (2026-10-30). Closes with the engine naming itself `orchestrator`, its tests and the e2e
  recordings changed with it, and `JUDGING_FILES` the files that judge the tree now, in a change of its own after the
  switch.
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
- **`tests/test_docs.py` reads the engine's names by pattern.** `rust_names` finds each `fn`, type, `const`,
  `static`, `mod`, string literal, impl `fn` and struct field in `engine/src` by regular expression, not by parsing,
  so a name in a comment or a `macro_rules!` counts as defined, and a doc's `Type::member` is not checked at all, only
  `Type.member`. Found by rust-engine ticket 11 (2026-10-30). Closes, if a doc names a gone item unnoticed, with the
  names read from rustdoc's JSON or syn, as the gate-rust helper reads Rust.
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
  Closes when a quality build that runs the scenarios fits in `QUALITY_TIMEOUT`, shown by live-checks/01's contract test
  green against one, or with a larger `QUALITY_TIMEOUT` in the Rust engine.
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
  (`crate::cli::drain::queue`, `read_queue`), where Python reads its spec.md through os.fsdecode. Decided against unless
  a run is found that writes such input.
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
  (`engine/src/ci.rs`, `engine/src/pypi.rs`, `engine/src/py/toml.rs`): an answer of Jenkins's of another shape than a
  queue item, a build's result, a list of builds or a test report is a `CIError` that may pass, where the Python engine
  fails with an `AttributeError`, and a queue item's `executable.number` that is not an integer is one too, where Python
  keeps it; a manifest is read as TOML 1.1, which tomllib before Python 3.15 refuses in part, and why one is not TOML is
  the toml crate's words with tomllib's `(at line L, column C)`; and rustls refuses a server certificate that is a
  CA's own, as a self-signed one made by `openssl req -x509` with its default extensions is (`CaUsedAsEndEntity`),
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
  desktop shell's are a `#[cfg(test)]` module in `ui/src-tauri/src/lib.rs`.
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
  read as it is (`crate::py::url::urlsplit`), where Python's handler fails on urlsplit's ValueError and answers
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
- **serve reads a run's files through a symbolic link.** A run's files are listed and read by `Host::run_files` and
  `Host::read`, which follow a symbolic link (`[ -f ]`, `cat`), so a link an agent leaves in its run directory, or a
  run directory that is a link, lets `/file` and the event stream show a file outside the run, `~/.ssh/id_rsa` say, to
  the desktop app's window; the name a request gives is still only a plain name, and only the token's holder can ask.
  `a_link_in_the_run_directory_is_followed_as_python_follows_it` (`engine/src/serve/tests.rs`) pins it. Ruled
  (2026-10-29, the user): keep the Python engine's behaviour until the switch, then refuse a link whose target is
  outside the run directory, as a fix on main once the Python engine is gone, which it is since rust-engine ticket 11.
  Open until that fix.
  Ticket: `.scratch/serve-run-files/issues/02-links-outside-run-refused.md` (#231).
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
- **An AppImage run without FUSE passes no signal on to the orchestrator.** Under `APPIMAGE_EXTRACT_AND_RUN` the
  AppImage's runtime forks, runs `AppRun` in the child and waits, so the process a caller started is the runtime,
  and `Cli::exec` (`ui/src-tauri/src/lib.rs`) only replaces the child. Raised by the cross-review of desktop-cli
  (F3, 2026-10-25), reproduced with `dist/orchestrator-desktop-linux-x86_64.AppImage serve`: "`kill -TERM <runtime
  pid>` → both orchestrator processes still running 2 s later, orphaned", so `timeout`, a systemd stop or `kill %1`
  on an AppImage `run` on a machine without FUSE leaves the orchestrator and its agents running unsupervised.
  Ctrl-C reaches the whole process group and is not affected; the mounted AppImage was not checked. Closes with
  `Cli::exec` asking the kernel for a signal when its parent, the runtime, dies (`PR_SET_PDEATHSIG`) when the app
  runs extracted, checked on a built AppImage, or with a runtime that forwards signals.
  Ticket: `.scratch/desktop-packaging-gaps/issues/02-appimage-signal-reaches-the-cli.md` (#240).
- **An AppImage run without FUSE leaves its extraction in `/tmp`.** Under `APPIMAGE_EXTRACT_AND_RUN` the
  AppImage runtime that linuxdeploy-plugin-appimage bundles extracts into `$TMPDIR/appimage_extracted_<hash>` and
  never removes it. `packaging/build-desktop.sh` sets the variable for the whole Tauri build, so linuxdeploy and
  appimagetool leave two such directories on the build machine at every build (Jenkins's agent among them). Users
  who run the desktop AppImage as the CLI without FUSE are left with one 336 MB directory per version. Its smoke
  test already points `TMPDIR` into its own directory. Found in the second build round of run 20261025-081413-327fc2
  (2026-10-25). Closes with a `TMPDIR` the build removes around `tauri build`, and with a README note, or a newer
  runtime that cleans up, for users.
  Ticket: `.scratch/desktop-packaging-gaps/issues/03-appimage-extraction-cleaned-up.md` (#241).
- **The Package stage still runs programs it wrote to `/tmp`, kept out of agentbox0's Falco mail only by path
  exceptions that let any binary placed under those paths run unreported.** The Test and Gate stages set `TMPDIR` to
  `$WORKSPACE_TMP/tmp`, on the agent's volume, which the rule cannot see (the Jenkinsfile's Test stage says why),
  but Package keeps `/tmp`. There the linuxdeploy and appimagetool that `packaging/build-desktop.sh` runs under
  `APPIMAGE_EXTRACT_AND_RUN` extract themselves into `$TMPDIR/appimage_extracted_<hash>` (entry above) and run from
  there. The homelab excepts those paths (falco/rules/homelab.yaml, `appimage_extracted`). Each stage now creates
  its own TMPDIR, so a stage added later has to copy both lines. Raised by cross-review 71 (F1, F3: "set TMPDIR for
  every stage that runs what it builds (one pipeline-level definition next to CARGO_HOME would do)"). Closes with
  that pipeline-level `TMPDIR`, created before any stage that uses it, and a Package stage seen to pass with it on
  agentbox0. The homelab can then drop `appimage_extracted`.
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
- **`doctor` checks gitleaks's `git` subcommand, not its version.** README asks for gitleaks 8.19 or later; the
  check reuses `Host.gitleaks_problem`, the run's pre-push check, which asks `gitleaks git --help` because a
  distribution's build may print no version number. So `doctor` shows no gitleaks version. Kept by choice
  (2026-10-24).
- **A spec left `ready-for-human` by a conflict stays there when `refresh` clears the conflict before a drain does.**
  drain moves such a spec to `in-review` only when its own refresh clears the run's conflict (`refreshed_headers`);
  a run cleared by `orchestrator.py refresh` beforehand has no conflicts left, so drain cannot tell the spec held
  back only by its conflict from one the human set `ready-for-human`, and leaves it, with an open pull request
  ready for review that no drain marks `done` once merged. Kept so that drain never overrides a status the human
  set. Raised while building the refresh command (2026-10-25). Closes with the run recording that a refresh cleared
  its conflict, which drain then reads, or with refresh moving the spec itself.
  Ticket: `.scratch/drain-refresh-cleared/issues/01-refresh-records-a-cleared-conflict.md` (#236).
- **Proofs run after the last round's review too.** No Builder turn follows to read them; only the draft pull
  request's review shows them, and each may take `PROOF_TIMEOUT`. Kept: a demonstrated defect is what the human
  most needs to see in a draft. Raised by the same review.
- **What a proof writes outside its own directory can enter the commit.** A proof runs in the working tree
  (`Workflow::run_proof`), and only its copy's directory is removed after it, so a file it writes elsewhere, such as the
  `__pycache__` of a module it imports in a project that does not ignore those, stays for `Workflow::publish`'s `git add
  --all`. In-place runs always had this; since proof-runs ticket 01 a worktree run has it too, its proofs importing the
  worktree's code. Raised there (2026-10-27). Closes with `Workflow::publish` committing only what the turns changed, or
  with proofs running in a throwaway copy of the tree. Ticket:
  `.scratch/proof-runs/issues/03-proof-writes-stay-out-of-the-commit.md` (#234).
- **A proof the Builder moves into the project stays carried as erroring.** `PROOFS_NOTE` lets the Builder move a proof
  into the project as a regression test; `Workflow::run_proof` then records it "it did not run: ... is gone", which is
  not a pass, so `Workflow::carried_proofs` keeps carrying it. Raised by the cross-review of proof-runs (review 64, F9,
  2026-10-27): "is carried and told as a standing finding in every later round": the Builder hears it does not pass, and
  the verdict step hears it "no longer fails but errors", even when the moved test passes in the project. Predates that
  branch, where the same move gave pytest's exit status 4, an error carried the same way. Closes with
  `Workflow::carried_proofs` treating a missing proof as moved and dropping it, and the notes saying so. Ticket:
  `.scratch/proof-runs/issues/04-moved-proof-not-carried.md` (#235).
- **`expected-value-changed` misses an inline literal's sign, type or structure.** `_changed_literals` pairs
  assertions by `_shape` and compares `_literals`, which drops types and operators: `assert f() == -5` becoming
  `== 5` changes the shape (no pair, no finding), and `== 1` becoming `== True` gives equal literal lists
  (`[1] == [True]`). Raised by the cross-review of `test-integrity-constants` (F4, 2026-10-27): "`_literals`
  equality misses sign, type and structure changes"; the constant path compares `_tree_fingerprint`s instead.
  Closes with the inline path comparing typed literals and pairing a unary minus with its operand.
- **A run that never reaches `done` keeps its local refs.** A locking step's commit stays under
  `refs/orchestrator/<key>/locked/` (`RunState::lock_ref`) until the run ends `done` (`Workflow::clean_up_locks`);
  `close` and `prune` do not delete them, so a failed run's locked snapshots are never collected by `git gc`. Closes
  with `close` and `prune` deleting `RunState::refs_prefix` as they delete the run's branch.
  Ticket: `.scratch/close-run-refs/issues/01-close-deletes-run-refs.md` (#232).
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
- **The secret scan does not look inside a file git takes for binary.** With no attributes, git still calls a file
  binary when its first 8000 bytes hold a NUL, and `git log -p` prints "Binary files ... differ" for it, to gitleaks and
  to `allow_comments` (`engine/src/host/scan.rs`) alike, so a secret or a `gitleaks:allow` in such a file is pushed
  unscanned. gitleaks' own `git` mode has the same blind spot. Raised by the cross-review of `push-secret-scan` (F15,
  2026-10-24). Closes with a decision in the spec whether binary content is in scope, and, if it is, a `--text` diff or
  a scan of the blobs themselves (`gitleaks dir` over the pushed files).
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
- **`tests/test_docs.py` gets `gate.py`'s parser by patching `argparse.ArgumentParser.parse_args`.**
  `gate.parse_args` builds its parser inline and returns only the parsed arguments, so `built_parser` calls it with
  `parse_args` patched class-wide to hand the parser back, and reads argparse's private `_actions`. Raised by
  cross-review 56 (F8, 2026-10-27): "Breaks if `parse_args` switches to `parse_known_args` or parses twice". It
  fails loudly then (`parse_args parsed without its parser's parse_args`). The engine's commands are read from its
  `--help` (`cli_commands`). Closes with a `gate.build_parser()` that `parse_args` calls and the test reads.
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
- **A running build's quality.md is taken as whole once two reads agree.** The quality round's `gate_or_end`
  (`engine/src/workflow/quality_rounds.rs`) reads a still running build's `quality.md` each poll and takes it when two
  reads at least `CI_POLL_SECONDS` apart are equal, because Jenkins serves an artifact while `archiveArtifacts` is still
  copying it. Raised by the cross-review of `quality-round-early-read` (F1, 2026-10-27): "Equal reads show only that the
  copy made no progress between them, not that it finished." Two such reads are taken only once they hold a whole
  non-blank line (review 58, F1), so a copy stalled that long past its first line and then resumed still hands the round
  a cut file, which shortens the Builder's findings and `quality_rounds`' fingerprints. Before 2026-10-27 the file was
  read only once the build ended; a copy that failed partway, ending the build, was taken cut then too, and still is.
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
- **`paths_between` (`engine/src/host.rs`) follows the user's `diff.relative`.** Its `git diff --name-only` has no
  `--no-relative`, so with `diff.relative=true` in the user's git config and a `cwd` that is a subdirectory of the
  repository (not checked whether a run can pass one), it lists only the paths under that subdirectory, and its
  `top_literal` pathspecs for files elsewhere match nothing. `Host::lock` then returns fewer locked paths and
  `Host::locked_changes` misses edits to them; `locked_changes`'s own `git diff` has the same exposure. Raised by the
  cross-review of `trust-quality-at-pr` (review 49, F7, 2026-10-26), which found it in what is now `judging_changes`
  (`engine/src/cli/judged.rs`) and fixed it there with `--no-relative`; of `paths_between` it said: "has the same
  exposure, but it is outside this range". Closes with `--no-relative` on both calls and a test that sets
  `diff.relative` and locks from a subdirectory. Ticket:
  `.scratch/git-diff-relative/issues/01-locks-ignore-diff-relative.md` (#233).
- **A run that dies right after it saved phase `done` never finishes ending.** `end_done_run`
  (`engine/src/workflow.rs`) saves `phase = DONE`, counts the turns' tokens (`count_tokens`), saves again with `owner`
  cleared, and only then writes `summary.md` and `metrics.json`, notifies the human that the run finished and closes
  the workspace. A crash after either of those two saves leaves a run that `resume` finds done: `Workflow::run`
  returns the verdict at once, so neither file is ever written, the human is never told, the herdr workspace stays
  open, and after the first save `owner` stays set and the turns have no token counts. Found in the Python engine by
  orchestrator-test-methods/05 (2026-10-28), whose crash test went with that engine; the Rust engine was ported to
  behave the same, with no test that crashes it after each save ("No test crashes the Rust engine after each save",
  above). Closes with a resume of a `done` run that finishes whatever of that ending is missing (each step can tell:
  the files exist, `owner` is cleared, the tokens are counted).
  Ticket: `.scratch/run-crash-ending/issues/01-done-run-finishes-its-ending.md` (#227).
- **A crash between an agent's start and its session being saved costs that turn its session.** `start`
  (`engine/src/workflow/turn.rs`) saves the agent's record, then asks herdr for it and `note_session` saves the
  session. A resume after the first save finds the agent alive (`agent` returns `How::Alive`) and prompts it without
  asking herdr, so the turn's record (`turn_record`) starts with no session; `note_session` saves it while the turn
  runs, but `end_turn` keeps `session` only when it matches the one at the start, so the turn ends with none, and
  `count_tokens` cannot count its tokens. Found in the Python engine by orchestrator-test-methods/05 (2026-10-28),
  ported to the Rust engine unchanged, untested as above. Closes with `agent` noting the session of an agent it finds
  alive.
  Ticket: `.scratch/run-crash-ending/issues/02-alive-agent-session-noted.md` (#228).
- **A resumed interview tells the human again that it waits for them.** `step_turn`'s `Announce::Interview`
  (`engine/src/workflow.rs`) has `announce` (`engine/src/workflow/turn.rs`) focus the human-paced agent's pane and
  notify "<label> is waiting for you" in every orchestrator that takes the turn (`turn` calls it from `written` on every
  pass), so a resume while the interview is under way repeats both, once per resume. Raised by cross-review 81 (F1,
  2026-10-28) against ticket orchestrator-test-methods/05, which allows only documented idempotent repeats: "Neither is
  idempotent: the cited comment … says each orchestrator does it again, which is the repeat the ticket asks the test to
  fail on." The user ruled it a bug (2026-10-28), not wanted. The Rust engine was ported to behave the same, untested as
  above. Closes with an engine that announces a turn only where none did (for example, only when `prompted` is not the
  turn's file). Ticket: `.scratch/run-crash-ending/issues/03-interview-announced-once.md` (#229).
