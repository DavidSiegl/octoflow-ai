# Architecture

A map of the code for someone about to change it. Usage is in the [README](../README.md), and the reasons behind
resume and stale-run detection are in [design.md](design.md). Code is named by symbol, not line, so this stays
true as lines move.

## The quality gate

The quality gate is not here: it is [octoflow-gate](https://github.com/DavidSiegl/octoflow-gate), a project of its own,
whose release `ci/gate.pin` names. Jenkins runs it ([quality gate](quality-gate.md)); the engine never does, and reads
the report it writes ([the report as a contract](quality-gate.md#the-report-as-a-contract)).

## `engine/`: the engine

The orchestrator, `octoflow-engine` in the top-level Cargo workspace: a library crate and the `orchestrator` binary
(`engine/src/main.rs`). It is the port of the Python engine, `orchestrator.py`, which it replaced in 0.49.0
(rust-engine ticket 11). It read and wrote every file, prompt and run state as that engine did until
octoflow-without-python's tickets replaced each such imitation with Rust's own; `state.json` has had a format of its
own since then, and a run an earlier release saved is refused (`state::format_problem`). Where a doc comment names a Python function or class, it is the one the Rust code was
ported from. By module:

| Module | What it holds |
|---|---|
| `json`, `map` | JSON is serde_json's `Value`, an object keeping its keys' order (`json::Object`, made by `json::object`), with `ValueExt`'s `is_set`, `text` and `kind` for a value read from a file; `Map`, a map from strings in insertion order for the engine's own tables (roles, steps, agents), which serializes in its order. |
| `constants`, `error`, `env`, `paths` | The constants, `OrchestratorError`, with `shortened` and `closest_name`, the "did you mean" of an unknown name, for messages; `child_env`, `ci_credentials` and `parse_env_file`; `ConfigDir` is `config_dir` and where its files are; `file_name` and `split_extension`, the parts of a path the engine names files by. |
| `prompts` | The built-in prompts, `Prompts` (by `Prompt`), `load_prompts`, `prompt_problem` and `saved_prompts`; `template`, the language of prompts, handoff file names and `proof_command`: `{name}` and the doubled braces `{{` and `}}`, nothing else. |
| `pipeline`, `workflow_file` | `Step`, `Pipeline` with all its checks, `default_workflow`, `find_workflow`, `is_built_in`; `parse_workflow`, `load_workflow_file`, `workflow_files`, `named_workflow`, `workflow_definition`, `workflow_toml`, `run_pipeline` and `saved_pipeline`. |
| `state` | `RunState`, saved by serde with `"format": 2` (`STATE_FORMAT`) first and the fields of a `state.json` it does not know kept, after its own; `format_problem`, why a `state.json` of another format is not read. |
| `handoff`, `quality` | `parse_verdict` and `parse_blocked`; the quality-file reader, `parse_gate`, `gate_failures`, `finding_fingerprint`, and the engine's own quality files, `leak_report` and `build_failed_report`. |
| `host`, `patch` | `Host`, a trait whose every operation is a provided method built on running one command (`exec`), so a fake overrides only what it plays: `ProcessHost`, the machine reached by real processes, here or over `ssh -o BatchMode=yes` with `ssh_sharing_options`' shared connection, each command in a process group of its own, its output read to the end on threads of their own, and the whole group killed with SIGKILL when it runs out of time; the secret scan (`scan_secrets`, of the commits a push would publish or of those since a base, as `Scanned` says, finding `quality`'s `Leak`s, `LeaksFound`); `ChangedFile`, `RunFile`, the pull request views with `GhError::Unusable` for `GhUnusable`, and `PushError::Leaks` for `LeaksFound`. Output kept verbatim is bytes, and a path git prints is refused when it is not UTF-8. `FakeHost` (`host/fake.rs`, tests only) is the Python tests' `FakeHost`, git played one command at a time through `git_run`, its other commands real processes when a test sets `processes`, as the Python tests' `host._run = subprocess.run` makes them, its time read from a `FakeClock` there, the Python tests' `FakeClock`. `added_lines` reads `change_diff`'s diffs. |
| `harness`, `herdr` | The agent harnesses: `HARNESSES`, one row per harness of what the Python engine kept in four tables, its permission, resume and directory arguments and its session logs, found by `harness(kind)`, each `Harness` giving its `permission_args`, `launch_args` and, from its `SessionLog`, `session_usage`; `AgentSettings`. `Herdr`, a trait of the commands a run gives herdr (the CLI's argv, JSON and error codes are `ProcessHerdr`'s alone, so a fake plays agents, not the protocol), `HerdrError` with herdr's code, `Agent` (herdr's record, its session included), `Direction`; `ProcessHerdr` runs the CLI here as `ProcessHost` runs a command, forwarding `--machine`, and has the commands no `Workflow` gives: `ssh_target`, `notify` and `machine`, and the `AgentConsole` serve reads and types into a run's agents through (`read`, `submit`, `send_keys`), each named by an `AgentAddress`, its herdr name and its run's server, and the `Launcher` serve starts a run's command by (`launch` in a new workspace's pane, `exited`). `HerdrServer` is which of this machine's herdr servers the CLI reaches, by `HERDR_SOCKET_PATH` and `HERDR_SESSION`: `Herdr::server` gives the one a run's agents are in, which the run saves as `RunState.herdr_server`. `FakeHerdr` (`herdr/fake.rs`, tests only) is the Python tests' `FakeHerdr`, a role's turns scripted as closures; a turn that raises the run's `Interrupt` fails its prompt, as Python's turn that raises KeyboardInterrupt. |
| `http`, `ci`, `local_gate`, `pypi`, `crates_io`, `package_index` | `Http`, the one seam to Jenkins, PyPI and crates.io, as `urlopen` is the Python engine's: one request, its whole 2xx answer, or `HttpError` with the status or, for no answer, URLError's words; `quote` and `urlencode` write its URLs' parts and forms; `HttpClient` sends it with ureq, through the proxy `http_proxy` or `https_proxy` names unless `no_proxy` names the host, trusting the certificates of the places `SSL_CERT_FILE` and `SSL_CERT_DIR` name, else of the system's as openssl-probe finds them, each read by rustls-native-certs (`root_certs`), and leaves an `Authorization` header behind on a redirect. `Jenkins` is the Python engine's `CI` class: each call of the quality job, its errors a `CIError`, `Request` (Python's `CIError`, with `status` and `transient`) or `Refused` (its `OrchestratorError`), with the credentials only in the Authorization header and Jenkins's text masked (`mask`); `QualityGate`, a run's `--quality-gate`, is such a `Jenkins` job or `Local`, the gate `local_gate` runs on the host: its `preflight`; `set_up`, which writes a project's missing `ci/gate.pin` and starter `gate.toml` for `doctor --setup-gate` and serve; and `start`, which starts a job on the host, detached, that fetches the octoflow-gate release the run's base pins into the host's cache, checked against its sha256, and runs it on a checkout of the snapshot by the base's settings: a `Judging`, whose `verdict` is None while the job runs and then a `LocalVerdict`, and whose `stop` ends it. `PyPI` is Python's, and `CratesIo` asks crates.io as it asks PyPI, with a User-Agent naming the project; both are a `PackageIndex`, which answers what it knows of a package as an `IndexedPackage`, and `PackageIndexes` holds one of each for the new-dependencies check. The tests' `FakeJenkins` (`ci/fake.rs`), `FakePyPI` (`pypi/fake.rs`) and `FakeCratesIo` (`crates_io/fake.rs`) play the servers behind `Http` as `FakeCI` and `FakePyPI` play them behind `urlopen`, so every URL, answer and error of `Jenkins`, `PyPI` and `CratesIo` runs in them; `http/tests.rs` runs `HttpClient` against a local server. |
| `checks` | The orchestrator's checks: `RunChange`, `Finding`, `CheckFindings`, `OrchestratorCheck`, `orchestrator_checks`, `judge_run` with the `RunFindings` it adds to a build's report, `diff_budget`, `unexpected_paths`, and in `checks::dependencies` `new_dependencies` with `DEPENDENCY_ECOSYSTEMS`, `python_dependencies` (pyproject.toml's lists, uv's indexes and sources, `uv.lock`), `cargo_dependencies` (a Cargo workspace's manifests and `Cargo.lock`), `PackageSource`, `package_key` and `spec_names`. |
| `workflow`, `runs`, `interrupt`, `spec_file` | `Workflow`, the phase machine (below): a run in the project's checkout or a worktree, blocked reports, a seeded spec, locked files (`workflow/locks.rs`), proofs (`workflow/proof_runs.rs`), calls that reach `origin` tried again through a short outage of it (`workflow/origin.rs`, `reaching_origin`, which `prune` uses too) and the quality gate's rounds (`workflow/quality_rounds.rs`: the snapshot scanned for secrets, then pushed and Jenkins's build polled to its quality file, or judged by the local gate; the orchestrator's checks added, the loop back to the gated step and the refs deleted): given its `Collaborators` (`Host`, `Herdr`, `Clocks`, `Human`, which is Python's `notify` and `log`, the `Interrupt`, `pid_alive`, and the quality gate, `Option<&QualityGate>`, with the `PyPI` its checks ask) and `RunSettings`, `run` ends in a verdict or `Stopped` (`Failed`, `TakenOver` for `RunTakenOver`, `Interrupted` for a Ctrl-C). `run_health`, `pid_alive` and `local_host`; `Interrupt`, the Ctrl-C, which the Workflow checks at each pause and which stops the command a `ProcessHost` or `ProcessHerdr` waits on (its group gets SIGINT, then SIGKILL, in `run_process`), raised by the thread `by_signals` starts for SIGINT, SIGTERM and SIGHUP, each unless it was ignored at start, after which `interrupt::exit` ends the program by the first SIGTERM or SIGHUP, as the Python engine dies by it; `SystemClocks` and `LocalHuman` (`workflow/system.rs`), the real run's clocks, whose sleep a Ctrl-C ends, and human, Python's `notify_locally` and `log`; `SpecFile` and `read_spec_file`, run --spec's spec, which `RunSettings::spec` gives a run that starts at `Pipeline::seeded_start`. `workflow/fixture.rs` (tests only) is the Python tests' `make_workflow`, `saved_run` and `resume`, with `Told`, the human as the tests read it, and `host/fake.rs`'s `FakeClock` as the Workflow's `Clocks`. |
| `cli` | The `orchestrator` program (`engine/src/main.rs`, which ends by `interrupt::exit`): `main`, which calls `by_signals` once it has read the arguments, for every command but `serve`, and its commands (the Python engine's `gui`, its tk window, was not ported); `run_command` dispatches them, and `herdrless_command` those that need no herdr pane, `workflows`, `judged-in-quality` and `doctor`, given a `Local` (this machine's `Host`, a herdr maker, Jenkins and the current directory). `cli::args` is `parse_args` on clap (`Command`, with `Target`, `Settings` and `Starts`, the flag groups argparse's parent parsers share), `check_flag_values` and `check_spec_args`; `cli::start` is `new_state`, `resumable_state` (with `find_run` and `resume_quality_limit`) and `resumed_state`, which turns the gate on or off by `turn_quality_gate_on` and `_off` (`workflow/gate_switch.rs`); `Connected` is what a command is given once `connect` has reached herdr and the host, the Workflow's `Collaborators`, how to make a run's `Jenkins`, standard output and error, and the `RunWorkflow` that drives a run (`run_workflow`), so the tests run commands on the fixture's fakes and script runs as Python patches `Workflow.run`; `finish`, `resume_command` and the interrupted note end it. `cli::listing` is `list`'s and `show`'s (`run_record`, `run_rows`, `run_outcome`, `run_round`, `print_runs`, `fill_pr_states`, `show_run`); `cli::prune` is `prune_branches`, `pull_request_pending`, `sweep_summary` and `sweep_branches`, which a `run` that opens a pull request calls first; `cli::close` is `close_run`; `cli::workflows` is `workflows_command`; `cli::refresh` is `ConflictCheck`, `has_conflict`, `refresh_conflict` and `refresh_command`; `cli::drain` is `drain_command` and `refreshed_headers`, and `cli::drain::queue` the spec queue (`QueuedSpec`, `read_queue`, `blocker_cycle`, `header_field`, `with_header_field`, `replace_local_file`); `cli::judged` is `judged_in_quality`; `cli::doctor` is `check_setup` and `report_checks`, without Python's tkinter check, and `doctor --setup-gate`'s call of `local_gate::set_up`. A Ctrl-C outside a Workflow fails the command it stops, which `main` takes for the interrupt (exit status 130). |
| `serve` | `serve`, on tiny_http (below): `serve_command`, `RunServer` (Python's `RunServer` and `RunRequestHandler`, a thread per request), `project_host` and the `Reached` project it gives; in `serve::start` `RunRequest` (`POST /run`'s body and the `run` command it is), `StartWait`, `Started` and `offered_workflows`; in `serve::url` `urlsplit` and `parse_qsl`, a request target's path and query; in `serve::setup` `Setup`, the desktop app's setup checks and marker, and `project_checks`; in `serve::runs` `ProjectRuns`, `AgentInput`, `plain_name`, `RunReadError` (`RunPathRefused`), `RunWatch` and its `Event`s (`sse_event`). |
| `pull_request`, `summary`, `proofs` | `spec_title`, `branch_name`, `verdict_line`, `commit_note`, `provenance`, `pr_body` and `without_conflict_warning`; `run_summary`, `run_metrics`, `turns_table`, `turn_lines`, `turn_tokens`, `format_duration`, `format_age`, `iso_utc`, `epoch_time` and `epoch_seconds`, times read and written by jiff; `ProofRun`, a `RunState.proofs` record as the Workflow reads and writes it, and `proofs_section`. |

Its tests are each module's `<module>/tests.rs`, the Python tests of those parts ported case for case, and checks
against the Python engine as the oracle: `engine/tests/oracle/` holds what the Python engine gave for the engine's
inputs, each `.json` written by a generator that ran it, removed with it, and the e2e recordings (`engine/e2e/recorded/`) are read as they are: every `state.json` they hold
reads into `RunState` and writes back byte for byte, every workflow the repository defines parses to the Python
engine's definition, every recorded run's commit note, trailers, pull request body and `summary.md` come out as the
Python engine wrote them, `metrics.json` as the recordings hold it, and every prompt the scenarios recorded is made of the
engine's templates (`prompt_parity.rs`). `host/tests.rs` and `host/scan/tests.rs` run `Host` on real git in
temporary repositories, with gh, gitleaks and ssh played by scripts on the `Host`'s `PATH`, which `ProcessHost`
takes from the environment it is given rather than the process's. `harness/tests.rs` replays the Python engine's
launch arguments for every harness, permission mode, model, session and directory, and its token counts of sample
session logs; `herdr/tests.rs` replays every herdr command the e2e scenarios recorded through `ProcessHerdr`, and
checks each recorded agent start against the launch arguments of the run's state as last saved before it.
`workflow/tests.rs` and its submodules, each holding one or more Python classes, run `Workflow` on `FakeHost`,
`FakeHerdr`, `FakeClock`, `FakeJenkins` and `FakePyPI`, the Python tests of it ported case for case.
`checks/tests.rs` runs `judge_run` on every quality file an e2e recording holds, from the build's quality.md its
scenario's Jenkins served, gives the recorded file, and reads the same failures from the judgement it adds them to.

## The phase machine

```
spec ──▶ build ⇄ review ──▶ publish ──▶ done
          ⇅
       quality   (with --quality-gate)
```

That is the default workflow's; in general a run's `phase` is the id of its current step, or `quality`,
`publish` or `done`. `Workflow::run` starts from the saved `phase` and `round`; a new run is a resume from the
first step. Before anything else, `check_start` checks that the run can finish: `check_alone` refuses a run
in the checkout while another one there is live (a worktree run neither refuses nor counts); in round 0 (before
its first editing step), a worktree run's `open_worktree` checks for a branch and an `origin`, fetches the base
branch and adds the worktree at `origin`'s (or finds it from before), `check_gate` runs `CI::preflight` and
checks the working tree, and for an in-place run `check_repo` fast-forwards the base branch; in a later round,
a worktree run's `check_worktree` refuses a run whose worktree is gone unless `pr_url` is set, and a resume
that starts in the gated step or in `quality` checks the CI credentials. Each fetch of the run, and the
`ls-remote` that lists its CI refs, goes through `reaching_origin` (`workflow/origin.rs`), which waits out a short
outage of `origin`. Then `walk` takes the steps in order, in `work_dir`:

- Each step's turn is `take_turn` and `step_turn`, with the prompts `prompts` fills in through
  `placeholder`. A human-paced step, the default's `spec`, has no timeout or stall notice and focuses its pane.
- `goto` makes the next step current and saves it. Before the first editing step, the default's `build`,
  `switch_to_branch` fast-forwards the base branch again and creates the branch (in a worktree run, from
  `origin`'s base branch fetched again, without touching the checkout), `base` is taken from HEAD and round 1
  starts.
- `quality`, after each turn of the quality-gated step when the gate is on: `quality` writes `quality-N-Q.md`.
  `analyse` snapshots, pushes and has Jenkins judge the change, saving each link of the chain in `ci` as it
  learns it, so a resume continues from the furthest one, and the round's quality file is the `quality.md` the
  build archived. `poll` retries a failed request until `QUALITY_TIMEOUT` and keeps the heartbeat going.
  A gate that does not pass goes back to the gated step, whose answer is `build-N-qQ.md` in the default.
- The verdict step, the default's `review`, records `verdict`; `CHANGES_REQUESTED` goes back to its loop target
  in the next round until `max_rounds`, resetting `quality_round`. A workflow without a verdict step records
  `FINISHED` once its last step is done, which counts as `APPROVE` (`SUCCEEDED`) for the exit status and the
  draft, while `verdict_line` and `commit_note` say that no agent reviewed the change. With the gate, a last
  quality round that did not pass (`gate_passed`) records `QUALITY_GATE_FAILED` instead, which does not count.
- `publish`, only with a pull request: `publish` commits, merges `origin`'s base branch in (recording
  `conflicts` and aborting when it conflicts), pushes and records `pr_url`, skipping any step already done.
  Then `leave_work_dir` checks the base branch out again, or in a worktree run removes the worktree, only
  logging a failure.
- `done`: `clean_up_quality` deletes the run's throwaway branches, `owner` is cleared,
  and with a pull request `close_workspace` closes the workspace.

A failure goes through `release`, which records `error`. Each role's turn is `turn`: it takes a handoff file that
already exists, and otherwise gets the agent ready through `agent` (`How`: `New`, `Alive`, `Resumed` or `Restarted`; an
exited agent is resumed into its saved session only if its record's `kind` is the role's harness now, and otherwise
starts fresh), prompts it with what `prompt_for` picks, and polls for the file in `await_handoff`, where
`tell_human` notifies the human of a blocked or stalled agent once. A verdict step's file without a `VERDICT` first
line goes to `retry` (in `verdict_turn`), which moves it aside and, once per file (`retried`), prompts the role
again with `RETRY_VERDICT_PROMPT`; `retrying` names the file while that turn is under way, so a resume sends the
retry prompt, not the step's own. A file of a step that can block (`Step::can_block`: neither human-paced nor the
verdict step) whose first line is `BLOCKED:` (`parse_blocked`) goes to `block`, which saves `blocked` and then
moves it aside to `.blocked-K`; `unblocked` then has `block_answer` notify the human and wait for the file again,
as often as it comes back `BLOCKED:`, with `BLOCKED_PROMPT`, no timeout and no stall notice, and a resume with
`blocked` set finishes an undone move (`finish_block`) and goes back into that wait. `prompts` appends
`BLOCKED_NOTE` to every prompt of such a step. `save` checks the owner before every write and stops the run as `Stopped::TakenOver`
after a takeover; `heartbeat` keeps `state.json` fresh while a turn runs. `written` and `quality` open a turn's
record in `turns` (`turn_record`, started by `Host::now`) and `end_turn` closes it at the handoff file's
`Host::mtime`, or at the start when that mtime reads earlier; both read the project host. The end of `run` and
`release` write `summary.md` (`run_summary`,
with `turns_table`), except after a takeover; `pr_body` adds the same table and `show` prints `turn_lines`. A run
that is done first counts each agent turn's tokens (`count_tokens`: `Host::session_log` reads the session's log
where its harness's `SessionLog` says its harness keeps one, and `turn_tokens` sums the answers within the turn), then writes
`metrics.json` (`run_metrics`), with the quality rounds `quality` records in `quality_rounds` by `gate_failures`;
the `metrics` command prints every run's (`Host::run_metrics`).

## Workflows

A workflow is data: a `Pipeline` with a name, its roles in pane order (role key to pane label) and a tuple of
`Step`s, and optionally a `proof_command`. A step names its id (also its phase), its role, its handoff file (fixed
like `spec.md`, or one per round like `build-{n}.md`), a first prompt, a later-round prompt (`again`) and a note for
a fresh session in a later round (`fresh_note`), and the flags `human_paced`, `edits`, `loop_to` (which makes it a
verdict step), `quality_gated` and `locks`. Prompts take `{task}`, `{cwd}`, `{n}`, `{change}`, `{path}`,
`{proofs_dir}`, `{findings}` and, for every step `X`, `{X_path}`, `{prev_X_path}` and `{earlier_X_paths}`.
`Pipeline` validates itself when constructed and refuses the definition, naming the problem, such as a duplicate id, a
loop target that does not come earlier, two gated steps or an unknown placeholder.

A workflow may have several verdict steps (`Pipeline::verdict_steps`), such as a Tech Lead's review back to the
Builder and then a Product Manager's acceptance back to the Builder too. A round runs them in order and ends at the
first that does not approve (`Workflow::take_turn`): before round `max_rounds` the run goes back to that step's loop
target in the next round, and in round `max_rounds` it goes on past the last verdict step, so a later verdict step
never judges a change an earlier one rejected. Every verdict step judges the change a round ends with, because
`loop_problem` refuses a loop target after the first verdict step, as a loop must run every earlier verdict step
again, and an editing step between the first verdict step and the last, whose edits the earlier ones would not judge.
One round counter, and one `max_rounds`, serve them all, so `{n}`, `build-{n}.md` and the quality loop are as with
one. `RunState.verdicts` records each verdict step's verdict per round (`RunState::record_verdict`), and
`RunState.verdict`, the run's verdict, turns to `CHANGES_REQUESTED` at a request for changes and to `APPROVE` only
when the last verdict step approves, so a state saved mid-round keeps the round before's. A verdict step's first turn
gets its first prompt even in a later round, and its `{prev_X_path}` and `{earlier_X_paths}` name only the rounds it
judged; `{findings}` names the verdict file that sent the run into the round, for the loop target's `again`. A state
saved before there were those records has none and loads all the same, its one verdict step taken to have judged
every round. The first verdict step is the proof step (`Pipeline::proof_step`), the only one whose proofs
`proof_command` runs. `list` names the step that sent a run back when several judged it (`run_outcome`), and
`list --json` and serve's `/runs` carry the records as `verdicts`.

The quality-gated step is the one the gate follows: its answer to quality round Q inserts `-qQ` before its file's
extension, other steps' prompts name its last report of the round, and `QUALITY_PASSED_NOTE` or
`QUALITY_UNRESOLVED_NOTE` ends each verdict step's prompt. The contract step, a first step with a fixed file that
does not edit, titles the branch and the pull request; without one the task does. The pull request body holds the
commit's provenance trailers as a table (`provenance`), the contract, the last editing step's last report, the
last quality file and the last round's verdict files, one per verdict step that judged it; with more than one
verdict step, its verdict line (`verdict_line`) names each verdict of the last round.

A locking step (`locks`: the first editing step, which no verdict loop runs again, so that what it locks is
its own work) locks the change as it stands when each of its turns ends: `Host::lock` snapshots the tree into the
local ref `refs/orchestrator/<key>/locked/<step>`, which nothing pushes and `done` deletes, and `RunState.locked`
saves the paths that differ from the base. After every other editing turn, `Workflow::hold_locks` writes the turn's
changes to those paths to `locked-<turn's file>.diff` in the run directory, saves a record of them in
`RunState::restored`, and then restores the paths from the ref (`Host::restore`, the working tree only, a path the ref
lacks deleted); a resume restores a saved record again. That happens before the quality round, so the gate judges
the locked files, and `LOCKED_NOTE` tells each verdict step which files of its round were restored and where the
diffs are.

A workflow with a `proof_command`, such as `python -m pytest -q {proofs}`, has its proof step back findings with
tests in `{proofs_dir}`, `RunState.proofs_dir(n)` in the run directory, so they never enter the snapshot or the
commit. After a review that requests changes, `Workflow::judge_proofs` runs each proof file with the command in the
working tree (`run_proof`: exit status 1 is a failing proof, which demonstrates the finding its name's first number
names; 0 passes and anything else errors, neither demonstrating), saves the outcomes in `RunState.proofs`, and
appends `proofs_section` under `PROOFS_HEADING` to the review; a resume runs no proof of a round that has records,
and appends the section only where the review lacks it. A proof runs from a copy of its directory inside the tree
it judges, `<work_dir>/.orchestrator/proofs/<round that wrote it>/`, so that pytest, looking upwards from it for its
configuration, meets the working tree's first (`run_proof` has the details); the review and the notes name the proof
where the verdict step wrote it, and say where it ran. The tree is left as the proof found it, ignored files aside:
`run_proof` snapshots it before the proof (`Host::tree_snapshot`) and afterwards puts back every file that differs
(`Host::restore_tree`), so nothing a proof writes elsewhere in the tree, such as a coverage file or a `__pycache__`,
reaches the commit; outside a git repository, or where git cannot snapshot the tree, nothing is put back. A
proof runs under GNU `timeout` where there is one, which stops its whole process group. A demonstrated proof is run
again in every later round until it passes or its file is gone: `Workflow::rerun_proofs`, right before the verdict
step's turn and so after the editing steps' turns and the quality loop, runs each carried proof (`carried_proofs`: its
last run failed, or a rerun found it erroring; one whose file a rerun found gone, as a proof the Builder moved into the
project is, is listed once as moved and carried no further) from the proofs directory of the round that wrote it, and
records the outcome in `RunState.proofs` with the round it ran in and, as `written`, the round that wrote it
(`proof_written` reads a record saved before there was one as a first run); a resume runs no proof again once the
round's reruns are recorded. `CARRIED_PROOFS_NOTE`, after the quality and locks notes, tells the verdict step which
proofs still fail or now error, with their last lines, and which now pass, and asks it to weigh one that does not pass
as a finding that stands unless a report shows the proof itself is wrong; the reruns leave the reviews themselves alone.
`PROOFS_NOTE` ends the next round's prompt of the step the review loops back to, naming every carried proof, from
whichever round wrote it.

`WORKFLOWS` holds only `DEFAULT_WORKFLOW`, worded by the built-in prompts; a run with prompt files gets it from
`find_workflow`, worded by its own. Other workflows are TOML files: `run --workflow NAME` finds
`NAME::toml` in `workflows_dir` (`~/.config/ai-agents-orchestrator/workflows/`), and `run --workflow-file` takes
one by path; the README's [Your own workflows](../README.md#your-own-workflows) gives the format. A file's
step can `use` a default step and override some of its fields; a role without a label keeps the default
workflow's label for its key, or else is named after the key; a role's `model` sits under every model flag,
and its `agent` (`Pipeline::agents`, one of `AGENT_KINDS`) under `--agent` and `--role-agent`. A
file cannot take a built-in workflow's name. `Pipeline` also rejects a prompt or file that is not a template of
known placeholders (`template_problem`, by `crate::prompts::template`), and a handoff file outside the run
directory or named like the run's own
`state.json`, `summary.md`, `metrics.json`, `quality-*` and `locked-*` files, and two steps whose files can share a name in some round (`Step::name_forms`,
`shared_name` and `collision_problem`, an exact check). `state.json` saves
the workflow's name and, for one that is not built in, its canonical definition (`workflow_definition`), which
a resume rebuilds the `Pipeline` from, so editing or deleting the file does not change a started run. One saved
before workflows existed loads as `default`. Panes: the first role takes the root pane, the second
splits right of it and each further one splits down from the one before; a workflow file's roles are in the
order its steps first use them. The tests define three more workflows
that run on this engine: Builder ⇄ Reviewer with the task as the contract, Spec Collector → Test Writer →
Builder ⇄ Reviewer, and an Implementer ⇄ Reviewer whose gated step writes `impl-N.md`.

## serve

`serve_command` (`engine/src/serve.rs`) starts a `RunServer` on tiny_http, on a free port of `127.0.0.1`, which hands
each request to a thread of its own; prints `{"url", "token"}` as one line; and serves until SIGINT or SIGTERM, then
returns 0. `crate::cli::main` runs it before it starts the other commands' signal handling, and `serve_command` blocks
SIGINT and SIGTERM and waits for them itself, so that SIGHUP keeps its default. Stopping the `RunServer` (dropping it)
ends every event stream and raises the `Interrupt` its projects' `ProcessHost`s run their commands by, so a poll an SSH
round trip holds up ends too. Like `doctor`, it needs no herdr pane. It reads the projects' files and writes none but
the local quality gate's, by `POST /setup/gate` (`local_gate::set_up`); it changes otherwise only what a run's agent is
given to read, by `/agent/input`, as the human would type it into the agent's pane; the desktop app's setup marker,
`setup-checked` in its configuration directory (`ConfigDir`), by `POST /setup/checked`; and, by `POST /run`, which runs
started, as the human would start one from a herdr pane.

- **Projects.** Each request names its project by `cwd` and an optional `machine`. The server connects each
  pair once through `project_host`: the `Host` that reaches the machine, the directory resolved on it, and the
  `AgentConsole` (`herdr.rs`) that reaches the herdr the runs' agents are in, here or forwarded to the machine. It
  keeps them as a `ProjectRuns`, with `target_args` of the pair for the records' resume commands. A run started or
  resumed without a machine saves in its state which herdr server of this machine it reached (`herdr_server`: the
  engine's `HERDR_SOCKET_PATH` and `HERDR_SESSION`, each set or unset), and the console sends each command to an
  agent with exactly those two variables, whatever session serve itself runs in or none: so a window started from
  a file manager still reaches agents a run started in a named session's pane, and a run in the default session is
  reached there from a window inside another session. Only a run saved before runs saved it is reached in the
  session serve's own environment picks: `HERDR_SOCKET_PATH`'s, else `HERDR_SESSION`'s, else the default one. On a
  machine, herdr forwards to the machine's own herdr, and nothing is saved.
- **Starting a run.** `POST /run` (`serve/start.rs`, `ProjectRuns::start`) makes a run id as `run` would
  (`new_run_id`), opens a workspace in this machine's herdr server, the one serve's environment reaches, labelled
  as the run labels its own (`workspace_label`), and runs its own binary's `run --run-id ID …` in the pane's
  shell (`Launcher`, `herdr.rs`), so the run can be watched and interrupted there and outlives serve. herdr's
  `pane run` types what it is given, so each word goes to the new shell as an environment variable instead, and
  only `"$OCTOFLOW_ARG_0" …` is typed. A project on a machine is started from here too, as
  `run --machine M --cwd DIR …` in a workspace in serve's own directory: the run's process is local and its agents
  are on the machine, as from the CLI. The
  hidden `--run-id` lets serve then wait for exactly that run: its `state.json` saved answers 201; the command's
  end without one, told by a line the shell prints after it, answers 422 with what the pane showed; neither
  within 30 seconds (`START_WAIT`) answers 202. When the server serve's environment reaches is not running, the
  workspace opens in the herdr session `octoflow` (`OWN_SESSION`) instead, whose server the `Launcher` starts
  first, headless, if it is not running either (`sh -c 'herdr server … &'`, then `herdr status server` until it
  prints `status: running`; it exits 0 either way). That server outlives serve; the run saves it as its
  `herdr_server`, so its agents are reached there as any run's.
- **Endpoints.** Every request needs the token, and every one but `/setup/checked` and `/workflows` names the
  project by `?cwd=` and, optionally, `?machine=`:

  | Endpoint | Gives |
  |---|---|
  | `/runs` | `{"version": 1, "runs": [...]}`, `list --json`'s document (`run_record` of each of `Host::run_states_inside`, Security below) |
  | `/files?run=ID` | the files in the run's directory that stay inside it (`Host::run_files_inside`, Security below), oldest first, each `{"name", "mtime"}` |
  | `/file?run=ID&name=NAME` | `{"run", "name", "text"}` for one of them, read through `Host::read_inside` |
  | `/agent?run=ID&role=ROLE` | `{"run", "role", "text"}`: the last lines (`&lines=N`, 200 by default) of what the run's agent of the role shows, through `herdr agent read` |
  | `POST /agent/input?run=ID&role=ROLE` | types the JSON body's `{"text"}` into that agent as a prompt (`herdr agent prompt`), or presses its `{"keys"}` (`herdr agent send-keys`) |
  | `/events` | a `text/event-stream` of `record`, `files` and `problem` events; see Events below |
  | `/setup` | `{"checks", "passed"}`: `doctor`'s checks of the project (`serve::setup`), each `{"name", "level", "ok", "detail", "fix"}`, reaching herdr from a pane only `optional`, and whether every `required` one is ok; it does not connect the project, so one that cannot be reached is a failed check |
  | `/setup/checked`, `POST /setup/checked` | `{"checked"}`: whether the setup marker is there; the POST writes it |
  | `POST /setup/gate` | `{"files": [{"path", "written"}]}`: writes the project's missing `ci/gate.pin` and starter `gate.toml` (`local_gate::set_up`), `written` false for one it kept |
  | `/workflows` | `{"workflows", "agents"}`: each workflow `run --workflow` offers from serve's configuration directory, with its description and its roles' labels, harnesses and models, and the harnesses there are |
  | `POST /run` | starts the run its JSON body asks for (task, workflow, agent, model, role_agents, role_models, no_pr, worktree) and answers `{"run"}`; see Starting a run above |

  `RunServer`'s doc comment is the authority on them.
- **Events.** Each `/events` stream has its own `RunWatch`. It polls `run_states_inside` every second, and every 5 for
  a `--machine` project (`SERVE_POLL`), and compares what it reads with what the stream
  last sent. A run whose record changed, other than in `AGING_RECORD_KEYS`, gets a `record` event, and one whose
  files changed a `files` event; a run whose `state.json` is gone gets a `record` event with `null`. The first
  poll sends every run. An unfinished run's files are read on every poll, a finished or failed run's only when
  its record changes. A project that cannot be read gives one `problem` event, and another with `null` once it
  can. A stream with nothing to send writes an SSE comment instead, so a client that has gone is noticed at the
  next poll.
- **Security.** Any local user can connect to a port on `127.0.0.1`, and any web page can ask the browser to.
  So `RunServer` checks every request, in this order. It answers 403 to a `Host` header that is
  not `127.0.0.1:<port>` or `localhost:<port>`, which is how a DNS-rebinding page's requests arrive. It also
  answers 403 to an `Origin` that is not the server's own or one given with `--origin`. The Tauri shell passes
  its webview's origin: `tauri://localhost`, or Vite's dev server under `tauri dev`. It then wants
  the token, 32 random bytes as `secrets.token_urlsafe` wrote them, compared in constant time as bytes
  (`same_bytes`). It may come as `Authorization: Bearer` or,
  for an `EventSource`, which cannot set headers, as `?token=`; without it the answer is 401. A CORS preflight
  (`OPTIONS`) carries no token, so it gets only the `Host` and `Origin` checks, and a 204 that allows `GET` and
  `POST` with `Authorization` and `Content-Type`. `Access-Control-Allow-Origin` echoes only an origin that passed. `ProjectRuns` lets a
  run id or file name be only a plain name (`plain_name`: no `/`, not `.` or `..`) and refuses `state.json`.
  It refuses such a path before anything is read (`RunReadError::Refused`), which the server answers with 403. A
  symbolic link is followed only as far as it stays inside: a file that, links resolved, is not in its run
  directory, links resolved, and every file of a run directory that is not in the project's `.orchestrator/runs`,
  are refused with 403 by `/file` and left out of `/files` and the event stream's `files` events. A run whose
  `state.json` is refused so is no run: `/runs` and the stream's `record` events leave it out, as a run without a
  `state.json`, and `/agent` answers 404. The links are resolved on the project's host, over SSH too
  (`Host::run_states_inside`, `run_files_inside` and `read_inside`); the engine's own reads of a run's files and
  states still follow them. An
  agent is named only by its run and role, which `ProjectRuns` looks up in the run's `state.json`, so a client can
  read or type into no other herdr agent; a POST body is read to at most 64 KiB (`MAX_BODY_BYTES`). `POST /run`
  runs a command a client chose only within `run`'s flags: the program is serve's own, each flag's value is joined
  to its flag and the task comes after `--`, so no value is taken for another flag, and no word is typed into the
  shell, where a quote, a `$` or a control character such as ^U could make it a command of its own: the shell
  gets each one as an environment variable.
- **Streams.** An event stream to an HTTP/1.1 client is chunked, so that it ends with its last chunk at a stop; to
  an HTTP/1.0 client it ends with the connection. The requests tiny_http reads otherwise than the Python engine's
  http.server did are `TODO.md`'s Open issue "serve answers some requests otherwise than Python's http.server".

## The desktop app: `ui/`

A Vite + React + TypeScript frontend in `ui/src/`, in a Tauri 2 shell in `ui/src-tauri/` (`create-tauri-app`'s
layout), with no Python of its own.

- **Shell.** `src-tauri/src/lib.rs` links the engine (`octoflow-engine`), so the app's executable, `octoflow`
  (`mainBinaryName` in `tauri.conf.json`), is the orchestrator too. In `setup` it starts its sidecar,
  `Sidecar::of_app`: its own executable as `serve --origin <the webview's origin>`, which is the CLI below. It
  reads the first line and keeps the URL, the token and the project, or why it failed, which is what the sidecar
  wrote to standard error when it wrote anything, for the `server` command; the project is the checkout the app
  was built in for a development build (`tauri dev`), null otherwise. With no checkout and no project opened
  before, the frontend asks for a project folder. It kills the sidecar on `RunEvent::Exit`. The webview's CSP
  allows connections only to itself, Tauri's IPC and `http://127.0.0.1`.
- **CLI.** Started with arguments, `main.rs` runs none of that: before GTK or WebKit start a thread, `lib.rs`'s
  `cli` takes the command line and `Cli::run` runs the engine's `cli::main` with it in the app's own process, and
  ends as the engine's `orchestrator` does (`interrupt::exit`), so the terminal, the exit status and the signals
  sent to the app are the CLI's; a user's directory that is gone prints why and exits 1. So the AppImage is the
  CLI too. Run without FUSE, the process its caller started is the AppImage's runtime, which stays the CLI's
  parent and passes on no signal: the CLI asks for SIGTERM when that parent dies (`stop_with_appimage_runtime`,
  prctl(2)'s parent-death signal), so whatever stops the runtime stops it by SIGTERM. On macOS, a lone `-psn_`
  argument, which a Finder launch may pass, opens the window. Windows, which builds no app and no console, has no
  CLI.
- **The user's shell.** Before the engine starts, `Cli::run` makes the process's environment and directory
  `UserShell`'s (`UserShell::enter`), not the ones the app was started with; the sidecar, being the CLI, does so
  itself. Only an AppImage makes them differ: its `AppRun` points path lists, `PYTHONHOME` and GTK's variables
  into the bundle and starts the app in `$APPDIR/usr`, and so does an extracted one's `squashfs-root/AppRun`.
  `UserShell` drops the bundle's entries and variables (its constants list them), naming the bundle by `$APPDIR`
  and by what that resolves to, and names the directory the AppImage was started from, `OWD`, or `PWD` where an
  extracted AppImage sets no `OWD`, so the engine, which reads the process's environment and directory where it
  needs them, and every process it starts see the user's. The process that shows the window keeps the bundle's,
  which its GTK needs. Decided against: serving the window in the app's own process (`TODO.md`, Planned, "The
  desktop app as the terminal client too").
- **Frontend.** `api.ts` types the protocol: `RunRecord` mirrors `run_record`, `RunRequest` is `POST /run`'s body, and
  `Api` builds the URLs and makes the requests. `hooks.ts`'s `useRunStream` opens the `EventSource` and folds its
  events into a `RunsState` with `runs.ts`'s `applyEvent`. Every opening starts from no runs, since the server sends
  every run again. `runs.ts` also holds `progress` (the stages of a run's progress bar, with `quality` after the step
  it judges) and the wording helpers, which follow `format_age` and `run_round`. A `RunEntry` keeps when its record
  arrived, so `heartbeatAge` ticks with `useNow` between events. The components are `ProjectBar`, `RunsOverview`,
  `RunForm` (a new run, selected once `POST /run` answers) and `SetupDialog`, both a `Dialog`, `RunDetail`,
  `AgentPanes`, `StepProgress` and `HandoffFile`, which renders a file with `react-markdown` and `remark-gfm`, raw
  HTML skipped, and fetches it again whenever its mtime changes.
- **Tests.** Vitest with React Testing Library in jsdom: `App.test.tsx` replays `test/recorded-stream.json`
  through `test/fakes.ts`'s `FakeEventSource`, with `fetch` stubbed. The recording is the events a real
  `serve` sent over a scripted run: the e2e test `app_event_stream` (below) fails when serve no longer sends them,
  and `recording.test.ts` when a record in them has other keys than `api.ts`'s `RunRecord` and the types in it, so a
  key added or dropped on either side fails a build. `runs.test.ts` covers the pure helpers. `cargo test -p octoflow-ui` starts
  and stops a real sidecar.

## Tests

`cargo test --workspace` runs every test but the desktop app's frontend's (`npm test` in `ui/`, Tooling below): the
engine's (above), the desktop shell's and xtask's, among them `xtask/tests/fetch_gate.rs` and
`xtask/tests/version_raised.rs`, which cover the scripts of `ci/` they are named for.

`engine/e2e/` (package `octoflow-e2e`) holds the e2e scenarios, which run the engine as a program, with nothing of
it imported; they are the only test of a run from start to verdict. Each scenario in `tests/e2e/scenarios.rs`
scripts a `World` (`tests/e2e/world.rs`): a git project on `main` with a bare `origin` beside it, the fake herdr, gh
and ssh first on `PATH` and the real gitleaks after them (TODO.md says what a missing one does), and Jenkins, and PyPI
behind an HTTPS proxy, served from the test process (`tests/e2e/servers.rs`). The fakes are `[[bin]]` targets of the
package (`src/bin/`, their logic in `src/fakes.rs`), so that its test finds them by `CARGO_BIN_EXE_*`; they and the
test share the ledger (`src/ledger.rs`, under `flock`). The World runs `E2E_ENGINE`'s binary when that is set, as
Jenkins's Test stage sets it to the build it made for coverage, else builds the checkout's with cargo, killing and
resuming it where the scenario says, and records, normalized by `src/normalize.rs` (times, run ids, pids, hashes, its
own paths and ports): each invocation's exit status, the output programs read whole and other output only as its
facts; the commands herdr and gh got and the requests Jenkins and PyPI got, in order, with `state.json` wherever it
changed between them; the run directory's files; and both repositories' refs, commits and working trees. A scenario
whose record differs from `recorded/<scenario>.json` fails, naming where. The recordings are the Python engine's,
made by the Python harness this replaced (octoflow-without-python tickets 01 and 02) before rust-engine ticket 11
removed that engine. `E2E_RECORD=1 cargo nextest run -p octoflow-e2e` writes recordings instead, from the Rust
engine, for a scenario added since or a change to what the engine does, whose recording is then a review of it. One
test beside the scenarios, `app_event_stream`, keeps a recording outside the package:
`ui/src/test/recorded-stream.json`, the events the desktop app's tests replay, which `tests/e2e/app_recording.rs`
folds from what a real `serve` sent after each step of two scripted runs, with fixed stand-ins for what the clock
decides. A kill ends the engine's group and each process below it, since the engine runs each command in a group of
its own.

The package's other test, `tests/docs/`, holds the README and `docs/` to the code: each command the docs run takes the
flags they give it, the README's flag table gives each flag the commands that take it, every flag is documented, the
names they give in backticks exist, and every relative link and its anchor resolve. It reads the commands and flags
from the help of the same binary the scenarios run (`src/engine.rs`), and the names by parsing `engine/src` and
`ui/src-tauri/src` with syn. Each failure names `file:line`.

Golden files hold generated text to a reviewed copy of it under `tests/golden/`, so a change to that text is reviewed
as the diff of its golden files. They hold the engine's prompts: each step's prompts in
the built-in workflow and each example workflow, as `Workflow` renders them, to a file per workflow, round 1 and,
for the steps the review loops over, round 2 as the running session and a fresh one hear it; the Python engine's
tests wrote them, and the engine's `workflow/tests/default_prompts.rs` holds the Rust engine to them. The e2e scenarios
record round 1 of each workflow as sent, so a reworded prompt fails both. Of round 2 of the examples and of a fresh
session's prompts, the recordings hold only the templates, in `state.json`; the text as sent, placeholders filled in
and a fresh session's prompt put together, only the golden files hold. The engine's
other texts, its follow-up prompts, notes, commit messages and pull request bodies, have no golden files: the
scenarios record them as sent, where one reaches them. TODO.md lists the texts neither reaches.

xtask's tests (`xtask/tests/`) cover the eval runner, `cargo xtask eval` ([Evals](design.md#evals)), with the
`fake-engine` binary as its engine, a program that plays a scripted run in the clone, and the eval tasks: each bundle
is of its source, and each hidden test rejects its task's starting point and accepts a solution of it. They run the
Jenkins contract check, `cargo xtask contract` ([The calls](quality-gate.md#the-calls)), over a scripted Jenkins. No
agent runs in the suite, and no request leaves the machine.

## Tooling

- `pyproject.toml`, with `uv.lock` and `.python-version`: not a Python project, but the quality gate's toolchain, the
  Markdown linter the gate runs, pymarkdown, in the `dev` group, the Python it runs on, and its rules.
- `ui/package.json`: the desktop app's npm scripts, `dev`, `build`, `test`, `typecheck`, `lint` and `tauri`. Its
  `.npmrc` sets `legacy-peer-deps`, because npm 9.2 as Debian ships it cannot resolve Vitest's optional peers.
  Jenkins runs the frontend's `typecheck`, `lint` and `test`, and the shell's tests under cargo-llvm-cov.
- `Cargo.toml`: the Cargo workspace, whose members are the engine (`engine/`, `octoflow-engine`, whose version is the
  release's, as `ci/release-version.sh` reads it), its e2e scenarios (`engine/e2e/`), the desktop shell
  (`ui/src-tauri/`) and the repository's chores (`xtask/`, run as `cargo xtask <command>` through `.cargo/config.toml`'s
  alias: `eval`, the eval set's runner, `bundles`, which builds its tasks' bundles, and `contract`, which checks the
  engine's Jenkins client against the real Jenkins), with one `Cargo.lock`, the lints every member takes
  (`[workspace.lints]`), and `rust-toolchain.toml`, the toolchain and components the Jenkins agent's image has.
- `packaging/build-desktop.sh`: builds the desktop app with `tauri build`, at the release's version,
  through a release config the script writes and merges over `tauri.conf.json`, which bundles nothing. The app is
  the one executable in it. It writes
  `dist/octoflow-desktop-<os>-<arch>` `.deb` and `.AppImage` on Linux, `.dmg` on macOS (signed ad hoc),
  each with its `.sha256`, and refuses a bundle without `build-binary.sh`'s third-party
  licenses, which it bundles as the resource `THIRD-PARTY-LICENSES.html`. Beside it, as
  `THIRD-PARTY-LICENSES-NPM.html`, go the licenses of the npm packages the app's window bundles, which
  `ui/scripts/npm-licenses.js` writes after `npm ci` to `dist/octoflow-desktop-npm-licenses.html`, the release's
  copy: every package of `ui/package-lock.json`'s production tree, and the build tools that add code of their own to
  the bundle (the script's list of them: Vite and Rolldown), with the license text `node_modules` holds for it and,
  for a build tool, its third-party notices,
  and a package under no license `packaging/about.toml` accepts fails the build. On Linux it then runs the AppImage as the
  CLI from a temporary directory: `--help` and `workflows` exit 0, a stand-in `herdr` that `doctor` starts gets no entry of the
  bundle in `LD_LIBRARY_PATH` and no `PYTHONHOME`, `APPDIR`, `APPIMAGE` or `GTK_PATH`, and `serve` runs in that
  directory, read from its `/proc` entry, and stops when the AppImage's runtime does.
- `packaging/build-binary.sh`: builds `dist/octoflow-<os>-<arch>` (`linux-x86_64`, `macos-arm64`), the engine's
  `orchestrator` binary, `cargo build --release --locked`, and its `.sha256`, then smoke-tests it: `--help`, and
  `workflows` lists `default`. Before the build, cargo-about writes `dist/octoflow-third-party-licenses.html`, the
  license texts of the crates in the binary and the desktop app, by `packaging/about.toml` and `about.hbs`; a crate
  under a license `about.toml` does not accept fails the script.
- `Jenkinsfile`: `uv sync --frozen` for the gate, the Cargo workspace's tests with line coverage (`cargo llvm-cov
  nextest`), without the e2e scenarios in a quality build whose change touches nothing they judge (the Test stage lists
  what), a UI stage that runs `ui/`'s `npm ci`, `typecheck`, `lint` and `test`, failing the build before the gate, a
  Gate stage that runs the octoflow-gate release `ci/gate.pin` names and archives `quality.md` and `quality.json` (see
  [quality gate](quality-gate.md)), and a Package stage that runs `packaging/build-binary.sh` and
  `packaging/build-desktop.sh` on every build but a quality build (`GIT_REF` set). On `main` a Release stage publishes a
  new version (`ci/release-version.sh`) as a GitHub release, with the Linux binary and the Linux desktop app, each with
  its `.sha256`, and `octoflow-third-party-licenses.html`, in one `gh release create`, and
  `docs/releases/v<version>.md`, where there is one, before GitHub's generated notes. Its parameter `GIT_REF` builds
  another branch; such a build never releases. A pull request build into `main` fails its Version stage unless the pull
  request raises the version over `main`'s or changes only docs, tests, CI or repository tooling (`ci/version-raised.sh`
  lists those paths), so that every merge into `main` that reaches users is released.
- `.github/workflows/automerge.yml`: keeps auto-merge on for a pull request into `main` from a branch of this
  repository, so it merges with a merge commit once Jenkins' required check passes; its header lists the pull
  requests it leaves to a human.
- `.github/workflows/macos-binary.yml`: Jenkins has no Mac, so it runs `packaging/build-binary.sh` and
  `packaging/build-desktop.sh` on `macos-latest`. When a release is published, or by hand for an existing tag, a
  second job uploads `octoflow-macos-arm64`, `octoflow-desktop-macos-arm64.dmg` and their `.sha256` to that release
  with `gh release upload --clobber`; with only `contents: write`, it never creates a release or a tag. For a pull
  request that touches the packaging, the UI or the engine, it only builds, with
  `contents: read`, as an advisory check. Everything else stays in Jenkins.

How to set Jenkins up for it is in [quality-gate.md](quality-gate.md).
