# TODO

What is known to be open in the engine (`engine/`), the desktop app (`ui/`) and CI, and why. A fixed entry
is deleted; git history and the release notes (`docs/releases/`) keep what was done. An entry that a fix only narrows
is rewritten to say what is left. The quality gate's entries moved with it to
[octoflow-gate's TODO.md](https://github.com/DavidSiegl/octoflow-gate/blob/main/TODO.md) and left this one on
2026-11-06 (gate-standalone/10).

## Planned

- **The e2e scenarios: what they do not record.** `engine/e2e/` runs 49 scenarios against the engine as a program and
  compares each with its recording (docs/architecture.md, Tests); a scenario's recording is a review of the engine's
  behaviour. Not covered, and accepted
  as known gaps (the user's ruling, 2026-11-11), since unit tests cover each harness's arguments and its token
  parsing: the harnesses other than claude and codex starting
  an agent (only their arguments differ); a session log's token counts (every turn records `unknown`); `--timeout` and
  the stall notice, which take real minutes; round 2 of the workflows other than `default` and
  a fresh session's prompts as sent, whose templates only the recorded `state.json` holds and whose text only golden
  files hold (docs/architecture.md, Tests). Where gitleaks or openssl is not on PATH, every scenario fails naming it: a
  skip would pass a machine that checked nothing, and the gate's test-integrity check fails an added skip (the user's
  ruling, 2026-10-27); Jenkins's Install stage downloads the pinned gitleaks (`ci/install-tools.sh`). What a record
  holds by design, each a choice a later ticket may revisit: output a human reads (logs, errors, `show`, `list`,
  `prune`) as its facts (`normalize::facts`), not its words; a run's files listed by modification time, as `/files` and
  the event stream list them, sorted by name, since seconds of the clock decide that order; ssh's commands not at all,
  only what they did; doctor's lines without git's, which is the host's. Two more are recorded as decided
  against below: the `state.json` saves between two commands, and serve's events between two checkpoints. One engine run
  takes 2 to 25 seconds a scenario (25 for serve's event stream). Decided against (2026-10-27): scenarios as data files the harness interprets, which needed a
  language of their
  own for kills, takeovers and serve's stream; a cassette of the expected requests that the fakes answer in order, which
  breaks on every count of polls; and a stub gitleaks where none is installed, which would record what no engine run
  with gitleaks does. Decided against too (2026-10-27, orchestrator-test-methods/06): golden files of the engine's
  follow-up prompts, notes, commit messages and pull request bodies, which the recordings hold as sent where a scenario
  reaches them.

- **The eval set: what it does not cover.** `cargo xtask eval` (live-checks/04, docs/design.md, Evals) runs
  the engine as a program, `run --no-pr --spec`; it runs `EVAL_ENGINE`, else the checkout's own build. Decided
  against (the user's ruling, 2026-11-11): an `eval` command of the engine's own, since evals are a
  developer's chore, not a user's feature. Not covered: the quality gate, which a trial
  runs only given `--quality-gate local` after `--`, and then by an empty `gate.toml`, since the toy repositories have
  none, so `quality rounds` stays 0; a `--worktree` or `--machine` run, which the runner refuses; and an interview, which `--spec` skips.
  Decided 2026-10-28.

## Open issues

- **Nothing tests that the shell's navigation guard and folder dialog are wired in.** Review 356 (F5): only the
  pure `in_app` (`ui/src-tauri/src/lib.rs`) is tested; were `.plugin(stay_in_app)` dropped from `run`, or the origin
  taken from the wrong config, every test would still pass, and the dev-url branch of `webview_origin` is untested
  too. Review 358 (F3): likewise `.plugin(tauri_plugin_dialog::init())` and the command name ProjectBar invokes,
  `plugin:dialog|open`, which its tests mock; the by-hand check in the packaged app was not run. What would close it:
  a test that builds the app with Tauri's mock runtime (`tauri::test`), asks its webview to navigate to another
  origin, and invokes `plugin:dialog|open` through its IPC to see that the command is known and allowed.
- **The guards against a second resume or permission answer are lost when their component remounts.** Review 360
  (F2, F4): ResumeButton's `clickedOn` (`ui/src/components/RunDetail.tsx`) and Conversation's `answering`
  (`ui/src/components/Conversation.tsx`) are component state. Leaving a run with ‹ Runs and coming back, or switching
  to another role's tab and back (AgentPanes keys Conversation by agent), shows Resume or the request's options
  enabled again while the first is under way; after a 202 the server no longer holds the run's resume lock, so a
  second resume ends the first one's agents. A second answer is refused with 409 `already_answered` and shown. The
  stream's reconnect remounted RunDetail too, until ticket gui-review/07 kept it mounted. What would close it: keep
  both in App's state, keyed by project and run (and agent), or have the server refuse a resume while one it started
  is still starting.
- **A conversation can miss an ACP agent's journal for a turn.** Review 359: the engine saves a role's new agent into
  state.json (`engine/src/workflow/turn.rs`) before `Agents::start` opens its journal (`engine/src/acp.rs`), so a
  record that lists the agent can come first; the conversation's first read then answers 404, and it reads again
  only when the run's record changes next (`readsAgain`, `ui/src/runs.ts`), which may be when the turn is recorded.
  What would close it: open the journal before the agent is saved, or have the server answer an agent with no
  journal yet as an empty session rather than 404.
- **The app words a finished run's verdict by a copy of the engine's rule.** `finishedVerdict` (`ui/src/runs.ts`)
  repeats `outcome_verdict` (`engine/src/cli/listing.rs`): a non-APPROVE verdict from a run judged by several steps
  names the last step. A change to one passes every test of the other. Left when the run record became typed
  (`RunRecord`): a worded verdict in the record would not retire the copy while the app still reads servers that
  do not send it. What would close it: a record key with the worded verdict, and the app using it once every server
  it supports sends it. Kept by choice (the user's ruling, 2026-11-27, todo-tickets): not worth it while the two rules
  rarely change.
- **The desktop app as the terminal client too: what was decided against.** The app, started with arguments, is the CLI
  (`cli` in `ui/src-tauri/src/lib.rs`), and the plain CLI binaries stay released: 13 MB against the AppImage's 120 MB of
  WebKit, and they start without FUSE on headless machines and in CI. Decided against
  (2026-11-11, when the app became its own `serve` sidecar): the window serving its runs in
  its own process, with or without the CLI there too. The AppImage's environment, which GTK needs and the engine's
  children must not get, would then go through the engine as a parameter (herdr, `ProcessHost`, `ConfigDir`, the HTTP
  clients, `current_dir`) where the sidecar takes the user's once at its start (`UserShell::enter`); an engine panic
  would end the window; and `serve`'s signal handling needs the main thread. Its one gain is the first-line handshake
  gone.
- **The local gate's report names an empty `judge/gate.toml` for a base without `gate.toml`.** Raised by
  cross-review 171 (F7): "the gate counts the file as there and reports `<…>/judge/gate.toml` as where the settings
  came from … It does not report 'the defaults (there is no …)', so quality.md hides that the defaults judged the
  change." octoflow-gate 0.10.0, as 0.7.0, refuses a `--config` that names no file or names a `pyproject.toml`
  (`JudgedBy::for_tree`, `parse_args`), and without `--config` it reads the change's own `gate.toml`, so the empty
  file is what keeps the change's settings out. Would close when octoflow-gate takes the judge's rules and settings
  without a settings file, as a `--rules` option, or reports an empty one as its defaults. Decided against (the user's
  ruling, 2026-11-11): either costs a gate release and a `BUILT_IN_PIN` bump for the name of one file in the report.
- **Only a round's first Builder report that changes nothing tracked skips the quality step.** Kept by choice (the
  user's ruling, 2026-11-24, todo-tickets): the narrowing of todo 90's wording in `take_turn`'s `unjudgeable`
  (`engine/src/workflow.rs`) costs one gate run in a rare case.
- **No test runs the pinned Node's real npm.** Decided against (the user's ruling, 2026-11-24, todo-tickets): the
  tests' fake npm (`acp_adapter/tests.rs`) knows only the npm rules added to it; an opt-in install against a local
  registry is not worth it, and a real install by hand at a pin raise catches a rule it does not know.
- **A GitHub token reaches tools in directories the build user can write outside the workspace.** Accepted (the
  user's ruling, 2026-11-24, todo-tickets; review 276, todo 91): the Version stage's comment in `Jenkinsfile` says
  what is and is not guaranteed. It rests on only this repository's pull requests building on the agent, which the
  job's configuration would show (whether it discovers fork pull requests); that page is admin-only, so it is
  unchecked from here. Reopen if the job builds pull requests from forks.
- **No coverage-guided fuzzing of the engine's parsers.** Decided against (the user's ruling, 2026-11-11): a
  cargo-fuzz pass over `handoff::first_line_word`, `parse_verdict`, `parse_gate`, `parse_blocked` and
  `parse_env_file`, looking for panics. The seeded generated-input tests (`test_support::Rng`) cover them instead.
- **A non-editing step between two verdict steps is refused, not tracked.** Decided against (the user's ruling,
  2026-11-11, GitHub #352, `.scratch/todo/issues/10`): tracking every step's rounds through its turn records, so that
  such a step's `again` prompt and `{prev_X_path}` name rounds it ran; `Pipeline::loop_problem` refuses the shape
  instead.
- **The desktop app's window has a minimum width.** Decided against (the user's ruling, 2026-11-11, GitHub #357,
  `.scratch/todo/issues/15`): letting the project bar's fields shrink so that its last button stays inside a narrow
  window; `tauri.conf.json` sets `minWidth` instead.
- **A quality round's Reviewer is told to weigh a loosened lint config.** Decided against (the user's ruling,
  2026-11-11, GitHub #377, `.scratch/todo/issues/35`): having the gate take clippy's rule files from the base commit,
  which is a change in octoflow-gate; the Reviewer's quality notes say to weigh an edited one instead.
- **No command removes the `orchestrator-ci/*` branches a release before 0.72.0 pushed.** With the Jenkins gate gone
  (drop-jenkins-gate/02, 2026-11-13), so are `delete_ci_refs` and `clean_up_quality`, which deleted a run's snapshot
  branches on origin; a run that had one in flight when it stopped leaves it there for good. Raised by cross-review
  191 (F6, nit). Decided against with the spec, which removed the snapshot refs whole: docs/releases/v0.72.0.md says to
  delete them with `git push origin --delete`. Reopens if a user reports many left.

- **No `doctor` check that a harness can start in the project without asking.** Claude Code's folder-trust dialog
  stops a run before its first turn; run-usability/02 made the run say so and the README list it, but `doctor` does
  not check it. Decided against reading Claude Code's trust record (`~/.claude.json`, `projects.<path>`) on
  2026-11-05: an undocumented file of another tool, which would break silently when it changes. Closes if Claude
  Code documents a way to ask whether a folder is trusted. The eval runner writes that record all the same, for its
  trials' clones, which no human is there to answer for (`xtask/src/claude_trust.rs`).
- **Writes to origin are not tried again through an outage.** `reaching_origin` (`engine/src/workflow/origin.rs`)
  wraps the run's fetches and its `ls-remote` reads only; the pushes (`engine/src/host.rs`'s `push_ref`,
  `push_branch`), the remote branch deletions (`delete_remote_branch`, `delete_remote_branch_at`) and the `gh` calls
  fail at the first refusal, as GitHub's incident of 2026-11-04 refused known keys. A write that reached origin but
  reported failure needs its own handling first: a delete tried again finds no ref, and a push under a lease sees
  the value it set. Left out of origin-retry ticket 01 (2026-11-04). Would close with each write's retry made safe to
  repeat, then routed through `reaching_origin`. Decided against (the user's ruling, 2026-11-11): a run a write fails
  is resumed once origin is back, and making each write safe to repeat costs more than that resume.
- **Host::fetch does not wait out an outage by itself.** Each caller wraps it in `reaching_origin`
  (`engine/src/workflow.rs`'s five, `cli/prune.rs`'s `prune_remote`), since a
  `Host` (`engine/src/host.rs`) has no clock to wait on and no human to tell; a new caller gets no retry unless it
  wraps too, and a wrapped `fast_forward` or `merge_upstream` would also retry a local merge whose error happened to
  say a phrase of `UNREACHABLE`. Deferred from cross-review 135 (F7, nit, 2026-11-04). Would close with the waiting
  and the telling reachable from `Host`, so that its origin reads retry themselves. Decided against (the user's
  ruling, 2026-11-11): the wrapped callers work, and a clock and a human in `Host`'s interface cost more than a new
  caller remembering to wrap.

- **The engine's Ctrl-C, what is left.** A SIGINT raises the `Interrupt` (2026-10-29), which stops the command the
  engine waits on (`crate::interrupt`), and every command of the binary takes it. Not closed: (1) A
  Ctrl-C during a PyPI or crates.io request (`crate::http`, ureq) waits for the request to end, up to its timeout:
  there is no process to stop, and ureq has no way to abandon a
  request from another thread. The user accepted that wait for a Ctrl-C (2026-10-29, review 100). A SIGTERM or a
  SIGHUP waits so too (cross-review 102, F4: "a `kill <pid>` or a service manager's stop
  during the dependency check's PyPI request waits up to the request
  timeout"); a service manager that sends SIGKILL after a shorter grace
  kills the engine with the run unreleased. Decided against (the user's ruling, 2026-11-11): a request the Interrupt
  can cut short, by slices of the timeout or a thread nobody waits for; the wait is accepted for SIGTERM and SIGHUP
  too, since the timeout bounds it. (2) Under macOS, `by_signals`
  reads no `/proc/self/status` and takes the kernel to discard a signal that was ignored when the engine started, even
  while blocked, as the BSD kernels do, so `serve`, which waits for SIGINT and SIGTERM also when it started with one
  ignored (`crate::serve::serve_command`), does
  not stop at one it started ignoring there, and `run_process` gives a command the whole 250 ms grace, nix having no
  `waitid` there; unchecked on a Mac. Decided against (the user's ruling, 2026-11-11): `engine/tests/signals.rs`, which
  is Linux's (it reads `/proc` and starts the engine with GNU `env`), ported to macOS and run on one, since no Mac runs
  the tests; reopen when a Mac user reports a hang. A process that left the command's group, as a daemon does, is not
  reached, as at a timeout; that is no gap, as the terminal's Ctrl-C does not reach one either. (3) On a Linux whose
  `/proc/self/status` cannot be read, as in a container or sandbox without `/proc`, `ignored_signals` finds nothing
  ignored, so `by_signals` waits for a signal the engine started with ignored and takes it, Linux queueing a blocked
  signal whatever its disposition (cross-review 102, F2: "an engine under `nohup` in a container or sandbox without
  /proc is hung up: the Interrupt is raised, the run released as interrupted", and `interrupt::exit`'s raise of the
  still-ignored signal returns, so the program exits 129 by `std::process::exit`). The gap is the same for SIGINT,
  SIGTERM and SIGHUP. nix reads a signal's disposition only through `sigaction`, which is `unsafe`. Decided against (the
  user's ruling, 2026-11-12): that query, a second `unsafe` block, or refusing to start on a Linux without `/proc`; such
  a container under `nohup` is rare.
- **The commands the engine runs outlive an engine that is SIGKILLed.** Each command runs in a process group of its
  own, so a SIGKILL of the engine, or of its group, ends the engine and leaves a running
  `herdr agent wait`, `gh pr create` or push to finish unwatched. SIGINT, SIGTERM and SIGHUP stop them
  (`interrupt::SIGNALS`, 2026-10-29); a SIGKILL cannot be caught. The e2e scenarios' kill
  kills each process below the engine too (`Invocation::kill`, `engine/e2e/tests/e2e/world.rs`), as "a crash or a lost
  machine" does; before, `kill_in_publish` got a pull request from the orphaned `gh`. Decided
  against (the user's ruling, 2026-11-11): the command's group killed when the engine dies, by a subreaper or a
  watchdog (Linux's `PR_SET_PDEATHSIG`, set in `run_process`'s `pre_exec`, reaches only the command itself, not what
  it starts); a SIGKILL allows no cleanup, and a resumed run takes over the agents it finds alive.
- **The engine's HTTP transport behaves otherwise than OpenSSL and curl in a few cases.** Its transport to PyPI and
  crates.io is ureq (`HttpClient`, `engine/src/http.rs`, 2026-10-28), which sends a request for a plain-http URL
  through a proxy with CONNECT (documented in README's Requirements: the user's ruling, 2026-11-12, GitHub #410). TLS
  trusts the certificates of the places the environment names, else of the system's as openssl-probe finds them, each
  read by rustls-native-certs (`root_certs`), which trusts otherwise than OpenSSL in five ways (the user's rulings,
  2026-10-28, cross-review 93, F22 and F23): with `SSL_CERT_FILE` set non-empty and `SSL_CERT_DIR` unset, or the
  reverse, it reads only what is set, where OpenSSL adds the system's default directory or file (with the other set
  empty, OpenSSL adds nothing either), so an `SSL_CERT_FILE` that cannot be read, such as a removed virtualenv's,
  trusts none where OpenSSL still trusts `/etc/ssl/certs`; a variable set empty counts as unset, which departs from
  OpenSSL only where the other is unset or empty too: with `SSL_CERT_FILE` empty and `SSL_CERT_DIR` unset the system's
  bundle is trusted too, where OpenSSL reads no file for it, with `SSL_CERT_DIR` empty and `SSL_CERT_FILE` unset the
  system's directories are, where OpenSSL reads no directory for it, and with both empty the whole system store is,
  or Mozilla's roots on a machine with no certificate, where OpenSSL loads no certificate at all; it reads every file
  of a certificate directory, where OpenSSL opens only the files named by a subject's hash, from `.0` up to the first
  missing or broken one, so a certificate under another name, or behind a broken link, is trusted here only; without
  either variable it takes the system's bundle and directories from openssl-probe's list of places, not from
  OpenSSL's compiled-in ones, also on macOS, where it does not read the keychain; and without either variable, on a
  machine with no certificate at all, it trusts Mozilla's roots (webpki-roots), where OpenSSL trusts none and fails
  every https request. The timeout bounds the connect and each read and write, but not name resolution. In what it
  reads (`engine/src/pypi.rs`, `engine/src/checks/dependencies.rs`): a manifest is read as TOML 1.1, which Python's
  tomllib before Python 3.15 refuses in part, and why one is not TOML is the toml crate's words with `(at line L,
  column C)`; and rustls refuses a server certificate that is a CA's own, as a self-signed one made by `openssl req
  -x509` with its default extensions is (`CaUsedAsEndEntity`), where OpenSSL accepts it when it is trusted, so the e2e
  scenarios' PyPI has a leaf under a CA of its own (2026-10-29). The rest is decided against unless a setup is found
  that needs it. Decided against (2026-10-28, design-it-twice): `PyPI` as a trait of its own with an in-memory fake.
  The engine's tests would then word every URL and error twice, in the code and in the fake; with the seam at `Http`,
  `FakePyPI` and `FakeCratesIo` play the servers and every line of `PyPI` and `CratesIo` runs in those tests. The user
  kept the `Http` seam (2026-10-28, cross-review 93, F1).
- **No build runs the Rust doctests.** cargo-nextest does not run doctests, and since the Test stage's `cargo llvm-cov
  nextest` replaced the UI stage's `cargo test`, nothing does: `octoflow_ui_lib` is an `rlib` among its crate types
  (`ui/src-tauri/Cargo.toml`), so `cargo test` ran them. The crate has no doc example today. Raised by cross-review 72
  (F3, 2026-10-27): "a doc example added later that fails to compile or asserts wrongly passes CI unnoticed".
  Decided against for now: a `cargo test --doc` beside it, which builds the dependencies a third time, uninstrumented,
  under the agent's 3 GB, for no example. Closes with `cargo test --doc --locked` in the Test stage once a doc example
  lands, or with cargo-llvm-cov's `--doctests` once it runs on a stable toolchain.
- **The e2e scenarios record `state.json` at each command the fakes get, not after each save.** The ledger reads it
  only when herdr, gh or PyPI gets a call, and leaves out a change to `heartbeat_at` alone
  (`engine/e2e/src/ledger.rs`, `changed_states`). Raised by cross-review 65 (F2, 2026-10-27): an engine that skips or
  reorders saves between two of those calls, or writes a wrong intermediate state that a later save overwrites, still
  passes. Only the engine's own process sees every save. Decided against (the user's ruling, 2026-11-11):
  a way to see each write of `state.json` from outside the process (inotify has no stdlib binding, so a new
  dev-dependency); `state.json` at each command the fakes get is the contract.
- **serve answers some malformed requests abruptly or silently.** serve is on tiny_http (`engine/src/serve.rs`),
  which handles a few requests no client of serve's sends in ways serve does not choose: a request line or header with
  a byte beyond ASCII gets no answer, its connection closed, rather than a 400, or a 401 for such a token
  (`non_ascii_tokens`); a header's value loses its trailing blanks, so a `Host` or `Authorization` header with one is
  judged without them; a header continued on a folded line is refused with 400; a netloc with one square bracket in an
  absolute request target is read as it is (`crate::serve::url::urlsplit`); tiny_http answers a bad request line, an
  `Expect` other than `100-continue` and HTTP/2 itself (400, 417, 505), and neither those nor the 501 to another
  method are logged on stderr; and a reply names tiny_http in its `Server` header. Every security check still refuses
  what it is for; these change only how. Decided against unless a client is found that sends such a request.
- **serve has no limit on its threads, and no timeout for a client that sends nothing.** The engine's serve answers
  each request on a thread of its own, and tiny_http keeps a thread for each open connection, kept alive between
  requests (`engine/src/serve.rs`); cross-review 105, F3: "A client that sends a partial request or idles holds it for
  good." Decided against for now (2026-10-29, the user's ruling): serve binds 127.0.0.1 only and every request needs
  the token. Reopen if serve listens beyond loopback.
- **The e2e scenarios compare serve's event stream at checkpoints, not event by event.** `serve_endpoints_and_events`
  keeps, at each point where the run waits on an agent and once it is done, the last `record` and `files` event of each
  run (`engine/e2e/tests/e2e/scenarios.rs`, `last_by_run`). Raised by cross-review 65 (F10, 2026-10-27), against the
  spec's "serve's JSON and SSE events" byte for byte: "missing, repeated or reordered intermediate events pass." How
  many events a step sends depends on where serve's one-second poll falls among the run's saves, so the stream between
  two checkpoints differs from run to run. Decided against (the user's ruling, 2026-11-11): a serve whose poll the
  harness can step, a hook in the engine; the folded stream is the contract, as the desktop app reads it.

- **unexpected-paths reads only the contract step's file.** `unexpected_paths` judges each path the change touches
  against `RunChange.spec`, the contract step's file, so a workflow whose contract is in business terms, such as
  the built-in `product-team`, whose requirements name no file and whose `design.md` lists the
  components, gets a finding to weigh for every path on every quality round. Raised by review 20 of
  product-team-workflow (F4, 2026-10-25): "the gate reads file names from the spec and this spec is in business
  terms by design". The workflow file's header says so. Decided against (the user's ruling, 2026-11-11): the check reading
  every non-editing step's file that precedes the first editing step, or a workflow key naming the file that holds
  the paths; only the contract step names paths in the built-in workflows, and the finding is one to weigh.
- **Is 3 GB the right memory limit for the agentbox0 Jenkins agent?** All builds on the node share its container's
  `mem_limit: 3g` (Homelab `jenkins/agentbox0.yaml`), which this repo cannot change. Since `ci/cargo-lock.sh`, two of
  this repository's builds compile Rust at once only when the Gate stage's clippy, outside the lock, checks the desktop
  shell's own crate, the one crate the UI stage's `cargo check` leaves it; but everything else they run (the gate's
  cargo-mutants, npm) still shares the 3 GB beside a locked rustc, as do other jobs' builds on the node. Decided
  against (2026-11-03): raising it; the 3 GB stays until a build is OOM-killed, which reopens it.
- **A server of the session `octoflow` left running by an older herdr counts as ready.** `server_runs`
  (`engine/src/herdr.rs`) takes the line `status: running` alone; herdr 0.9.1's `herdr status server` prints beside
  it `endpoint_compatible: yes`, `private_protocol: 22`, `private_protocol_compatible: yes`. The session is left
  running by design, for weeks, with nobody attached; after a herdr update its server is the old binary. If herdr
  then refuses the CLI's commands as incompatible, every Start fails at `workspace create` with herdr's error, and
  nothing says that `herdr session stop octoflow` clears it. Raised by cross-review 169 (F5); not reproduced, since
  what herdr answers in that case is unknown. Decided against (the user's ruling, 2026-11-11): the engine restarting
  a server whose `private_protocol_compatible` is not `yes`, which guesses at what herdr answers; reopen when a
  herdr update shows the failure, then name the stop in the error.
- **`doctor` checks gitleaks's `git` subcommand, not its version.** README asks for gitleaks 8.19 or later; the
  check reuses `Host.gitleaks_problem`, the run's pre-push check, which asks `gitleaks git --help` because a
  distribution's build may print no version number. So `doctor` shows no gitleaks version. Kept by choice
  (2026-10-24).
- **Proofs run after the last round's review too.** No Builder turn follows to read them; only the draft pull
  request's review shows them, and each may take `PROOF_TIMEOUT`. Kept: a demonstrated defect is what the human
  most needs to see in a draft. Raised by the same review.
- **A run that is never closed keeps its local refs.** A locking step's commit stays under
  `refs/orchestrator/<key>/locked/` (`RunState::lock_ref`) until the run ends `done` (`Workflow::clean_up_locks`) or
  is closed (`close_run`, `engine/src/cli/close.rs`). Left: the refs of a failed or stale run nobody closes
  (`close --all` closes them all in one command), of one closed by a release whose `close` left them, and of one
  whose run directory was deleted by hand, which `close` cannot find; their locked snapshots are never collected by `git gc`. `git for-each-ref refs/orchestrator/` lists
  them, and `git update-ref -d` deletes one. Decided against on 2026-10-25: `prune` deleting them, and a sweep of
  `refs/orchestrator/<key>/` whose run directory is gone.
- **An agent's pull request can reach `main` on rules it wrote itself, with no human.** Jenkins's App reports
  `continuous-integration/jenkins/pr-head`, which the agents' token cannot post, but a pull request build runs
  the pull request's own `Jenkinsfile` and gate (`docs/quality-gate.md`, What the Gate stage judges), and since
  `automerge.yml` a pull request into `main` merges once that status is green. Raised by the cross-review of
  `pr-automerge` (F5, 2026-10-24): "A branch the agents push under any other name, once a pull request is open on
  it, merges into `main` (and so into a release) without a human". The orchestrator's own pull requests, under
  `orchestrator/`, were held for a human until the user decided on 2026-10-24 that every pull request of this
  repository merges on a green build; so a run's change now reaches `main` and a release unreviewed too. Decided
  against (the user's ruling, 2026-11-11, keeping that of 2026-10-24): an approving review required on `main` that
  the agents cannot give (Homelab `.scratch/agent-identity`, ticket 03), or a Jenkinsfile and gate a pull request
  build takes from `main` rather than from the pull request; the risk is accepted.
- **A pull request moved off `main` can merge unchecked before `automerge.yml` turns auto-merge off.** No ruleset
  requires a check outside `main`, so once a pull request with auto-merge on targets another branch and has no
  conflict, GitHub merges it at once, racing the run that would turn auto-merge off; and a conflict cleared from
  the base side starts no run. Raised by the cross-review of `pr-automerge` (R3-1(b), 2026-10-24): "After the push
  that resolves the conflict, the PR is CLEAN against a base with no required check, so GitHub merges it at once
  and unchecked", "In (b) that run still races GitHub's immediate merge. A conflict cleared by a change to the base
  rather than the head raises no pull-request event at all." Decided against (the user's ruling, 2026-11-11): a
  ruleset that requires the check on every branch, or one that restricts merges outside `main`.
- **The secret scan does not look inside a change git diffs as binary.** Neither gitleaks nor `allow_comments`
  (`engine/src/host/scan.rs`) reads a change `git log -p` shows as "Binary files ... differ"; which changes those are
  is in `Host::scan_secrets`'s doc. gitleaks skips one before any rule runs, path rules and its own `pkcs12-file`
  included (tested with gitleaks 8.30.1, cross-review 147), so a secret in it is pushed unreported. That includes a
  text file replacing a binary one. Decided against (2026-11-03, kept 2026-11-05 after the path-rule fallback was
  found not to work, secret-scan-gaps/02): scanning such changes as text with `--text`. Reopen it if such a secret is
  ever pushed; a check of file names (`.p12`, `.pfx`, `.jks`) would catch keystores but not the text-replaces-binary
  case.
- **Four copies of the test writer of an executable script.** A test's script is written by a child process, so that
  no test thread's fork holds it open for writing when it runs (ETXTBSY; ui-test-text-file-busy/01, 2026-11-03). The
  same `sh` line is in `tool` (`engine/src/host/tests.rs`, which says why), `Dir::tool` (`engine/tests/program.rs`),
  `World::make_bin` (`engine/e2e/tests/e2e/world.rs`) and `put_executable` (`ui/src-tauri/src/lib.rs`). Raised by
  cross-review 123 (F2, nit): "A correction to the writer has to land in all four, and a copy that is missed brings
  the flake back in that crate." Decided against (the user's ruling, 2026-11-11): one writer in a test-support crate
  of the workspace's own; the scratch directories removed on drop, which such a crate would also have held, are the
  `tempfile` crate's since todo/48, so it would hold this writer alone.
- **The starter `gate.toml`'s Rust and npm tests name no failing test.** `starter_settings`
  (`engine/src/local_gate.rs`) guesses `cargo test --workspace` and `npm test` without a `junit` report: neither
  writes JUnit without configuration that `doctor --setup-gate` does not write (nextest's through a
  `.config/nextest.toml` profile, jest's through a reporter), so a failing test fails the gate as tests that could not
  be judged, naming the command, not the test. Kept by choice (doctor-gate-setup ticket 01, 2026-11-11): setup writes
  only `ci/gate.pin` and `gate.toml`. Closes, if wanted, with setup also writing the reporter's configuration.
- **opencode's exported config stays set in its pane's shell.** Cross-review 174 F3: "Nothing unsets
  `OPENCODE_CONFIG_CONTENT` after the agent exits … A failed or `--no-pr` run keeps its workspace; an `opencode` the
  user starts in that pane inherits the run's model and its `<run dir>/*` allow rule." `Herdr::export` says so
  (`engine/src/herdr.rs`). Nothing can be typed into the pane while the agent runs, and herdr 0.9.1's `agent start`
  takes no environment. Decided against (the user's ruling, 2026-11-11): the engine typing an `unset` into the pane
  once the agent has exited; `Herdr::export` documents it, and it reaches only that pane. Revisit if herdr's
  `agent start` takes `--env`.
- **FakeHost plays git command lines, not repository operations.** The Workflow tests' `FakeHost`
  (`engine/src/host/fake.rs`) answers each git command `Host` sends, so a flag changed in `engine/src/host.rs` has to
  change in the fake too. Decided against (the user's ruling, 2026-11-11, from the architecture review): moving the
  fake's seam up to repository operations (fetch, fast-forward, merge, push). At command level `Host`'s own steps run
  in every Workflow test, and since a command the fake has no arm for fails the test, a flag changed in one file and
  not the other is caught.
- **`Host` is one trait over every concern.** `Host` (`engine/src/host.rs`) covers what its doc comment lists, from
  processes and files to gh and the run store, and `FakeHost` fakes them at mixed levels. Decided against (the user's ruling,
  2026-11-27, from the architecture review): splitting it into run store, pull requests and git repository traits.
  No caller would take a narrower one: `Workflow` (`engine/src/workflow.rs`) calls git, files, gitleaks, gh and the
  run store, and `cli/prune.rs`, `cli/close.rs`, `cli/forget.rs` and `local_gate.rs` each call two concerns or
  more. The fake's levels are decisions already taken: git at command level (the entry above), the run store one
  in-memory copy (`host/fake/memory_runs.rs`). Revisit if a module comes to need only one concern.
- **serve's `RunRequest` checks harness kinds and role flags that the command line checks again.** `RunRequest`
  (`engine/src/serve/start.rs`, `parse`, `by_role`, `argv`) checks what `engine/src/cli/args.rs` checks
  (`role_agent_arg`, `role_model_arg`). Decided against (the user's ruling, 2026-11-11): validating by running
  `parse_args` on the built argv and returning clap's error. The desktop app shows serve's `error` text verbatim
  (`ui/src/api.ts`), and clap's is several lines with `Usage:` and "try '--help'"; `serve/start/tests.rs`'s round
  trip through `parse_args` already catches the two checks drifting apart.
- **Restoring takes time of the paths restored times the paths tracked.** Decided against for now (the user's
  ruling, 2026-11-27, todo-tickets): no run has hit it; reopen when one does. `Host::restore` (`engine/src/host.rs`)
  hands the paths it keeps to one `git restore --pathspec-from-file`, which matches every pathspec against every
  path the repository tracks. Restoring 2000 paths took 0.11 s of CPU in a repository of 2000 files and 4.85 s in one
  of 40000 (cross-review 348, 2026-11-27, idle machine), and it all runs under Host's `DEFAULT_TIMEOUT` of 60 s: so,
  extrapolated and not tried, a repository of some hundreds of thousands of files cannot have a few thousand changed
  paths put back by `restore_tree`, the more so on a loaded machine. Found by GitHub #684, whose test was shrunk
  instead. What would close it: writing the kept paths from a temporary index of ref (`read-tree`, then
  `checkout-index --stdin`), which looks each path up once, or a timeout that scales with the paths.
