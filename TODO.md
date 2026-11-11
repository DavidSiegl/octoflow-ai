# TODO

What is known to be open in the engine (`engine/`), the desktop app (`ui/`) and CI, and why. A fixed entry
is deleted; git history and the release notes (`docs/releases/`) keep what was done. An entry that a fix only narrows
is rewritten to say what is left. The quality gate's entries moved with it to
[octoflow-gate's TODO.md](https://github.com/DavidSiegl/octoflow-gate/blob/main/TODO.md) and left this one on
2026-11-06 (gate-standalone/10).

## Planned

- **The macOS app's name in Finder.** The product is Octoflow AI, but the Tauri bundle is `octoflow`, without a space
  because Tauri names the `.deb`, `.app` and `.dmg` after it verbatim. The macOS app could show "Octoflow AI" in
  Finder while staying `Octoflow.app`, with `CFBundleDisplayName` in a `ui/src-tauri/Info.plist`, unchecked on a Mac.
  Agreed 2026-10-25.

- **The desktop app as the terminal client too.** Started with arguments, the app, which links the engine, is the
  orchestrator's CLI (`cli` in `ui/src-tauri/src/lib.rs`), so the AppImage is the CLI too and the README says how to
  get the CLI from each download; on macOS, where a `.app` is a directory, the CLI is a symlink to
  `Octoflow.app/Contents/MacOS/octoflow`. The plain CLI binaries stay released: 13 MB against the AppImage's 120 MB
  of WebKit, and they start without FUSE on headless machines and in CI. Decided against: one file on every
  platform, by embedding the orchestrator's bytes in the Rust executable and unpacking them at start (PyInstaller's
  onefile done twice), or by Python hosting the window, which [ui-options.md](docs/ui-options.md) chose against.
  Agreed 2026-10-24. Decided against too, 2026-11-11 (rust-engine/12, which made the app its own `serve` sidecar):
  the window serving its runs in its own process, with or without the CLI there too. The AppImage's environment,
  which GTK needs and the engine's children must not get, would then go through the engine as a parameter (herdr,
  `ProcessHost`, `ConfigDir`, the HTTP clients, `current_dir`) where the sidecar takes the user's once at its start
  (`UserShell::enter`); an engine panic would end the window; and `serve`'s signal handling needs the main thread.
  Its one gain is the first-line handshake gone.

