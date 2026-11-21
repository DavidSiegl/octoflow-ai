<p align="center"><img src="docs/logo.svg" alt="An octopus holding a terminal, a robot and a chat bubble" width="200"></p>

<!-- The platforms badge repeats the platforms a release ships for, as "Install" and "Desktop app" list them:
     a change to one changes all three. -->
<p align="center">
  <a href="https://github.com/DavidSiegl/octoflow-ai/releases/latest"><img src="https://img.shields.io/github/v/release/DavidSiegl/octoflow-ai" alt="Latest release"></a>
  <a href="https://github.com/DavidSiegl/octoflow-ai/releases"><img src="https://img.shields.io/github/downloads/DavidSiegl/octoflow-ai/total?label=asset%20downloads" alt="Downloads of all release assets"></a>
  <a href="#install"><img src="https://img.shields.io/badge/platforms-Linux%20x86__64%20%7C%20macOS%20arm64-informational" alt="Platforms: Linux x86_64, macOS arm64"></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/built%20with-Rust-B7410E?logo=rust&logoColor=white" alt="Built with Rust"></a>
  <a href="https://github.com/DavidSiegl/octoflow-ai/actions/workflows/macos-binary.yml"><img src="https://img.shields.io/github/actions/workflow/status/DavidSiegl/octoflow-ai/macos-binary.yml?label=macOS%20binary" alt="macOS binary build"></a>
  <a href="https://github.com/DavidSiegl/octoflow-ai/commits/main"><img src="https://img.shields.io/github/last-commit/DavidSiegl/octoflow-ai" alt="Last commit"></a>
  <a href="https://ko-fi.com/davidsiegl"><img src="https://img.shields.io/badge/Ko--fi-support%20me-FF5E5B?logo=kofi&logoColor=white" alt="Support me on Ko-fi"></a>
</p>

# Octoflow AI

