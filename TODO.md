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

- **A resumed done run's late summary.md or metrics.json can miss the desktop app.** Raised by cross-review 238 (F3):
  a done run stopped before its ending wrote them is finished by a resume that writes them without saving state.json
  after (`Workflow::end_done_run`, `ending_left`'s `release: false`), so serve's event stream, which stopped listing
  the run's files once they settled, lists them again only at the record's next change (`RunWatch::poll`). A failed
  run's release saves again after its summary (todo/70); would close when this ending does the same.
- **raise-gate-pin's pin_of reads a malformed ci/gate.pin otherwise than the engine.** `pin_of`
  (`xtask/src/raise_gate_pin.rs`) takes a sha that is not 64 hex digits, which the engine's `parse_pin` and
  `ci/fetch-gate.sh` refuse, and `v0.9.2 <sha>`, which `parse_pin` refuses and `ci/fetch-gate.sh` passes on to a
  download address that does not exist; and it refuses a pre-release such as `1.0.0-rc1`, which `parse_pin` takes;
  the test holding the two together (`a_pin_reads_as_the_engine_reads_it`) feeds only well-formed pins. Raised
  by cross-review 256 (F1). Closes with pin_of calling the engine's reading, or the test feeding the diverging cases
  and the two agreeing on them.
- **serve's POST /forget archives a run's metrics.json through a link out of its run directory.** serve's sweeps act
  only on runs inside the runs directory (`RunScope::Inside`, todo/75), but `Host::forget_run` reads the run's
  `metrics.json` with a plain `read`, so a run directory inside whose `metrics.json` is a link to a file outside has
  that file's JSON added to the project's metrics archive before the directory is removed. Reading it as `read_inside`
  does, an outside one taken as none, would close it (cross-review 248).
- **Two processes forgetting one run at once can both forget it.** serve holds a per-project lock through each
  forget sweep (`ProjectRuns::forgetting`, todo/74), but that lock is the process's own: a second app instance, each
  with its own `serve` sidecar, or `octoflow forget` from a shell next to the app, can both read a closed run before
  either deletes it, both answer "forgotten", and `Host::forget_run`'s check-then-append can archive two metrics lines.
  Raised by cross-review 245 (F1). Closes with a lock file in the project's `.octoflow-ai/` that `forget_all` holds,
  over SSH too.
- **A resumed pull-request run past round 0 does not check gitleaks before its turns.** `Workflow::check_start`
  (`engine/src/workflow.rs`) runs `check_before_interview`, which checks gitleaks, only at round 0, so an ungated
  `--pr` run resumed after gitleaks was removed spends its remaining turns and fails only at publish. A gated run is
  checked again on resume (`gate_switch::check_gate_mid_run`). Raised by cross-review 250 (pre-existing).
- **Nothing checks that a workflow's actions stay pinned by sha.** todo/65 pinned every `uses:` in
  `.github/workflows/` by commit sha, and `.github/dependabot.yml` keeps a sha a sha, but the Gate stage's actionlint
  accepts a tag: a later `uses: actions/cache@v4` in a job that holds `AUTOMERGE_TOKEN` passes every check and brings
  back the moved-tag risk. Raised by cross-review 235 (F3). Closes with a check in the gate that fails a `uses:` whose
  ref is not 40 hex digits (zizmor's `unpinned-uses`, or a grep in `ci/`).
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
- **serve's `RunRequest` checks harness kinds and role flags that the command line checks again.** `RunRequest`
  (`engine/src/serve/start.rs`, `parse`, `by_role`, `argv`) checks what `engine/src/cli/args.rs` checks
  (`role_agent_arg`, `role_model_arg`). Decided against (the user's ruling, 2026-11-11): validating by running
  `parse_args` on the built argv and returning clap's error. The desktop app shows serve's `error` text verbatim
  (`ui/src/api.ts`), and clap's is several lines with `Usage:` and "try '--help'"; `serve/start/tests.rs`'s round
  trip through `parse_args` already catches the two checks drifting apart.