- **The e2e scenarios: what they do not record.** `engine/e2e/` runs 43 scenarios against the engine as a program and
  compares each with its recording (docs/architecture.md, Tests). The recordings are the Python engine's, made before
  rust-engine ticket 11 removed it, by the Python harness octoflow-without-python ticket 02 removed; a scenario added
  since records the Rust engine, and its recording is a review of the engine's behaviour. Not covered, and accepted
  as known gaps (the user's ruling, 2026-11-11), since unit tests cover each harness's arguments and its token
  parsing: the harnesses other than claude and codex starting
  an agent (only their arguments differ); a session log's token counts (every turn records `unknown`); `--timeout` and
  the stall notice, which take real minutes; the GUI, which the Rust engine drops; round 2 of the example workflows and
  a fresh session's prompts as sent, whose templates only the recorded `state.json` holds and whose text only golden
  files hold (docs/architecture.md, Tests). Where gitleaks or openssl is not on PATH, every scenario fails naming it: a
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
  own build. Decided against (the user's ruling, 2026-11-11): an `eval` command of the engine's own, since evals are a
  developer's chore, not a user's feature. Not covered: the quality gate, since the
  toy repositories have no quality job and their origin is a bundle no snapshot can be pushed to, so `quality rounds`
  stays 0; a `--worktree` or `--machine` run, which the runner refuses; and an interview, which `--spec` skips.
  Decided 2026-10-28. The verification run it still lacks is under Open issues.

## Open issues

- **The local gate's report names an empty `judge/gate.toml` for a base without `gate.toml`.** Raised by
  cross-review 171 (F7): "the gate counts the file as there and reports `<…>/judge/gate.toml` as where the settings
  came from … It does not report 'the defaults (there is no …)', so quality.md hides that the defaults judged the
  change." octoflow-gate 0.7.0 refuses a `--config` that names no file or names a `pyproject.toml`
  (`JudgedBy::for_tree`, `parse_args`), and without `--config` it reads the change's own `gate.toml`, so the empty
  file is what keeps the change's settings out. Would close when octoflow-gate takes the judge's rules and settings
  without a settings file, as a `--rules` option, or reports an empty one as its defaults. Decided against (the user's
  ruling, 2026-11-11): either costs a gate release and a `BUILT_IN_PIN` bump for the name of one file in the report.
- **A local gate whose job cannot start shows it at the first verdict, not at `start`.** Raised by cross-review 171
  (F6): "If setsid has gone missing since the preflight, the first verdict reports 'its job gone without a status:
  <job.log tail>'." LAUNCH prints the job's pid either way; the tail of `job.log` still says why. Closes with
  LAUNCH failing when the job's group never appears, which needs telling that from a job that ended within its wait.
- **A stuck loop can compare a round read from `quality.json` with one read from `quality.md`.** Raised by
  cross-review 160 (F3): "A round judged from quality.json records every failing finding's fingerprint; one judged
  from quality.md (gate_failures) records at most QUALITY_FINDINGS_LIMIT." A round of a build that has
  `quality.json` is still read from its `quality.md` when the orchestrator stopped between writing the quality file
  and saving the round, or when two reads of a running build agreed before `quality.json` was copied; with more than
  50 identical failing findings in rounds Q-1 and Q from different sources, `GateFailures.repeats` is false and the
  stuck rule misses that loop, so the Builder gets one more turn. Closes with recording the round before the quality
  file is written, or comparing only the first `QUALITY_FINDINGS_LIMIT` of a JSON-read round against a Markdown-read
  one; goes with the fallback below.

- **No `doctor` check that a harness can start in the project without asking.** Claude Code's folder-trust dialog
  stops a run before its first turn; run-usability/02 made the run say so and the README list it, but `doctor` does
  not check it. Decided against reading Claude Code's trust record (`~/.claude.json`, `projects.<path>`) on
  2026-11-05: an undocumented file of another tool, which would break silently when it changes. Closes if Claude
  Code documents a way to ask whether a folder is trusted.
- **The desktop app's recorder reads the e2e Normalizer's placeholders back out of its text, and folds events as
  `last_by_run` does.** Raised by cross-review 166 (F9, nit): "`<run:N>`, `<key:N>`, `<pid:`, `<host>`,
  `<duration>`, `<seconds>`, `<time>` and `<world>` are spelt here as well as in normalize.rs, and line 134 takes
  any `<…>` left in any record string for an unknown placeholder: a task, a BLOCKED question or an error with angle
  brackets in it panics with "no stand-in for the placeholder" once the scenario holds one"
  (`stand_in_text`, `engine/e2e/tests/e2e/app_recording.rs`); and (F8, nit) "`app_recording::recording` folds "last
  record, last files per run" as `last_by_run` does" (`engine/e2e/tests/e2e/scenarios.rs`), so a change to the fold
  has two places. Today's scenario holds no angle bracket of its own. Closes with a Normalizer that takes the
  replacements to write (the e2e placeholders, or the app's stand-ins) rather than text to parse again, and one
  fold both callers shape their output from.
- **Writes to origin are not tried again through an outage.** `reaching_origin` (`engine/src/workflow/origin.rs`)
  wraps the run's fetches and its `ls-remote` reads only; the pushes (`engine/src/host.rs`'s `push_ref`,
  `push_branch`), the remote branch deletions (`delete_remote_branch`, `delete_remote_branch_at`) and the `gh` calls
  fail at the first refusal, as GitHub's incident of 2026-11-04 refused known keys. A write that reached origin but
  reported failure needs its own handling first: a delete tried again finds no ref, and a push under a lease sees
  the value it set. Left out of origin-retry ticket 01 (2026-11-04). Would close with each write's retry made safe to
  repeat, then routed through `reaching_origin`. Decided against (the user's ruling, 2026-11-11): a run a write fails
  is resumed once origin is back, and making each write safe to repeat costs more than that resume.
- **Host::fetch does not wait out an outage by itself.** Each caller wraps it in `reaching_origin`
  (`engine/src/workflow.rs`'s five, `quality_rounds.rs`'s `delete_ci_refs`, `cli/prune.rs`'s `prune_remote`), since a
  `Host` (`engine/src/host.rs`) has no clock to wait on and no human to tell; a new caller gets no retry unless it
  wraps too, and a wrapped `fast_forward` or `merge_upstream` would also retry a local merge whose error happened to
  say a phrase of `UNREACHABLE`. Deferred from cross-review 135 (F7, nit, 2026-11-04). Would close with the waiting
  and the telling reachable from `Host`, so that its origin reads retry themselves. Decided against (the user's
  ruling, 2026-11-11): the wrapped callers work, and a clock and a human in `Host`'s interface cost more than a new
  caller remembering to wrap.
- **The npm notices' refusal of a multi-licensed package says it has no license text.** A package offered under a
  license `packaging/about.toml` does not accept, whose one license file names none of the licenses, fails
  `npmNotices` with "no one license text for MIT OR GPL-3.0-only", which sends the reader looking for a missing file;
  the refusal is that the text may be the unaccepted license's alone, and `npmNotices`'s `@throws` does not name the
  case. Raised, not recorded, by cross-review 131 (round 4, 2026-11-04). Closes with a message of its own for that
  case ("one license text for an offer not every license of which about.toml accepts") and the `@throws` naming it.

- **new-dependencies points a second source's finding at the package's first added line.** `compare_dependencies`
  (`engine/src/checks/dependencies.rs`) gives each `NewPackage` and source-changed finding the first line the change
  added to its manifest that names the package (`line_naming`), not the line of that source's entry: `foo = "1"` and
  `foo = { git = ".." }` added to one Cargo.toml both point at the first. Found while implementing ticket
  cargo-dependency-rules/01 (2026-11-04). Closes with the readers giving each source the line of its entry.

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
  on a KeyError or a TypeError, or, for a `proofs` record
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
  kills the engine with the run unreleased. Decided against (the user's ruling, 2026-11-11): a request the Interrupt
  can cut short, by slices of the timeout or a thread nobody waits for; the wait is accepted for SIGTERM and SIGHUP
  too, since the timeout bounds it. (2) Under macOS, `by_signals`
  reads no `/proc/self/status` and takes the kernel to discard a signal that was ignored when the engine started, even
  while blocked, as the BSD kernels do, so `serve`, which waits for SIGINT and SIGTERM also when it started with one
  ignored, as Python's `signal.signal` replaces an ignored disposition (ticket 10, `crate::serve::serve_command`),
  does not stop at one it started ignoring there, and `run_process` gives
  a command the whole 250 ms grace, nix having no `waitid` there; unchecked on a Mac. Decided against (the user's
  ruling, 2026-11-11): `engine/tests/signals.rs`, which is Linux's (it reads `/proc` and starts the engine with GNU
  `env`), ported to macOS and run on one, since no Mac runs the tests; reopen when a Mac user reports a hang. A process that left the command's group, as a daemon does, is not reached, as at
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
  machine" does; before, `kill_in_publish` against the Rust binary got a pull request from the orphaned `gh`. Decided
  against (the user's ruling, 2026-11-11): the command's group killed when the engine dies, by a subreaper or a
  watchdog (Linux's `PR_SET_PDEATHSIG`, set in `run_process`'s `pre_exec`, reaches only the command itself, not what
  it starts); a SIGKILL allows no cleanup, and a resumed run takes over the agents it finds alive.
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
- **No build runs the Rust doctests.** cargo-nextest does not run doctests, and since the Test stage's `cargo llvm-cov
  nextest` replaced the UI stage's `cargo test`, nothing does: `octoflow_ui_lib` is an `rlib` among its crate types
  (`ui/src-tauri/Cargo.toml`), so `cargo test` ran them. The crate has no doc example today. Raised by cross-review 72
  (F3, 2026-10-27): "a doc example added later that fails to compile or asserts wrongly passes CI unnoticed".
  Decided against for now: a `cargo test --doc` beside it, which builds the dependencies a third time, uninstrumented,
  under the agent's 3 GB, for no example. Closes with `cargo test --doc --locked` in the Test stage once a doc example
  lands, or with cargo-llvm-cov's `--doctests` once it runs on a stable toolchain.
- **The e2e scenarios record `state.json` at each command the fakes get, not after each save.** Ticket
  rust-engine/01 asks for "`state.json` after each save"; the ledger reads it only when herdr, gh, Jenkins or PyPI
  gets a call, and leaves out a change to `heartbeat_at` alone (`engine/e2e/src/ledger.rs`, `changed_states`). Raised
  by cross-review 65 (F2, 2026-10-27): "a Rust engine that skips or reorders saves between two of those calls, or
  writes a wrong intermediate state that a later save overwrites, still passes." Only the engine's own process sees
  every save: for the Python engine an audit hook through `sitecustomize`, or a `mv` on PATH that `Host.write` calls,
  would catch them, and the Rust engine could be held to neither. Decided against (the user's ruling, 2026-11-11):
  a way to see each write of `state.json` from outside the process (inotify has no stdlib binding, so a new
  dev-dependency); `state.json` at each command the fakes get is the contract.
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
  between two checkpoints differs from run to run. Decided against (the user's ruling, 2026-11-11): a serve whose
  poll the harness can step, a hook in the engine; the folded stream is the contract, as the desktop app reads it.

- **The quality file's format has two names for each part.** The engine's copies (`QUALITY_OK`,
  `QUALITY_FAILED_CONDITIONS`, `QUALITY_FINDINGS_LIMIT` in `engine/src/quality.rs`, `Finding::numbered`,
  `Finding::warning` and `Finding::error` in `engine/src/checks.rs`, `shortened` in `engine/src/error.rs`) and
  the gate's (`GATE_OK`, `FAILED_CONDITIONS`, `QUALITY_ISSUE_LIMIT`, `_numbered`, `_finding`, `_clip`) differ, so a
  grep for one misses its twin. Most date from gate-split ticket 01. Raised by cross-review 61 (F9, 2026-10-27).
  Closes with one name per part on both sides, a change to both programs.
- **`--machine` over SSH assumes a POSIX login shell and a server that takes several sessions.** Since `run_tool`
  (`engine/src/cli/doctor.rs`), doctor takes a remote command's exit status 127 for not on PATH: a csh or tcsh login
  shell exits 1 ("Command not found."), so a missing gh reads as `gh auth status fails` and a missing git as a failed
  `--version`; and a tool on PATH that exits 127 itself, such as a shim whose target is gone, reads as not on PATH. And
  `ssh_sharing_options` overrides a target's own `ControlMaster no`, so a server with `MaxSessions 1` refuses the second
  of two commands run at once on the shared connection (the desktop app and `serve` run several), which ssh then fails.
  Raised by cross-review 33 (F4, F6), neither reproduced. Closes with doctor asking through `sh -c`, as `on_path` does,
  and with an explicit `ControlMaster`/`ControlPath` in `ssh -G <target>` left in place.
- **unexpected-paths reads only the contract step's file.** `unexpected_paths` judges each path the change touches
  against `RunChange.spec`, the contract step's file, so a workflow whose contract is in business terms, such as
  `examples/workflows/product-team.toml`, whose requirements name no file and whose `design.md` lists the
  components, gets a finding to weigh for every path on every quality round. Raised by review 20 of
  product-team-workflow (F4, 2026-10-25): "the gate reads file names from the spec and this spec is in business
  terms by design". The example's header says so. Decided against (the user's ruling, 2026-11-11): the check reading
  every non-editing step's file that precedes the first editing step, or a workflow key naming the file that holds
  the paths; only the contract step names paths in the example workflows, and the finding is one to weigh.
- **Removing a tree of read-only directories is written out twice.** `chmod -R u+rwX "$dir" && rm -rf "$dir"`,
  with a warning when it fails, is in the Jenkinsfile's Test stage (emptying `TMPDIR` before the tests) and in
  `packaging/build-desktop.sh`'s `remove_temporaries`, for the AppImage extractions it removes; a fix to one does
  not reach the other. `engine/src/proofs.rs`'s `REMOVE_TREE_SCRIPT` is a looser third (`rm` runs even when `chmod`
  fails, with no warning). Raised by cross-review 132 (F6, nit, 2026-11-04): "Record or share it, as the implementer
  judges". Closes with one script both call, should a third copy be needed or the idiom change.
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
  shell's own crate, the one crate the UI stage's `cargo check` leaves it; but everything else they run (the gate's
  cargo-mutants, npm) still shares the 3 GB beside a locked rustc, as do other jobs' builds on the node. Decided
  against (2026-11-03): raising it; the 3 GB stays until a build is OOM-killed, which reopens it.
- **The herdr server serve starts keeps serve's terminal session.** With no herdr server running, `Launcher::launch`
  (`engine/src/herdr.rs`, `start_server`) starts the session `octoflow`'s as `sh -c 'herdr server … &'`: a process
  group of its own, standard streams on /dev/null, but no session of its own, since macOS has no `setsid` command and
  `pre_exec` is unsafe code. The desktop app has no terminal, so nothing follows there; a `serve` typed into a
  terminal leaves the server with that terminal as its controlling one. Not seen to end the server (its group is in
  the background, so a hangup is not sent to it), and not tried on macOS. Closes with `setsid(2)` before the exec, or
  a herdr that detaches its server itself.
- **A server of the session `octoflow` left running by an older herdr counts as ready.** `server_runs`
  (`engine/src/herdr.rs`) takes the line `status: running` alone; herdr 0.9.1's `herdr status server` prints beside
  it `endpoint_compatible: yes`, `private_protocol: 22`, `private_protocol_compatible: yes`. The session is left
  running by design, for weeks, with nobody attached; after a herdr update its server is the old binary. If herdr
  then refuses the CLI's commands as incompatible, every Start fails at `workspace create` with herdr's error, and
  nothing says that `herdr session stop octoflow` clears it. Raised by cross-review 169 (F5); not reproduced, since
  what herdr answers in that case is unknown. Decided against (the user's ruling, 2026-11-11): the engine restarting
  a server whose `private_protocol_compatible` is not `yes`, which guesses at what herdr answers; reopen when a
  herdr update shows the failure, then name the stop in the error.
- **Each `exited` look filters the environment anew.** After serve starts a run, `exited` (`engine/src/herdr.rs`)
  filters the environment for the session's server (`in_server`) at each look, every 500 ms for up to 30 s. Raised
  by cross-review 169 (F8).
- **`doctor` checks gitleaks's `git` subcommand, not its version.** README asks for gitleaks 8.19 or later; the
  check reuses `Host.gitleaks_problem`, the run's pre-push check, which asks `gitleaks git --help` because a
  distribution's build may print no version number. So `doctor` shows no gitleaks version. Kept by choice
  (2026-10-24).
- **Proofs run after the last round's review too.** No Builder turn follows to read them; only the draft pull
  request's review shows them, and each may take `PROOF_TIMEOUT`. Kept: a demonstrated defect is what the human
  most needs to see in a draft. Raised by the same review.
- **Each proof snapshots the whole working tree twice.** `Workflow::run_proof` takes `Host::tree_snapshot` before the
  proof and `Host::restore_tree` another after it, each a `git add -A` of every untracked, non-ignored file under
  `DEFAULT_TIMEOUT` (60 s): on a tree with large untracked files a snapshot over the limit makes every proof "did not
  run" or "did not run cleanly". Raised by cross-review 143 (F9, nit). Kept: one snapshot before a batch of proofs
  (`judge_proofs`, `rerun_proofs`) would save one of the two, but changes `run_proof`'s interface and every test that
  calls it, and leaves the limit in place. Closes with that, and with a snapshot timeout that scales, or the snapshot
  taking only the paths `git status` reports changed.
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
- **README and docs left over from the README rewrite (PR #90).** Raised by cross-review 50 (2026-10-26), in what
  PR #90 brought, not in code: the README no longer says which commands take only `--machine`/`--cwd`.
- **Each poll of a running build also asks for its quality.md.** Raised by cross-review 58 (F2, 2026-10-27, PR
  #105): the request is a 404 until Gate has run; asking for `artifacts[relativePath]` in the result's `api/json`
  tree would halve the requests.
- **Four copies of the test writer of an executable script.** A test's script is written by a child process, so that
  no test thread's fork holds it open for writing when it runs (ETXTBSY; ui-test-text-file-busy/01, 2026-11-03). The
  same `sh` line is in `tool` (`engine/src/host/tests.rs`, which says why), `Dir::tool` (`engine/tests/program.rs`),
  `World::make_bin` (`engine/e2e/tests/e2e/world.rs`) and `put_executable` (`ui/src-tauri/src/lib.rs`). Raised by
  cross-review 123 (F2, nit): "A correction to the writer has to land in all four, and a copy that is missed brings
  the flake back in that crate." Decided against (the user's ruling, 2026-11-11): one writer in a test-support crate
  of the workspace's own; the copies of a scratch directory removed on drop are to go to the `tempfile` crate
  instead, so no such crate is made.
- **`working_tree`'s copy of git's index resolves the index path twice.** From cross-review 167 round 2, raised and
  not recorded as a finding (`engine/src/host.rs`, `working_tree`'s script): "`cp` and `touch -r "$i"` resolve the
  index path twice. When another git process renames a new index into place between the two, a second or more after
  the old one was written, the copy holds the old entries under the newer time and the racy check is off again for
  that run"; the pass's remedy, not tried: `{ cat > "$t" && touch -r /dev/stdin -- "$t"; } < "$i"`. Not fixed with
  gitignore-restore-flake/01: it needs a concurrent index write within microseconds. Closes when the copy's entries
  and time come from one open file.
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