Coding agents in relay, no role judging its own work: a handoff workflow running in [herdr](https://herdr.dev),
each role in the agent of your choice: [Antigravity CLI, Claude Code, Codex, opencode or pi](#five-coding-agents-in-any-mix).
The default workflow:

```text
Spec Collector ──spec.md──▶ Builder ──build-N.md──▶ Reviewer ──review-N.md──▶ APPROVE ──▶ pull request
                               ▲                                   │
                               └──────── CHANGES_REQUESTED ────────┘
```

Each role is a separate interactive agent session in its own herdr pane, so you can watch or step into any of
them. The agents run on this machine or on another one saved in herdr. The [desktop app](#desktop-app) runs
them without herdr, on this machine, and shows each one's conversation. Each feature is built on its own branch
and ends as a pull request on GitHub, where you review it. With `--quality-gate local`, a quality gate run on the
project's own machine judges the Builder's change (linters, tests, coverage of new code) before the Reviewer sees it
([quality gate](docs/quality-gate.md)).

## Five coding agents, in any mix

Every role runs in the agent harness you pick for it, so the agent that reviews a change need not be the one
that wrote it:

| Harness | `--agent` | Models | `--permission-mode` it understands |
|---|---|---|---|
| [Antigravity CLI](https://github.com/google-antigravity/antigravity-cli) | `agy` | Google's Gemini | `default`, `acceptEdits`, `bypassPermissions`, `plan` |
| [Claude Code](https://github.com/anthropics/claude-code) | `claude`, the default | Anthropic's Claude | every mode, as it is |
| [Codex CLI](https://github.com/openai/codex) | `codex` | OpenAI's GPT | `default`, `acceptEdits`, `bypassPermissions`, `plan` |
| [opencode](https://github.com/sst/opencode) | `opencode` | any provider it is set up for | none; it starts with its own default |
| [pi](https://github.com/badlogic/pi-mono) | `pi` | any provider it is set up for | none; it starts with its own default |

- **A second opinion from another model.** `--role-agent review=codex` has Claude Code write the change and
  Codex review it, so the review does not share the builder's blind spots.
- **One flag for every role, or one role at a time.** `--agent agy` moves the whole run; `--role-agent
  ROLE=KIND` moves one role; a [workflow file](#your-own-workflows) can pin `agent = "codex"` to a role. Models
  work the same way, with `--model` or `--role-model ROLE=MODEL`.
- **One permission vocabulary.** `--permission-mode` takes Claude Code's mode names and gives each harness its
  own equivalent ([Harnesses](#harnesses)).
- **Sessions that survive.** An agent that exits is relaunched into its saved session, with its harness's own
  resume syntax.

```bash
octoflow run "add a token-bucket rate limiter" --role-agent review=codex --role-agent spec=agy
```

## Requirements

`octoflow doctor` checks these, here and where the agents run, and says how to fix each one that
is missing. It also checks, as optional, what [the local quality gate](docs/quality-gate.md#the-local-gate) needs
there, and which gate release judges the project; `doctor --setup-gate` writes a starter `gate.toml` into the
project, and with `--pin-gate <version>` a `ci/gate.pin` holding it at that release. Without a pin, a run is judged
by the release built into the octoflow that started its first quality round, so updating octoflow updates the gate
for the runs that start after it.

- For the CLI, herdr 0.9+ where the agents run, with the integration of each harness a run uses
  (`herdr integration install <integration>`: the kind's own name, e.g. `herdr integration install claude`, but
  `antigravity-cli` for `agy`)
- For the CLI, herdr's panes starting a POSIX shell (sh, bash, zsh, dash, ksh) or fish, which herdr takes from
  `[terminal] default_shell` in its `config.toml`, else `$SHELL` ([Harnesses](#harnesses)). The desktop app needs
  no herdr: its setup dialog checks what its runs need instead ([Desktop app](#desktop-app))
- The CLI of each harness a run uses on `PATH` where the agents run: `claude` by default
- Each harness able to start in the project directory without asking first. Claude Code, for one, asks once per
  folder whether to trust it: open `claude` there once, or answer in the pane when the run says the role
  "needs your answer before it starts". A `--worktree` run starts its agents in a new directory under the
  project, `.octoflow-ai/worktrees/<key>`, which a harness may ask about again
- Unless you use `--no-pr`: a git checkout with an `origin` it can push to, and [`gh`](https://cli.github.com)
  logged in; with `--worktree`, a branch checked out and an `origin` even with `--no-pr`
- For any run that pushes: [gitleaks](https://github.com/gitleaks/gitleaks#installing) 8.19+ and git 2.36+.
  Every push is scanned for secrets first, by the base's `.gitleaks.toml` and `.gitleaksignore`, and refused on
  a finding. A change git shows as "Binary files ... differ" is not scanned, and no rule in `.gitleaks.toml`
  reaches it: a secret in it, a `.p12` keystore say, or a `gitleaks:allow` in it, is pushed unreported. git shows a
  file's change so when either side is binary to git: most often a NUL in its first 8000 bytes or a size over your
  `core.bigFileThreshold` (512 MiB unless set), and also whenever your git configuration, the repository's
  `.git/info/attributes` or your `core.attributesFile` makes the file binary, as `-diff` does (the repository's own
  `.gitattributes` do not count). So a text file replacing a binary one is not scanned either
- Nothing for the orchestrator itself, a single [binary](#install); to build it from a checkout, Rust by
  `rust-toolchain.toml`; [uv](https://github.com/astral-sh/uv), with the Python it brings, only for
  [running the gate by hand](docs/quality-gate.md#the-gate)
- For `--machine`, which only the CLI has: the machine saved in herdr and non-interactive SSH to it, which the orchestrator's
  commands share one connection to (OpenSSH's `ControlMaster`), unless its server refuses a second session; see
  [Running the agents on another machine](docs/design.md#running-the-agents-on-another-machine)
- For `--quality-gate local`: a project on a Linux x86_64 machine, which the gate is built for, with curl, tar,
  setsid, ps, sha256sum (or shasum) and gitleaks, and the tools the project's `gate.toml` has the gate run, its test
  commands and linters, all on `PATH`
  ([the local gate](docs/quality-gate.md#the-local-gate))

## Install

Each [release](https://github.com/DavidSiegl/octoflow-ai/releases) has a single-file executable for Linux
x86_64 and for Apple Silicon macOS, each with its `.sha256`:

```bash
# Linux x86_64; on an Apple Silicon Mac, octoflow-macos-arm64 and `shasum -a 256 -c`
curl -fLO https://github.com/DavidSiegl/octoflow-ai/releases/latest/download/octoflow-linux-x86_64
curl -fLO https://github.com/DavidSiegl/octoflow-ai/releases/latest/download/octoflow-linux-x86_64.sha256
sha256sum -c octoflow-linux-x86_64.sha256 && chmod +x octoflow-linux-x86_64
./octoflow-linux-x86_64 run "add a token-bucket rate limiter to the API client"
```

The macOS binary is not signed or notarized. Downloaded with a browser, it is quarantined: right-click it and
choose Open once, or run `xattr -d com.apple.quarantine octoflow-macos-arm64`. A file `curl` downloads opens
straight away.

Everywhere else, build it from a checkout with cargo, by the toolchain `rust-toolchain.toml` pins:

```bash
cargo build --release --locked -p octoflow-engine   # target/release/octoflow
```

The examples below call it `octoflow`: put the binary on your `PATH` under that name. The desktop app's build
(below) puts the app at the same path; it takes these commands too but needs the desktop's libraries, so build the
engine after it. `octoflow --version` says which release it is.

### Desktop app

A desktop app shows your runs: one project's runs, their handoff files and their agents, live. It starts new
ones and closes those no longer running, and of a running run it changes nothing but what you type to its agents.
It is a Tauri 2 app in [`ui/`](ui). Each release has it
for Linux x86_64, as `octoflow-desktop-linux-x86_64.deb` and `.AppImage`, and for Apple Silicon macOS, as
`octoflow-desktop-macos-arm64.dmg`, each with its `.sha256`. The app is the orchestrator too, so it needs no
checkout. The macOS app is signed ad hoc only, not notarized, so macOS refuses to open it the
first time when a browser downloaded it, as it does the binary.

- **Overview:** each run's task, its progress through the workflow, its round, the current role, whether it is
  running or stale, the question it is blocked on, its verdict and its pull request, which a click opens in your
  browser. Closed runs are left out until "Show closed runs" is checked.
- **New run:** "New run…" above the overview asks for the task, the workflow (`default` unless you pick another,
  with its description; "Edit…" under it opens the workflow editor, described under Workflows below, on it, and
  "New…" on a copy of `default`; the form waits while the editor is open, and then picks the workflow saved, if it
  loads), an agent and a model for the whole run (the workflow's own unless you pick them), one row per role of the
  workflow for its own agent and model (each `--role-agent` and `--role-model`, with what an empty
  one falls back to shown greyed; a role the next workflow you pick also has keeps its row's values), a permission
  mode for every role (`default`, `acceptEdits`, `plan` or `bypassPermissions`, given as `--permission-mode`;
  `default` unless you pick another, so every harness asks before it runs a command), and two boxes: "No pull
  request", off as on the CLI, and "Separate worktree", on, so that runs started side by side do not share the
  checkout. Start runs `octoflow run` with them in the background, on this computer, its agents its own child
  processes speaking the [Agent Client Protocol](https://agentclientprotocol.com), and the app then opens the run.
  The run outlives the app, and its output goes to `run.log` in its run directory. A run that fails before it
  starts, such as one the checkout already holds, shows that log's last lines in the form. The app does not run pi
  yet, and refuses opencode under `default` and `acceptEdits`, where it would not ask before it runs a command.
- **Clean up runs:** "Clean up runs…" above the overview shows what `octoflow close --all` would do to each
  run: the workspace, refs, worktree and branch it would close, delete or keep, the uncommitted changes that would
  go with a failed run's worktree, and why a run that is still running is skipped. Close closes the runs it showed,
  skipping any that changed since, and shows how each went; the overview then leaves them out. With nothing to
  close it says so.
- **Delete closed runs:** "Delete closed runs…", beside "Show closed runs" while some run is closed, lists by task
  the closed runs `octoflow forget --all` would delete, and counts those waiting for `prune`. Delete deletes
  them, keeping their metrics, and shows how each went; the overview and the count then drop them.
- **Workflows:** "Workflows…" in the top bar lists every workflow `octoflow workflows` lists, with its file and
  description, or the error of one that does not load, which you fix by hand or delete. New starts from a copy of
  `default`, Duplicate from a copy of the one picked, and Edit opens a workflow's file (View a built-in one,
  read-only). The editor has a field for every key in [Your own workflows](#your-own-workflows) but `proof_command`
  and the diff budget, which you set in the file and a save keeps: the roles as a table, the steps as a list you add
  to, remove from and reorder, and each step's prompts beside the placeholders they may use. A moment after each
  change the orchestrator checks the workflow as `run` would, and Save stays off while it names a problem. Saving a
  file someone changed since you opened it is refused, and Reload reads it again. Delete asks first and names the
  file. A run already started keeps its own copy of the workflow.
- **Run detail:** the run as an octopus map: the head is the run, with its round, and each stage of its workflow
  (its steps, the quality gate after each gated step, and the pull request) is an arm ending in a station that shows
  who holds it, the harness and model of its last turn, its rounds and a verdict step's last verdict. The working
  arm is amber, and arrows show where a verdict step loops back to. A bubble on the head asks for your answer when
  the run waits on you. Below the map are what runs now and the run's turns, newest first. Under it, the files in
  the run's directory, the newest or the one you pick rendered as Markdown, updated as they are written. A run that
  stopped, failed or lost its orchestrator has a Resume button, which continues each agent's saved session.
- **Agents:** a tab for each role the run has started, showing its agent's conversation: what it says, its thoughts
  (collapsed), its tool calls, and its permission requests, each answered with a button. While the run waits on you
  (the interview, a blocked report's question) the tab has a box for your message. A run the app started in herdr
  before 0.94.0 shows its agent's herdr pane instead, as long as herdr can be reached here, with a box to type to it;
  such a run resumes from the CLI.
- **About:** the logo and name at the top open it: the release version, the copyright and the license, with links to
  this repository, its releases, Ko-fi, and the third-party licenses the app bundles, each opened in your browser.
- **Project bar:** a project folder on this computer; running agents on another machine is the CLI's `--machine`.
  The app reopens the project you opened last.
- **Setup:** "Check setup…" shows what the app's runs need for the open project, with the fix for each problem:
  each harness's command on `PATH` and answering the protocol, a codex login, git, gh, gitleaks and the quality gate. claude and codex
  run through an ACP adapter, which the dialog offers to install (several hundred MB each, in octoflow's data
  directory). It opens by itself when the app starts, until you have closed it once with everything a run needs
  present. Its "Set up the quality gate" button does what `doctor --setup-gate` does, without a pin.
- **Settings:** "Settings…" in the top bar sets what closing the window does with the open project's runs still
  running: ask each time (the default), keep them running, or stop them. Asked, "Keep running" leaves them running
  without the app, and "Stop all" stops each as Ctrl-C stops a CLI run, for Resume to continue; "Don't ask again"
  saves the answer as the setting.

Each download is the CLI too, with the same commands and flags; started without arguments, the app opens its
window:

- **AppImage:** the file itself, e.g. `./octoflow-desktop-linux-x86_64.AppImage run "..."` after `chmod +x`.
  Without FUSE (`libfuse2`) it runs only with `APPIMAGE_EXTRACT_AND_RUN=1` set or `--appimage-extract-and-run` as
  its first argument, and then extracts itself into `$TMPDIR` (`/tmp` by default) and leaves it there, about 340 MB
  for each version. Install FUSE, or extract it once with `--appimage-extract` and run the `squashfs-root/AppRun`
  that makes instead.
- **`.deb`:** it installs the app as `/usr/bin/octoflow`, with its [Tab completion](#tab-completion) for bash, zsh
  and fish, which works in each new shell with nothing to add to your startup files; bash's needs the
  `bash-completion` package, which minimal and container images leave out.
- **macOS:** link the app's executable onto your `PATH`:

  ```bash
  ln -s /Applications/Octoflow.app/Contents/MacOS/octoflow /usr/local/bin/octoflow
  ```

  If macOS refuses to run it, the app was downloaded with a browser: open it once with right-click and Open, or
  run `xattr -dr com.apple.quarantine /Applications/Octoflow.app`. Not yet checked on a real Mac.

- **Headless machines and CI:** the plain binaries in [Install](#install), which need neither WebKit nor FUSE.

To run the app from a checkout, it needs Node 20.19+ with npm, and Rust with
[Tauri 2's system packages](https://v2.tauri.app/start/prerequisites/). On Debian or Ubuntu:

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev pkg-config
cd ui
npm ci
npm run tauri dev     # the window, rebuilt as you edit
npm run tauri build   # an optimized build, ../target/release/octoflow, in place of the engine's binary there
npm test              # the frontend's tests; npm run typecheck and npm run lint check it too
cargo test -p octoflow-ui     # the shell's, which start and stop a real serve
```

The app has the orchestrator built in: for its window it starts itself a second time, as `octoflow serve`, and
stops that when the window closes. `serve` works without the app too: it prints one JSON line,
`{"url": "http://127.0.0.1:<port>", "token": "..."}`, and serves the project's runs on that port until Ctrl-C or
SIGTERM; besides reading them, it answers their agents and starts, resumes and stops runs, each in the background on
this machine. Its endpoints and security checks are in [serve](docs/architecture.md#serve).

### Tab completion

`octoflow` completes its own command line: subcommands, flags, `--workflow` names (yours included) and, after
`resume`, `show`, `close`, `forget` and `refresh`, the runs of the project, newest first, by key (zsh and fish show
each run's phase and task). The shell runs `octoflow` on each Tab, so new runs and workflow files show up at once.
Runs are read from the project of `--cwd`, else the current directory, and only on this computer: with `--machine`,
no runs are offered. bash and zsh read the `--cwd` as typed, so a quoted or escaped path, or one with a variable,
gives no runs there; fish reads it only before the cursor. The `.deb` sets completion up for all three shells (bash's
through `bash-completion`); otherwise add the line for your shell to its startup file, with `octoflow` on your `PATH`:

```bash
source <(OCTOFLOW_COMPLETE=bash octoflow)                          # ~/.bashrc
source <(OCTOFLOW_COMPLETE=zsh octoflow)                           # ~/.zshrc
OCTOFLOW_COMPLETE=fish octoflow | source                           # ~/.config/fish/config.fish
```

The line asks the binary for its script at each shell start, so the script always matches the binary. The
AppImage is not supported: each Tab would mount it, or without FUSE extract its 340 MB again.

## Usage

```bash
# Check what a run needs, and print a fix for each missing piece; exit status 1 if any is missing
octoflow doctor --machine <machine> --cwd ~/GitHub/myproject

# The same, then write what --quality-gate local reads into the project, where it is missing; commit it yourself
octoflow doctor --cwd ~/GitHub/myproject --setup-gate

# The same, and hold the project at one octoflow-gate release, however octoflow is updated
octoflow doctor --cwd ~/GitHub/myproject --setup-gate --pin-gate 0.15.2

# From a herdr pane, on the project in the current directory
octoflow run "add a token-bucket rate limiter to the API client"

# Agents on a machine saved in herdr, driven from anywhere; --cwd is a path on that machine
octoflow run "add a token-bucket rate limiter" --machine <machine> --cwd ~/GitHub/myproject

# In a git worktree of the run's own, so the checkout stays yours and several runs can work at once
octoflow run --worktree "add a token-bucket rate limiter"

# Skip the interview: a spec you already have is the contract
octoflow run --spec docs/specs/rate-limiter.md

# Another workflow: a built-in or one of yours by name, or a workflow file anywhere; `workflows` lists them
octoflow run --workflow product-team "add a token-bucket rate limiter"
octoflow run --workflow quick "fix the off-by-one in the pager"
octoflow run --workflow-file ~/flows/pairing.toml "fix the off-by-one in the pager"

# With the quality gate after each Builder turn
octoflow run --quality-gate local "add a token-bucket rate limiter"

# Run every ready spec under .scratch/<slug>/spec.md, one pull request each; see Draining specs
octoflow drain --worktree

# A project's runs and whether each is still running; --json for scripts, --pr-state adds each PR's state
octoflow list --machine <machine> --cwd ~/GitHub/myproject

# One run in full, by its run id or the six-character key at its end
octoflow show e292fb

# Continue a stopped run
octoflow resume e292fb

# Done with a failed or finished run: close its workspace, delete its local refs, and a failed --worktree run's
# worktree and branch
octoflow close e292fb

# What closing every finished, failed and stale run would do, uncommitted files included; drop --dry-run to do it
octoflow close --all --dry-run

# Delete every closed run's directory, so that list no longer shows it; metrics still prints its line. A run whose
# branch prune has yet to delete stays until it has
octoflow forget --all

# After you resolved a pull request's conflict and pushed: drop its warning, and its draft if it was approved
octoflow refresh e292fb

# Delete the branches of runs whose pull request is merged, or closed 14 days ago, and a leftover clean worktree
octoflow prune --dry-run

# Every finished run's metrics.json, forgotten runs' included, one JSON line each: outcome, roles, turns with times
# and tokens
octoflow metrics | jq -s 'map(.rounds) | add / length'
```

`--machine` and `--cwd` work the same way for every command, and `doctor` checks a project only when given
`--cwd`.

| Flag | Description |
|---|---|
| `--machine NAME` | Saved herdr machine to run the agents on. Requires `--cwd`. |
| `--cwd PATH` | Project directory, on the machine if `--machine` is given. Default: current directory. |
| `--max-rounds N` | Review rounds before giving up (default 3). |
| `--timeout SECONDS` | How long one Builder or Reviewer turn may take (default 1800). The interview has no limit. |
| `--agent KIND` | The [harness](#harnesses) every role runs in. Overrides a role's `agent` in the workflow file. |
| `--role-agent ROLE=KIND` | The harness of one role, by its key, e.g. `review=codex`. Overrides `--agent`; repeatable. |
| `--permission-mode MODE` | Permission mode for every role, by Claude Code's names; each other harness gets its [equivalent](#harnesses). Which to choose for a run nobody watches: [unattended runs](#unattended-runs). |
| `--model MODEL` | Model for every role, passed to its harness unchanged. Default: the workflow file's model for the role, else the harness's own. |
| `--spec-model`, `--build-model`, `--review-model MODEL` | Model for the Spec Collector, Builder or Reviewer. Overrides `--model`. |
| `--role-model ROLE=MODEL` | Model for one role, by its key, e.g. `tests=sonnet`. Overrides `--model`; repeatable. |
| `--workflow NAME` | `run` and `drain`: `default`, `design`, `peer-review`, `quick`, `product-team`, or one of [your own](#your-own-workflows). A resumed run keeps its workflow. |
| `--workflow-file FILE` | `run` and `drain`: like `--workflow`, for the workflow file at `FILE`. |
| `--spec FILE` | `run` only: skip the interview. `FILE` is read on this machine, a relative path from the current directory, not `--cwd`. It becomes the first step's handoff file, and the run starts at the step after it. Without a task, the task is the spec's first line, when that is a level-one heading, or else `FILE`'s name. Needs a workflow whose first step writes the spec, such as `default` or `product-team`. Later steps that wait for you, such as `design`'s direction and acceptance, still do. |
| `--no-pr` | `run` only: leave the change uncommitted and the workspace open instead of opening a pull request. Works outside git. |
| `--worktree` | `run` and `drain`: work in a git worktree of the run's own; see [Worktree runs](#worktree-runs). |
| `--quality-gate local` | `run`, `drain` and `resume`: after each Builder turn, have the quality gate judge the change: the octoflow-gate release the run's base pins in `ci/gate.pin`, cached in `~/.cache/octoflow/gate/`, run on a checkout of the change, with the project's tests run as you and with no sandbox. `local` is the flag's only value; the Jenkins job names it took before 0.72.0 are refused. |
| `--no-quality-gate` | `resume` only: turn the run's quality gate off for good. |
| `--max-quality-rounds N` | With the gate: quality rounds per review round before the Reviewer gets the change anyway (default 3). |
| `--force` | `resume` only: take over a run that still looks alive. |
| `--dry-run` | `prune`, `drain`, `close` and `forget`: print what would happen, and change nothing. |

A run saves its settings. `resume` takes the same flags as `run` except `--no-pr`, `--worktree`, `--workflow`,
`--workflow-file` and `--spec`; a flag given to `resume` overrides the saved value. Turning the quality gate on or
off on `resume` has rules of its own; see
[quality gate](docs/quality-gate.md#turning-the-gate-on-or-off-on-resume).

### Harnesses

A role's harness is herdr's `--kind` for its agent. The precedence is `--role-agent`, then `--agent`, then the
role's `agent` in the workflow file, then `claude`. A model name is passed as it is, never translated, so give
each role a model its harness knows. opencode, whose interactive interface takes no `--model`, gets it as `model` in
an `OPENCODE_CONFIG_CONTENT` its pane's shell exports before starting it with `--standalone`, so that shell must take
`export NAME='value'`, as sh, bash, zsh and fish do, so `doctor` checks which shell herdr's panes start
([Requirements](#requirements)). pi asks before no tool call, so a worktree run needs nothing for it
(see Worktree runs).

| `--permission-mode` | claude | agy | codex | pi, opencode |
|---|---|---|---|---|
| `default` | `--permission-mode default` | (nothing) | (nothing) | dropped |
| `acceptEdits` | `--permission-mode acceptEdits` | `--mode accept-edits` | `--full-auto` | dropped |
| `bypassPermissions` | `--permission-mode bypassPermissions` | `--dangerously-skip-permissions` | `--dangerously-bypass-approvals-and-sandbox` | dropped |
| `plan` | `--permission-mode plan` | `--mode plan` | `--sandbox read-only` | dropped |
| `auto`, `dontAsk`, any other | passed through | dropped | dropped | dropped |

A dropped mode is not passed, so the agent starts with its own default, and the log says so once per role. An
agent that exited is relaunched into its saved session: `--resume <id>` for claude, `--conversation <id>` for
agy, `--session <id>` for pi and opencode, and `codex resume <id>`. A role whose harness changed since starts a fresh session.

#### Unattended runs

Without `--permission-mode`, each agent starts in its harness's configured default. Claude Code's `default` and
`acceptEdits` both stop for a shell command that is not read-only and not allowed in its settings, so a Builder that
runs the tests, or a Reviewer that checks them, waits until someone answers in its pane; the run says the role
"needs your answer" and goes on once answered.
For a run nobody watches, give claude `--permission-mode auto`, which lets Claude Code's classifier approve each
action, or `bypassPermissions`, which approves everything and belongs on a sandbox or a throwaway machine. The
table above shows what each other harness gets for these modes; pi and opencode get none, so set their own
configuration instead.

### Exit status

| Status | Meaning |
|---|---|
| `0` | Approved, or `FINISHED` for a workflow without a verdict step. |
| `3` | Changes still requested after the last round, or `QUALITY_GATE_FAILED` for a workflow without a verdict step. The pull request is a draft. |
| `4` | Approved or finished, but the pull request conflicts with its base branch, so it is a draft. |
| `5` | `drain`: the specs left wait on blockers that are not `done`, or were deleted or emptied while it ran. `refresh`: a pull request still conflicts, or GitHub has not yet computed whether it does. |
| `1` | Error. |
| `130` | Interrupted. |

`drain` exits with the status of the run that stopped it, `0` once no spec is `ready-for-agent`, or `5`.

## How a run works

1. **Workspace.** The run gets its own herdr workspace, with a pane per role. The project must be on a branch with a
   clean working tree; the pull request targets that branch. It refuses the branch of another run that is not
   closed, such as the one a failed run leaves checked out for `resume`: check out that run's base branch instead.
   The branch is fetched from `origin` and fast-forwarded, and if that fails, for example offline or because it has
   diverged, the run stops before the interview. A fetch that could not reach `origin`, or that `origin` refused to
   let in, is tried three more times, 5, 20 and 60 s apart, each wait shown in the log, before the run stops. A
   `--worktree` run checks `origin`'s version of the branch out in its own worktree instead. A run that will open a
   pull request first deletes the branches of earlier runs whose pull request is done, as `prune` does.
2. **Spec Collector.** A notification tells you it is waiting. Answer its questions in its pane; once you approve
   the spec, it writes `spec.md`. With `--spec`, there is no interview.
3. **Builder.** The base branch is fast-forwarded again, and the branch `orchestrator/<spec title>-<id>` is
   created from it. The Builder implements the spec, verifies it, and writes `build-N.md` without committing. If
   it needs a decision from you, it writes `BLOCKED: <question>` instead, and the run notifies you and waits for
   your answer in its pane ([When the orchestrator needs you](docs/design.md#when-the-orchestrator-needs-you)).
4. **Quality gate**, with `--quality-gate local`. A snapshot of the working tree is judged where the project is,
   with nothing pushed. If the gate does not pass, the Builder answers its findings, up to `--max-quality-rounds`
   times; then the change goes to the Reviewer either way, who is told whether the gate passed. A snapshot holding a
   secret is never judged. See [quality gate](docs/quality-gate.md).
5. **Reviewer.** A fresh session checks the change against the spec and writes `review-N.md`, which starts with
   `VERDICT: APPROVE` or `VERDICT: CHANGES_REQUESTED`; requested changes go back to the Builder.
6. **Pull request.** The change is committed as one commit, the base branch is merged in if it moved on, and the
   branch is opened as a pull request with `gh`. It is a draft if changes were still requested after the last
   round, or if the merge conflicts; then you resolve the conflict and run `refresh`
   ([Refreshing a conflicted pull request](docs/design.md#refreshing-a-conflicted-pull-request)). The workspace
   is closed, and the branch stays until a later run's sweep or `prune` deletes it
   ([Deleting run branches](docs/design.md#deleting-run-branches)). An approved run that changed nothing git
   tracks, its work only in ignored files, opens no pull request: it checks the base branch out again (a `--worktree`
   run keeps its worktree, detached) and deletes its branch, and its summary says there was nothing to publish.
7. **Summary.** Whenever the run ends, it writes `summary.md`: the outcome, the pull request, each role's harness
   and model, and a table of the turns with their durations, which the pull request gets too. A finished run also
   writes `metrics.json`, which `metrics` prints
   ([Turn timings and the summary](docs/design.md#turn-timings-and-the-summary)).

Everything a run writes stays in `<project>/.octoflow-ai/runs/<run-id>/`, which git ignores, until `forget` deletes
it. Runs in one checkout share its working tree, so a run refuses to start or resume while another run there is
`running`; use `--worktree`, or a clone per run, to run in parallel. An engine from before 0.89.0 kept this
directory as `.orchestrator/`; the first command, or `serve`, that finds one moves it to `.octoflow-ai/`, its
worktrees with it, once every run there is closed. Until then it refuses, moving nothing: finish and `close` those
runs with the older engine first.

A failed run keeps its workspace open and stays on its branch, and so does a `--no-pr` run. `resume` continues a
stopped run where `state.json` says ([Resuming a run](docs/design.md#resuming-a-run)), and `close` cleans up after
one you are done with, the local refs that keep its locked files included, or with `--all` after every run that is
not running ([Closing a run](docs/design.md#closing-a-run)). `forget` then deletes a closed run's directory, keeping
its line in `metrics`. `list` marks a run `stale` after five minutes without a heartbeat
([Stale runs](docs/design.md#stale-runs)).

See also [design decisions](docs/design.md), [architecture](docs/architecture.md), [roadmap](docs/roadmap.md)
and [quality gate](docs/quality-gate.md).

### Worktree runs

A `--worktree` run works in `.octoflow-ai/worktrees/<key>`, checked out from `origin`'s version of the project's
branch, so the project's checkout need not be clean and stays yours to use. The worktree holds what git tracks.
Untracked and ignored files, such as a `.venv`, are not in it, though Claude Code still reads the project's
`CLAUDE.md` from the parent directory. The handoff files stay in the project's run directory. agy, claude and
codex are given access to it by a flag, and opencode by a `permission.external_directory` rule in the same
`OPENCODE_CONFIG_CONTENT` that carries its model; pi asks before no write, so it needs nothing.

The worktree is removed once the pull request is open. A failed or `--no-pr` run keeps its worktree, whose path
`show` prints. `close` removes a failed run's worktree, discarding its changes, but keeps a finished `--no-pr`
run's, which holds the change. A `--worktree` run whose worktree is gone cannot be resumed, unless its pull
request is already open.

If removing the worktree failed because it had changes, it stays, and so does its branch, which it keeps checked out.
Once the pull request is merged, or closed for 14 days, `prune` removes such a worktree under
`.octoflow-ai/worktrees/` when it has no uncommitted changes, still has the run's branch checked out, and is on the
pull request's head commit or behind it, and then deletes the branch. Files git ignores, such as a hand-made `.env`,
go with the worktree, as with `close`. Otherwise `prune` keeps both and says why. It also forgets a worktree whose
directory you deleted by hand (`git worktree prune`), which would otherwise keep its branch.

## Draining specs

`drain [DIR]` works through a directory of specs, `.scratch/` by default. Each `DIR/<slug>/spec.md` is a
candidate, and drain runs the ready ones one after another, in slug order, each as `run --spec` would, each
ending in its own pull request. You merge the pull requests and run `drain` again until every spec is done.

Drain reads and writes only the spec's header, the lines before its first `##` heading:

```markdown
# Add a token-bucket rate limiter

Status: ready-for-agent
Blocked by: retry-policy, http-client/spec.md
Run: 20261024-231412-d64cd0
PR: https://github.com/o/r/pull/7
```

- `Status:` moves from `ready-for-agent` to `in-review` while the run's pull request is open, and to `done` once it
  has merged. A run that does not approve, conflicts or fails leaves its spec `ready-for-human`. Any other status
  is left alone.
- `Blocked by:` lists the specs that must be `done` first, by slug or by a path ending in `<slug>/spec.md`. A
  missing blocker, or blockers in a cycle, stop drain before it runs anything.
- `Run:` and `PR:` are written by drain.

Drain stops rather than polls: a spec whose blocker's pull request is still open waits for the next drain. If
the specs directory is inside the checkout, ignore it in git, or use `--worktree`, since the header edits would
otherwise dirty the tree. See [Draining specs](docs/design.md#draining-specs) for what a drain does in order and
how it treats specs you edit while it runs.

## Your own workflows

The roles, their prompts and their order are a workflow. Five are built in: `default` (Spec Collector → Builder ⇄
Reviewer), `design` (for visual work such as a page, a logo or a mockup: Creative Director → Designer's concepts →
your pick of a direction → Designer ⇄ Design Critic, who renders the work, then your verdict; you are asked at the
brief, the direction and the acceptance), `peer-review` (for research written up and refereed: Editor → Author →
Methods Referee, who reproduces every number → Domain Referee, who checks the claims and sources → the Editor's
decision to accept or send it back for revision), `quick` (Builder ⇄ Reviewer, the task as the contract) and `product-team` (Product Manager → Architect →
QA Engineer → Engineer ⇄ Tech Lead, then the Product Manager's acceptance: requirements in business terms, a design
against the code, acceptance tests written and locked before the code, and the Product Manager judging the change
the Tech Lead approved against the requirements, or sending it back to the Engineer). Add your own as a TOML file in
`~/.config/octoflow-ai/workflows/`, named after the workflow, or anywhere with `--workflow-file`. `octoflow
workflows` lists them and says why one does not load, and `workflows NAME` prints a workflow's full definition, a
built-in's to start a variant from. A file cannot take a built-in's name: one made before that workflow was built in,
such as a `quick.toml` copied from an earlier release, no longer loads; delete it to run the built-in, or rename it
to keep yours.

```toml
# An abridged product-team.toml: the Architect's design before the default Builder and Reviewer.
description = "Product Manager -> Architect -> Engineer <-> Tech Lead"

[roles]                    # optional: labels, default models and harnesses
spec = "Product Manager"
design = { label = "Architect", model = "sonnet" }
review = { label = "Tech Lead", agent = "codex" }

[[steps]]
use = "spec"               # a step of the default workflow, as it is

[[steps]]
id = "design"              # also the role, unless role = "..." says otherwise
file = "design.md"         # the handoff file that ends the turn; {n} in it makes one per round
prompt = """
You are the Architect. Design how to meet the requirements in {spec_path} in the code in {cwd}; change no file.
As your last step, write the design to {design_path} in a single write."""

[[steps]]
use = "build"              # the default Builder step, with only its first prompt replaced
edits = true               # as it already is: the run's base and branch are fixed before the first editing step
prompt = "... the design in {design_path} ... write a report to {build_path} in a single write."

[[steps]]
use = "review"
```

| Step key | Meaning |
|---|---|
| `id`, `role` | The step's id, and the role that takes it, the id by default. |
| `file` | The handoff file in the run directory that ends the turn; `{n}` in it makes one per round. |
| `prompt`, `again`, `fresh_note` | The first prompt, the prompt in a later round, and the note for a fresh session in a later round. |
| `use` | `"spec"`, `"build"` or `"review"`: start from that default step and override only the keys given. |
| `loop_to` | Makes this a verdict step, which sends `CHANGES_REQUESTED` back to the earlier step it names. A workflow may have several: each round runs them in order and ends at the first that requests changes, but for the last round, which goes on to a later one that waits on you. Each must loop back to a step at or before the first verdict step, through at least one step that edits, and only verdict steps may come between the first and the last, so that every verdict step judges the change the round ends with. |
| `edits`, `human_paced`, `quality_gated` | The step edits the code, waits on you, or is the one the quality gate follows. |
| `locks` | The first editing step locks the files it changes, as `product-team`'s QA Engineer does; another step's later changes to them are undone. |
| `fresh_repeats_again` | A fresh session in a later round gets `again` too. |

A workflow without a verdict step ends `FINISHED`, and its pull request says no agent reviewed the change. One with
several approves when the last of them approves the round it ends with, and `--max-rounds` bounds the rounds of all of
them together, except that a step that waits on you is always asked: in the last round it gets the change even when an
earlier verdict step requested changes, your approval approves the run, and your request for changes takes one round
more. A workflow's `proof_command`, such as `"python -m pytest -q {proofs}"`, runs the tests the first verdict step
wrote to back its findings. Prompts fill in `{task}`, `{cwd}`, `{n}`, `{change}`, `{path}`, `{proofs_dir}`, `{findings}`
(from round 2 on, the verdict files whose requests for changes started the round, which the default Builder's `again`
names: more than one when a step that waits on you was asked after another's request in the last round), and for every
step `X`, `{X_path}`, `{prev_X_path}` and `{earlier_X_paths}`; write a literal brace as `{{` or `}}`. A placeholder
is its name alone, with no conversion, format spec, attribute or index. The file is checked when it loads, and a run
saves its workflow, so editing the file does not change a run already started.
The desktop app's workflow editor writes these same files, in canonical form: every step written out, with no
comments and no `use` lines, a `use` step's text taken from the built-in prompts, not your prompt files. It keeps
`proof_command` and the diff budget `max_files` and `max_lines` ([quality gate](docs/quality-gate.md)) as the file
set them, but does not edit them, and leaves `proof_command` out while no step loops back. It asks before it rewrites
a file written by hand.
[Workflows](docs/architecture.md#workflows) has the full rules.

## Prompt files

To change only some of the texts the agents get, put `NAME.md` in `~/.config/octoflow-ai/prompts/`,
where `NAME` is the text's name in the engine's `engine/src/prompts.rs`. Its content, less its trailing newline, is sent in its
place, with exactly the placeholders of the text it replaces.

| File | Sent |
|---|---|
| `SPEC_PROMPT.md`, `BUILD_PROMPT.md`, `REVIEW_PROMPT.md` | the default workflow's first prompt to each role |
| `FIX_PROMPT.md`, `RECHECK_PROMPT.md` | the Builder's and the Reviewer's prompt in a later round |
| `REBUILD_NOTE.md`, `REREVIEW_NOTE.md` | to a fresh Builder or Reviewer session in a later round |
| `QUALITY_FIX_PROMPT.md` | to the quality-gated step when the gate did not pass |
| `QUALITY_FOLLOWS_NOTE.md` | at the end of every prompt of the quality-gated step in a run with the gate, saying the gate follows and giving the turn's `--timeout` |
| `QUALITY_PASSED_NOTE.md`, `QUALITY_UNRESOLVED_NOTE.md`, `QUALITY_STUCK_NOTE.md`, `QUALITY_OFF_NOTE.md` | at the end of the verdict step's prompt, on how the quality gate ended |
| `LOCKED_NOTE.md`, `PROOFS_NOTE.md`, `CARRIED_PROOFS_NOTE.md` | on restored locked files, and on the Reviewer's failing proofs |
| `BLOCKED_NOTE.md`, `BLOCKED_PROMPT.md` | at the end of every prompt of a step that can block, and once its report asked the human |
| `CONTINUE_PROMPT.md`, `RETRY_VERDICT_PROMPT.md` | to a session restarted mid-turn, and after a review without a verdict line |

`workflows` lists the prompt files in effect. A bad file stops `run` or `drain` before any agent starts; see
[Prompt files](docs/design.md#prompt-files).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your
option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this
project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms
or conditions.

The binaries and the desktop app contain Rust crates under their own licenses, whose texts each release has as
`octoflow-third-party-licenses.html`; the desktop app has the same file among its resources as
`THIRD-PARTY-LICENSES.html`. `packaging/build-binary.sh` writes it with
[cargo-about](https://github.com/EmbarkStudios/cargo-about), by `packaging/about.toml`, which lists the licenses a
dependency may be under: a dependency under any other fails the build. The desktop app's window also contains npm
packages, whose license texts each release has as `octoflow-desktop-npm-licenses.html` and the app among its
resources as `THIRD-PARTY-LICENSES-NPM.html`; `packaging/build-desktop.sh` writes it from `ui/package-lock.json`,
under the same list.
