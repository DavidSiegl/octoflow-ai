//! herdr, the terminal multiplexer the role agents run in: the [`Herdr`] trait is the commands a run gives it, and
//! [`ProcessHerdr`] gives them by running the `herdr` CLI.
//!
//! The trait is the commands, not the CLI's protocol: a fake plays a role's agent at the level of a prompt and its
//! handoff file (`FakeHerdr`, herdr/fake.rs), and how herdr is
//! invoked and what it answers (its argv, its JSON, its error codes, the limit on its process) is ProcessHerdr's
//! alone. A trait of one primitive, a herdr call that the commands are provided methods on, as Host is built on
//! exec, was weighed and not taken: a fake would then have to read herdr's argv and write its JSON to play an agent,
//! for nothing a run's tests need, and the run would see that primitive too.
//!
//! [`HerdrServer`] says which of this machine's herdr servers a command reaches: a run saves the one its agents are
//! in, so that serve reaches them through [`AgentConsole`] from whatever environment it was started in.

use std::ffi::OsString;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::constants::AGENT_START_TIMEOUT_MS;
use crate::error::{os_error, OrchestratorError};
use crate::host::{last_lines, line_ends_as_lf, run_process, ProcessError};
use crate::interrupt::Interrupt;
use serde_json::Value;
use crate::json::ValueExt;

#[cfg(test)]
pub(crate) mod fake;

/// How long a herdr command that names no time of its own may run, in seconds.
const CALL_LIMIT_SECONDS: u64 = 120;
/// How much longer than the time herdr is given to wait the CLI's process may run, in milliseconds: the limit only
/// guards against a hung CLI.
const WAIT_MARGIN_MS: u64 = 60_000;

/// How long, in milliseconds, a pane's shell has to run a line Herdr::export types into it.
const EXPORT_TIMEOUT_MS: u64 = 10_000;

/// What the line Herdr::export types prints, before its nonce, once the shell has run it.
const EXPORTED: &str = "octoflow: exported";

/// Why a herdr command failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HerdrError {
    /// herdr answered with an error, by herdr's code (such as agent_not_found or timeout; `bad_response` for an
    /// answer that is not one, and `exit_<status>` for a failure herdr gave no code for).
    Herdr { code: String, message: String },
    /// herdr could not be asked, or what it answered is not what was asked for.
    Failed(OrchestratorError),
}

impl HerdrError {
    /// An error with herdr's code and message.
    pub fn new(code: &str, message: impl Into<String>) -> HerdrError {
        HerdrError::Herdr { code: code.to_string(), message: message.into() }
    }

    /// herdr's code for the error; None for a failure herdr gave none for.
    pub fn code(&self) -> Option<&str> {
        match self {
            HerdrError::Herdr { code, .. } => Some(code),
            HerdrError::Failed(_) => None,
        }
    }
}

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HerdrError::Herdr { code, message } => write!(f, "herdr {code}: {message}"),
            HerdrError::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HerdrError {}

impl From<HerdrError> for OrchestratorError {
    fn from(e: HerdrError) -> OrchestratorError {
        match e {
            HerdrError::Failed(e) => e,
            herdr => OrchestratorError(herdr.to_string()),
        }
    }
}

/// The variable that points the herdr CLI at a server's API socket. herdr sets it in every pane it starts, to its own
/// socket, and it wins over HERDR_SESSION (herdr 0.9.1's src/session.rs, active_api_socket_path).
pub const HERDR_SOCKET_PATH: &str = "HERDR_SOCKET_PATH";

/// The variable that picks the named herdr session the CLI reaches, when HERDR_SOCKET_PATH is unset; with neither,
/// it reaches the default session (herdr 0.9.1's src/session.rs, SESSION_ENV_VAR).
pub const HERDR_SESSION: &str = "HERDR_SESSION";

/// Which of this machine's herdr servers the herdr CLI reaches, as the two variables of its environment that pick it,
/// HERDR_SOCKET_PATH and HERDR_SESSION, each as it was set or unset. herdr's own `--session` flag, which wins over
/// both, is never given. state.json holds it as {"HERDR_SOCKET_PATH": ..., "HERDR_SESSION": ...}, each a string, or
/// null for a variable that was unset; one missing from the object is unset.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HerdrServer {
    /// HERDR_SOCKET_PATH's value; None when it is unset.
    #[serde(rename = "HERDR_SOCKET_PATH", default)]
    socket_path: Option<String>,
    /// HERDR_SESSION's value; None when it is unset.
    #[serde(rename = "HERDR_SESSION", default)]
    session: Option<String>,
}

/// The named herdr session a Launcher starts a command in when the server its own environment picks is not running:
/// one herdr server of the user's, started headless by the first such launch and left running, which every later
/// one shares and `herdr session attach octoflow` shows. A fixed name rather than the environment's own session
/// (decided by the user, 2026-11-11), so that what serve started is told apart from the session the human works in.
pub const OWN_SESSION: &str = "octoflow";

/// How long to keep looking for something herdr reports to come about, and how often.
#[derive(Clone, Copy, Debug)]
struct Poll {
    /// From the first look, after which the last look's answer stands.
    limit: Duration,
    /// Between two looks.
    every: Duration,
}

/// ProcessHerdr::server_start outside the tests. herdr 0.9.1's server took about a second to answer.
const SERVER_START: Poll = Poll { limit: Duration::from_secs(15), every: Duration::from_millis(200) };

/// ProcessHerdr::shell_ready outside the tests: a new pane's shell may still be running its startup files.
const SHELL_READY: Poll = Poll { limit: Duration::from_secs(10), every: Duration::from_millis(100) };

/// The shell script ProcessHerdr::start_server starts `herdr server` with, in the background, its standard output
/// and error going to `octoflow/herdr-server.log` under the state directory of the environment herdr runs in, emptied
/// first: XDG_STATE_HOME when it is an absolute path, as the [XDG Base Directory
/// spec](https://specifications.freedesktop.org/basedir-spec/latest/) asks, else `$HOME/.local/state`. It prints that
/// file's path. When the file cannot be made or written, the server is started all the same with its output going to
/// /dev/null, and the script prints `/dev/null` and, on standard error, why. The file is state of the engine's that
/// outlives the start: for the end of the wait to quote, and for the human to read while the server runs. One file,
/// since the only server serve starts is the session OWN_SESSION's.
///
/// The file is tried with `true`, not `:`: a failed redirection on a special built-in such as `:` ends a POSIX shell
/// before its `else` (XCU 2.8.1), as dash, Debian's /bin/sh, does.
const START_SERVER: &str = r#"case $XDG_STATE_HOME in /*) d=$XDG_STATE_HOME ;; *) d=$HOME/.local/state ;; esac
d=$d/octoflow
if mkdir -p -- "$d" && true >"$d/herdr-server.log"; then log=$d/herdr-server.log; else log=/dev/null; fi
herdr server </dev/null >"$log" 2>&1 &
echo "$log""#;

/// Held while a ProcessHerdr looks for a herdr server and starts it (`start_server`), so that of two launches at once
/// in this process, as serve's requests make them, the second finds the server the first started rather than
/// starting another.
static STARTING_SERVER: Mutex<()> = Mutex::new(());

impl HerdrServer {
    /// The server of the session OWN_SESSION, whatever the environment picks.
    pub(crate) fn own_session() -> HerdrServer {
        HerdrServer { socket_path: None, session: Some(OWN_SESSION.to_string()) }
    }

    /// The server a herdr CLI run in the environment vars reaches; None when either variable is set to what is not
    /// UTF-8, which state.json cannot hold. A variable vars hold twice counts by its last value, as a process gets it.
    pub fn of_environment(vars: &[(OsString, OsString)]) -> Option<HerdrServer> {
        let value = |name: &str| match vars.iter().rev().find(|(k, _)| k == name) {
            None => Some(None),
            Some((_, v)) => v.to_str().map(|v| Some(v.to_string())),
        };
        Some(HerdrServer { socket_path: value(HERDR_SOCKET_PATH)?, session: value(HERDR_SESSION)? })
    }

    /// vars with HERDR_SOCKET_PATH and HERDR_SESSION as the server has them, each one it has unset removed: the
    /// environment in which a herdr CLI reaches the server, whatever vars picked.
    fn select(&self, vars: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
        let picks = [(HERDR_SOCKET_PATH, &self.socket_path), (HERDR_SESSION, &self.session)];
        let picked = |k: &OsString| picks.iter().any(|(name, _)| k == name);
        let mut selected: Vec<_> = vars.iter().filter(|(k, _)| !picked(k)).cloned().collect();
        for (name, value) in picks {
            if let Some(value) = value {
                selected.push((OsString::from(name), OsString::from(value)));
            }
        }
        selected
    }
}

/// herdr's record of a running agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    /// Its lifecycle status, such as idle, working, blocked or done.
    pub status: String,
    /// The id of the harness's session it runs in, once herdr knows it; None before, and
    /// when herdr reports an empty one.
    pub session: Option<String>,
}

/// Where a new pane goes beside the one it is split from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Right,
    Down,
}

impl Direction {
    /// The direction as herdr's --direction takes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Right => "right",
            Direction::Down => "down",
        }
    }
}

/// The commands a run gives herdr: workspaces and panes to show the agents in, and the agents themselves. Every
/// error names herdr's code where herdr gave one (HerdrError::code); each method says which codes it answers instead
/// of failing.
pub trait Herdr {
    /// Create a workspace labelled label whose root pane starts in cwd, without focusing it; gives (workspace id,
    /// root pane id).
    fn create_workspace(&self, cwd: &str, label: &str) -> Result<(String, String), HerdrError>;

    /// Split a new pane, starting in cwd, off pane in direction, without focusing it; gives its id.
    fn split(&self, pane: &str, direction: Direction, cwd: &str) -> Result<String, HerdrError>;

    /// Give pane the label shown on it.
    fn rename_pane(&self, pane: &str, label: &str) -> Result<(), HerdrError>;

    /// Have the shell in pane export each (name, value) of vars, so that an agent started there after (start_agent)
    /// inherits them: herdr's `agent start` passes an agent arguments only. They stay set in that shell after the
    /// agent exits. The shell must take POSIX `export NAME='value'`, as sh, bash, zsh and fish do; fish reads `\\`
    /// and `\'` inside the quotes as escapes, so a value with `\\` in it reaches fish changed. Returns once the shell
    /// has run the export. Errors: Failed, without typing anything, for a value with a control character (typable)
    /// or a pane whose shell has not reached its prompt within a few seconds, as when an agent still runs there;
    /// Failed when the shell has not run the line within EXPORT_TIMEOUT_MS; herdr's, such as pane_not_found.
    fn export(&self, pane: &str, vars: &[(String, String)]) -> Result<(), HerdrError>;

    /// Start an agent named name in pane, of the harness kind (herdr's --kind), passing it args. Gives false when it
    /// is blocked on a startup dialog (herdr's agent_not_ready), such as Claude Code's folder-trust question in a
    /// directory it has not seen: herdr keeps the name bound, so the agent can still be waited on.
    fn start_agent(&self, name: &str, pane: &str, kind: &str, args: &[String]) -> Result<bool, HerdrError>;

    /// Send the agent a prompt, without waiting for it to be taken. Errors: Failed, without asking herdr, for text
    /// that starts like a long option, `--` and a letter or digit, which herdr cannot be given (see argument).
    fn prompt(&self, name: &str, text: &str) -> Result<(), HerdrError>;

    /// Wait for the agent to reach one of the statuses until (any change when empty), for at most timeout_ms
    /// milliseconds, or as long as it takes when None; gives the status it reached. Errors: herdr's timeout when the
    /// time runs out first.
    fn wait(&self, name: &str, timeout_ms: Option<u64>, until: &[&str]) -> Result<String, HerdrError>;

    /// herdr's record of the agent; None once it has exited (herdr's agent_not_found).
    fn agent(&self, name: &str) -> Result<Option<Agent>, HerdrError>;

    /// Whether the workspace is still there (herdr's workspace_not_found is false).
    fn workspace_exists(&self, workspace: &str) -> Result<bool, HerdrError>;

    /// Whether the pane is still there (herdr's pane_not_found is false).
    fn pane_exists(&self, pane: &str) -> Result<bool, HerdrError>;

    /// Bring the agent's pane to the front.
    fn focus(&self, name: &str) -> Result<(), HerdrError>;

    /// Close the workspace with its panes.
    fn close_workspace(&self, workspace: &str) -> Result<(), HerdrError>;

    /// Which of this machine's herdr servers the commands reach, without asking herdr; None when they are forwarded
    /// to a herdr machine, or when the server cannot be told (HerdrServer::of_environment).
    fn server(&self) -> Option<HerdrServer>;
}

/// A run's agent, as a console reaches it: its herdr name, and the herdr server it runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAddress {
    /// The agent's herdr name, as its run's state saves it.
    pub name: String,
    /// The server of this machine its run started it in (RunState::herdr_server); None for the one the console's own
    /// environment reaches, and ignored by a console that forwards to a herdr machine.
    pub server: Option<HerdrServer>,
}

/// What a window does with a run's agent, by its address: show what its pane shows, and type into it, as the human
/// would in the pane (serve's /agent endpoints). Every error names herdr's code where herdr gave one, such as
/// agent_not_found for an agent that has exited, or agent_blocked for a prompt to one blocked on a question.
pub trait AgentConsole: Send + Sync {
    /// The last lines of the agent's terminal, at most lines of them, as plain text without its ANSI styling.
    fn read(&self, agent: &AgentAddress, lines: u32) -> Result<String, HerdrError>;

    /// Type text into the agent and submit it, as herdr's `agent prompt` does, without waiting for it to be taken.
    /// Errors: as Herdr::prompt's, so Failed, without asking herdr, for text that starts like a long option.
    fn submit(&self, agent: &AgentAddress, text: &str) -> Result<(), HerdrError>;

    /// Press keys in the agent's terminal, in order, each as herdr names it (`esc`, `enter`, `up`, `ctrl+c`); herdr
    /// checks every key before it sends any, so a key it does not know sends none. Errors: Failed, without asking
    /// herdr, when a key starts like a long option, `--` and a letter or digit, which herdr cannot be given.
    fn send_keys(&self, agent: &AgentAddress, keys: &[&str]) -> Result<(), HerdrError>;
}

/// What serve does to start a command the human can watch and interrupt as one they typed: run it in the shell of a
/// new herdr workspace's pane, where it outlives serve, and tell from the pane when it has exited.
pub trait Launcher: Send + Sync {
    /// Open a workspace labelled label whose pane's shell starts in cwd, without focusing it, and run argv there as
    /// one command, each word as given, whatever a shell or a terminal would make of its characters (quotes, `$`,
    /// newlines, control characters). A word must hold no NUL, which no process argument can. The workspace is in
    /// the herdr server the launcher's environment picks or, when that one is not running, in the session
    /// OWN_SESSION, whose server is started first if it is not running either, and outlives the launcher.
    /// Errors: herdr's; OWN_SESSION's server cannot be started, or does not answer in time. The workspace is closed
    /// again when it opened but the command could not be given to it.
    fn launch(&self, cwd: &str, label: &str, argv: &[String]) -> Result<Launched, HerdrError>;

    /// What launched's pane showed of its command, at most its last lines lines, once it has exited; None while it
    /// runs. Errors: herdr's, such as pane_not_found once the pane has been closed.
    fn exited(&self, launched: &Launched, lines: u32) -> Result<Option<String>, HerdrError>;
}

/// A command Launcher::launch started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launched {
    /// The id of the pane it runs in, as its herdr server names it.
    pub(crate) pane: String,
    /// The herdr server that pane is in; None for the one the launcher's own environment picks.
    pub(crate) server: Option<HerdrServer>,
}

impl Launched {
    /// The named herdr session its pane is in, which `herdr session attach <it>` shows, when the launcher picked the
    /// server by that name, as it does OWN_SESSION's; None for the server the launcher's own environment picks,
    /// whichever that is.
    pub fn herdr_session(&self) -> Option<&str> {
        self.server.as_ref().filter(|s| s.socket_path.is_none()).and_then(|s| s.session.as_deref())
    }
}

/// The line a shell prints in pane once the command Launcher::launch typed there has exited. It names the pane, so
/// that no output of the command can be it unless it knew the pane's id in advance; and launch types it quoted, so
/// that the typed line, which the terminal shows, is not it either.
fn exit_marker(pane: &str) -> String {
    format!("octoflow: the command in pane {pane} has exited")
}

/// The environment variable of the new pane's shell that holds word i of a command Launcher::launch runs.
fn launched_word(i: usize) -> String {
    format!("OCTOFLOW_ARG_{i}")
}

impl Launcher for ProcessHerdr {
    /// herdr 0.9.1's `pane run` types its arguments, joined by spaces, into the pane's shell as keystrokes, so no word
    /// of argv is typed: each is given to the new pane's shell as an environment variable (`workspace create --env`),
    /// and the typed line is only `"$OCTOFLOW_ARG_0" "$OCTOFLOW_ARG_1" ...; echo '<marker>'`, which POSIX shells and
    /// fish read alike. A word typed even quoted would reach the terminal's line editing first: a ^U erases what was
    /// typed before it, and a ^C or ^M ends the line, so a client's text could run as a command of its own.
    fn launch(&self, cwd: &str, label: &str, argv: &[String]) -> Result<Launched, HerdrError> {
        match self.launch_in_server(cwd, label, argv) {
            // A machine's herdr is not this one's to start.
            Err(e) if e.code() == Some("server_not_running") && self.machine.is_none() => {
                let server = HerdrServer::own_session();
                let own = self.in_server(&server);
                own.start_server()?;
                Ok(Launched { pane: own.launch_in_server(cwd, label, argv)?, server: Some(server) })
            }
            launched => Ok(Launched { pane: launched?, server: None }),
        }
    }

    fn exited(&self, launched: &Launched, lines: u32) -> Result<Option<String>, HerdrError> {
        let herdr = self.reaching_server(launched.server.as_ref());
        let pane = launched.pane.as_str();
        // Unwrapped, so that a long line is one line, as the marker must be to be found; a few more lines than asked
        // for, for the marker and the shell's prompt after it.
        let shown = lines.saturating_add(5).to_string();
        let args = ["pane", "read", pane, "--source", "recent-unwrapped", "--lines", &shown];
        let text = herdr.exec_forwarded(&args, Some(Duration::from_secs(CALL_LIMIT_SECONDS)))?;
        let marker = exit_marker(pane);
        let all: Vec<&str> = text.lines().collect();
        let Some(end) = all.iter().rposition(|line| line.trim_end() == marker) else { return Ok(None) };
        let start = end.saturating_sub(lines as usize);
        Ok(Some(all[start..end].join("\n")))
    }
}

impl ProcessHerdr {
    /// Launcher::launch in the server this herdr's environment picks, and no other; gives the pane's id. Errors:
    /// herdr's, such as server_not_running.
    fn launch_in_server(&self, cwd: &str, label: &str, argv: &[String]) -> Result<String, HerdrError> {
        let words: Vec<String> = argv.iter().enumerate().map(|(i, w)| format!("{}={w}", launched_word(i))).collect();
        let mut args = vec!["workspace", "create", "--cwd", cwd, "--label", label, "--no-focus"];
        for word in &words {
            args.extend(["--env", word.as_str()]);
        }
        let created = self.call(&args)?;
        let workspace = field(&created, "workspace create", &["workspace", "workspace_id"])?;
        let pane = field(&created, "workspace create", &["root_pane", "pane_id"])?;
        let typed: Vec<String> = (0..argv.len()).map(|i| format!("\"${}\"", launched_word(i))).collect();
        // A pane id is herdr's, such as w1:p1, with no quote in it.
        let line = format!("{}; echo '{}'", typed.join(" "), exit_marker(&pane));
        // `pane run` prints nothing on success; its errors are JSON as every command's.
        if let Err(e) = self.exec_forwarded(&["pane", "run", &pane, &line], Some(Duration::from_secs(CALL_LIMIT_SECONDS))) {
            // An idle shell nobody asked for; when the close fails too, the error that matters is still the run's.
            let _ = self.close_workspace(&workspace);
            return Err(e);
        }
        Ok(pane)
    }

    /// Have the herdr server this herdr's environment picks running on this machine: started headless, when it is
    /// not, in a process group of its own with no terminal's input or output, so that it outlives the engine as the
    /// server herdr's own client starts does. One start at a time in this process (STARTING_SERVER). The server's
    /// standard output and error go to the file START_SERVER names, emptied at each start, which the server keeps
    /// writing to for as long as it runs. Errors: herdr cannot be asked; the server cannot be started; or it does not
    /// answer as running within server_start's limit, with what herdr last answered instead, when it answered with an
    /// error, and the end of what the server wrote, or that it wrote nothing, that it cannot be read, or why its
    /// output was not kept.
    fn start_server(&self) -> Result<(), HerdrError> {
        // The lock guards no data, so one a panicking start left poisoned is as good as new.
        let _starting = STARTING_SERVER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.server_runs(Duration::from_secs(CALL_LIMIT_SECONDS))? {
            return Ok(());
        }
        let failed = |why: String| HerdrError::Failed(OrchestratorError(format!("cannot start a herdr server: {why}")));
        // `herdr server` serves until it is stopped, so a shell starts it in the background and exits; run_process
        // gave the shell a process group, which the server keeps.
        let argv = ["sh", "-c", START_SERVER].map(String::from);
        // The file the server's output goes to, as sh printed it, and what sh said on standard error, which is why
        // when that file is /dev/null.
        let (log, unkept) = match (self.run)(&argv, &self.env, Some(Duration::from_secs(CALL_LIMIT_SECONDS))) {
            Ok((0, stdout, stderr)) => (
                String::from_utf8_lossy(&stdout).trim().to_string(),
                String::from_utf8_lossy(&stderr).trim().to_string(),
            ),
            Ok((status, _, stderr)) => {
                return Err(failed(format!("sh exited with {status}: {}", String::from_utf8_lossy(&stderr).trim())))
            }
            Err(ProcessError::Start(e) | ProcessError::Io(e)) => return Err(failed(os_error(&e, None))),
            Err(ProcessError::Timeout) => return Err(failed("sh did not return".to_string())),
            Err(ProcessError::Interrupted) => return Err(failed("interrupted".to_string())),
        };
        let Poll { limit, every } = self.server_start;
        let deadline = Instant::now() + limit;
        loop {
            // A look may take as long as the whole wait, the last one too, so that a server up at the wait's end is
            // still found and a herdr that hangs holds the launch for twice the limit at most. An error of herdr's
            // own, a look that ran out of time included, is a server not up yet; herdr not asked at all ends the
            // wait, as an Interrupt raised meanwhile does, within a pause of it.
            // The error herdr answered this look with, if it did.
            let refused = match self.server_runs(limit) {
                Ok(true) => return Ok(()),
                Ok(false) => None,
                Err(e @ HerdrError::Herdr { .. }) => Some(e),
                Err(e @ HerdrError::Failed(_)) => return Err(e),
            };
            if Instant::now() >= deadline {
                let last = refused.map_or_else(String::new, |e| format!("; herdr status server last answered: {e}"));
                let text = std::fs::read(&log).map(|bytes| String::from_utf8_lossy(&bytes).trim_end().to_string());
                let wrote = match text {
                    _ if log == "/dev/null" => format!("herdr server's output was not kept: {unkept}"),
                    Ok(text) if text.is_empty() => format!("herdr server wrote nothing to {log}"),
                    Ok(text) => format!("herdr server wrote to {log}:\n{}", last_lines(&text)),
                    Err(e) => format!("cannot read what herdr server wrote: {}", os_error(&e, Some(&log))),
                };
                return Err(failed(format!("it was not running after {} seconds{last}; {wrote}", limit.as_secs_f64())));
            }
            std::thread::sleep(every);
        }
    }

    /// Whether the herdr server this herdr's environment picks runs on this machine, asked with limit on herdr's
    /// process. `herdr status server` exits 0 either way (herdr 0.9.1), and says it in a line of its output. Errors:
    /// exec's.
    fn server_runs(&self, limit: Duration) -> Result<bool, HerdrError> {
        let out = self.exec(&["status", "server"], Some(limit))?;
        Ok(out.lines().any(|line| line.trim() == "status: running"))
    }

    /// Returns once pane's shell is at its prompt, its own process group in the foreground, looking again within
    /// shell_ready: a new pane's shell may still be running its startup files, and a line typed meanwhile into an
    /// agent running there would be a prompt to it. Errors: Failed when the shell is not at its prompt within
    /// shell_ready's limit; bad_response for an answer without the shell's pid; herdr's, such as pane_not_found.
    fn await_prompt(&self, pane: &str) -> Result<(), HerdrError> {
        let Poll { limit, every } = self.shell_ready;
        let deadline = Instant::now() + limit;
        loop {
            let info = self.call(&["pane", "process-info", "--pane", pane])?;
            let Some(shell) = info.pointer("/process_info/shell_pid").and_then(Value::as_u64) else {
                return Err(HerdrError::new("bad_response", "pane process-info: no process_info.shell_pid in the answer"));
            };
            let foreground = info.pointer("/process_info/foreground_process_group_id").and_then(Value::as_u64);
            if foreground == Some(shell) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let why = format!("cannot type into pane {pane}: its shell is not at its prompt");
                return Err(HerdrError::Failed(OrchestratorError::new(why)));
            }
            std::thread::sleep(every);
        }
    }
}

/// What running the herdr CLI once gives: its exit status and both outputs, as run_process gives them.
type Ran = Result<(i32, Vec<u8>, Vec<u8>), ProcessError>;

/// Runs the herdr CLI: its argv, herdr first, the environment it runs in, and how long its process may run (None: as
/// long as it takes).
type RunHerdr = dyn Fn(&[String], &[(OsString, OsString)], Option<Duration>) -> Ran + Send + Sync;

/// herdr reached by running its CLI here, each command forwarded to a saved SSH machine (herdr's `--machine`) when
/// the run has one. Each command runs in the engine's environment as child_env gives it (crate::env::child_env), so
/// herdr and the agents it starts never inherit the CI credentials; a console's command to an agent whose run saved
/// its server runs with that server's HERDR_SOCKET_PATH and HERDR_SESSION in place of the engine's (AgentAddress).
/// Each fails as interrupted when the Interrupt it was given is raised while it runs, even one that herdr had already
/// carried out, such as a prompt delivered (crate::host::run_process).
#[derive(Clone)]
pub struct ProcessHerdr {
    /// The herdr machine its commands are forwarded to, as `herdr machine list` names it by label or id; None for
    /// this machine's herdr. Never empty.
    machine: Option<String>,
    /// The environment its commands run in, without the variables child_env withholds.
    env: Arc<Vec<(OsString, OsString)>>,
    /// Runs one herdr command.
    run: Arc<RunHerdr>,
    /// The time it gives a herdr server it starts to answer `herdr status server` as running; its limit is also the
    /// longest one look at that may take.
    server_start: Poll,
    /// The time it gives a pane's shell to reach its prompt before export types into it.
    shell_ready: Poll,
}

impl fmt::Debug for ProcessHerdr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessHerdr").field("machine", &self.machine).finish_non_exhaustive()
    }
}

impl ProcessHerdr {
    /// herdr here, its commands forwarded to machine when given one, an empty machine being none, and stopped by
    /// interrupt.
    pub fn new(machine: Option<&str>, interrupt: &Interrupt) -> ProcessHerdr {
        ProcessHerdr::in_environment(machine, std::env::vars_os(), interrupt)
    }

    /// As new, the CLI found on and run in the environment vars as child_env would give it, were vars the engine's
    /// own: what a test gives in place of the process's environment, which every test shares.
    pub(crate) fn in_environment(
        machine: Option<&str>,
        vars: impl Iterator<Item = (OsString, OsString)>,
        interrupt: &Interrupt,
    ) -> ProcessHerdr {
        let interrupt = interrupt.clone();
        ProcessHerdr {
            machine: machine.filter(|m| !m.is_empty()).map(String::from),
            env: Arc::new(crate::env::without_withheld(vars)),
            run: Arc::new(move |argv, env, limit| run_process(argv, None, limit, env, &interrupt)),
            server_start: SERVER_START,
            shell_ready: SHELL_READY,
        }
    }

    /// As new, each command run by run in place of a process, in an empty environment, for the tests.
    #[cfg(test)]
    pub(crate) fn with_runner(
        machine: Option<&str>,
        run: impl Fn(&[String], Option<Duration>) -> Ran + Send + Sync + 'static,
    ) -> ProcessHerdr {
        ProcessHerdr {
            machine: machine.filter(|m| !m.is_empty()).map(String::from),
            env: Arc::default(),
            run: Arc::new(move |argv, _, limit| run(argv, limit)),
            server_start: SERVER_START,
            shell_ready: SHELL_READY,
        }
    }

    /// This herdr, its commands run so that they reach the agent's server when it names one and they are not
    /// forwarded to a machine: the herdr a console's command to the agent goes to.
    fn reaching(&self, agent: &AgentAddress) -> ProcessHerdr {
        self.reaching_server(agent.server.as_ref())
    }

    /// This herdr, its commands run so that they reach server when one is given and they are not forwarded to a
    /// machine, whose herdr picks its own.
    fn reaching_server(&self, server: Option<&HerdrServer>) -> ProcessHerdr {
        match (server, &self.machine) {
            (Some(server), None) => self.in_server(server),
            _ => self.clone(),
        }
    }

    /// This herdr, its commands reaching server whatever server its own environment picks.
    fn in_server(&self, server: &HerdrServer) -> ProcessHerdr {
        ProcessHerdr { env: Arc::new(server.select(&self.env)), ..self.clone() }
    }

    /// The herdr machine the commands are forwarded to; None for this machine's herdr.
    pub fn machine(&self) -> Option<&str> {
        self.machine.as_deref()
    }

    /// The SSH target of the machine, for reaching its filesystem, from this machine's saved machines: machine
    /// management is herdr's here, and is never forwarded. Errors: this herdr has no machine, which herdr is then not
    /// asked about; no saved machine is named so, or it is disabled.
    pub fn ssh_target(&self) -> Result<String, HerdrError> {
        let no_machine = || HerdrError::Failed(OrchestratorError::new("no herdr machine chosen, so none to reach"));
        let machine = self.machine.as_deref().ok_or_else(no_machine)?;
        let out = self.exec(&["machine", "list", "--json"], Some(Duration::from_secs(CALL_LIMIT_SECONDS)))?;
        let bad = |why: String| HerdrError::new("bad_response", format!("machine list: {why}"));
        let profiles = serde_json::from_str::<Value>(&out).map_err(|e| bad(e.to_string()))?;
        let profiles = profiles.as_array().ok_or_else(|| bad("not a list of machines".to_string()))?;
        for p in profiles {
            if p.as_object().is_none() {
                return Err(bad(format!("a machine is not an object: {}", p)));
            }
            if ["label", "id"].iter().any(|key| p.get(key).and_then(Value::as_str) == Some(machine)) {
                if !p.get("enabled").is_some_and(Value::is_set) {
                    return Err(HerdrError::Failed(OrchestratorError(format!("herdr machine {machine} is disabled"))));
                }
                return match p.get("target") {
                    Some(Value::String(target)) => Ok(target.clone()),
                    _ => Err(bad(format!("machine {machine} has no target"))),
                };
            }
        }
        Err(HerdrError::Failed(OrchestratorError(format!("no saved herdr machine named {machine}"))))
    }

    /// Show a desktop notification with title and body, and a sound, from herdr.
    pub fn notify(&self, title: &str, body: &str) -> Result<(), HerdrError> {
        self.call(&["notification", "show", title, "--body", body, "--sound", "request"]).map(drop)
    }

    /// call_within, its process given CALL_LIMIT_SECONDS: for a command that does not wait on an agent.
    fn call(&self, args: &[&str]) -> Result<Value, HerdrError> {
        self.call_within(args, Some(Duration::from_secs(CALL_LIMIT_SECONDS)))
    }

    /// Run one API-backed command, forwarded to the machine, its process given limit as exec takes it, and give its
    /// `result`. Errors: as exec's, and herdr answering with what is not a result (bad_response).
    fn call_within(&self, args: &[&str], limit: Option<Duration>) -> Result<Value, HerdrError> {
        let out = self.exec_forwarded(args, limit)?;
        let command = args[..args.len().min(2)].join(" ");
        let bad = |why: String| HerdrError::new("bad_response", format!("{command}: {why}"));
        let mut answer = serde_json::from_str::<Value>(&out).map_err(|e| bad(e.to_string()))?;
        match &mut answer {
            Value::Object(map) => map.remove("result").ok_or_else(|| bad("\"result\"".to_string())),
            _ => Err(bad(format!("the answer is not an object: {}", out.trim()))),
        }
    }

    /// The standard output of `herdr args`, forwarded to the machine, run with limit as exec takes it. Errors: exec's.
    fn exec_forwarded(&self, args: &[&str], limit: Option<Duration>) -> Result<String, HerdrError> {
        let mut argv: Vec<&str> = vec![];
        if let Some(machine) = &self.machine {
            argv.extend(["--machine", machine]);
        }
        argv.extend(args);
        self.exec(&argv, limit)
    }

    /// The standard output of `herdr args`, run with limit on its process (None: as long as it takes). Errors: herdr
    /// is not on PATH; its process runs out of time (timeout); the Interrupt is raised while it runs ("herdr ...:
    /// interrupted", no code); it exits with a status other than 0, with the code and
    /// message of the error it printed as JSON on stderr or stdout, else exit_<status> and what it printed; or it
    /// prints what is not UTF-8.
    fn exec(&self, args: &[&str], limit: Option<Duration>) -> Result<String, HerdrError> {
        let argv: Vec<String> = std::iter::once("herdr").chain(args.iter().copied()).map(String::from).collect();
        // The command as an error names it: its first three words, never a prompt's text, which can be long and goes
        // to the log and state.json with the error.
        let command = args[..args.len().min(3)].join(" ");
        let failed = |why: String| HerdrError::Failed(OrchestratorError(format!("herdr {command}: {why}")));
        let (status, stdout, stderr) = match (self.run)(&argv, &self.env, limit) {
            Ok(ran) => ran,
            Err(ProcessError::Start(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(HerdrError::Failed(OrchestratorError::new("herdr is not on PATH")))
            }
            Err(ProcessError::Start(e) | ProcessError::Io(e)) => return Err(failed(os_error(&e, None))),
            Err(ProcessError::Timeout) => {
                return Err(HerdrError::new("timeout", format!("herdr {command} did not return")));
            }
            Err(ProcessError::Interrupted) => return Err(failed("interrupted".to_string())),
        };
        let decode = |bytes: Vec<u8>| {
            String::from_utf8(bytes)
                .map(|text| line_ends_as_lf(&text))
                .map_err(|e| failed(format!("printed what is not UTF-8: {}", e.utf8_error())))
        };
        let (stdout, stderr) = (decode(stdout)?, decode(stderr)?);
        if status != 0 {
            return Err(herdr_error(status, &stdout, &stderr));
        }
        Ok(stdout)
    }
}

/// answer's value, or None when herdr answered with the error code: for a command whose one error is an answer.
fn unless_code<T>(code: &str, answer: Result<T, HerdrError>) -> Result<Option<T>, HerdrError> {
    match answer {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.code() == Some(code) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The error a herdr command that exited with status printed: a server error arrives as `{"error": {"code",
/// "message"}}` on stderr or stdout, a syntax error (exit 2) as plain text.
fn herdr_error(status: i32, stdout: &str, stderr: &str) -> HerdrError {
    // A code or message that is no string is written as JSON, so that it is still shown.
    let text = |value: &Value| value.as_str().map_or_else(|| value.to_string(), String::from);
    for stream in [stderr, stdout] {
        let Ok(answer) = serde_json::from_str::<Value>(stream) else { continue };
        let Some(error) = answer.get("error").and_then(Value::as_object) else { continue };
        if let (Some(code), Some(message)) = (error.get("code"), error.get("message")) {
            return HerdrError::new(&text(code), text(message));
        }
    }
    let printed = if stderr.is_empty() { stdout } else { stderr };
    HerdrError::new(&format!("exit_{status}"), printed.trim())
}

/// Ok when each value of vars can be typed into pane's shell (Herdr::export); else Failed, naming the first that
/// cannot: one with a control character, which the shell's line editor would take as a key rather than text.
pub(crate) fn typable(pane: &str, vars: &[(String, String)]) -> Result<(), HerdrError> {
    match vars.iter().find(|(_, value)| value.chars().any(char::is_control)) {
        Some((name, _)) => {
            let why = format!("cannot type {name} into pane {pane}: its value has a control character");
            Err(HerdrError::Failed(OrchestratorError::new(why)))
        }
        None => Ok(()),
    }
}

/// The names of vars, joined for a message.
fn names(vars: &[(String, String)]) -> String {
    vars.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ")
}

/// A string field of an answer, at the path of keys; bad_response naming the command and the path when it is not
/// there or not a string.
fn field(answer: &Value, command: &str, path: &[&str]) -> Result<String, HerdrError> {
    let mut value = answer;
    for key in path {
        value = value.get(key).unwrap_or(&Value::Null);
    }
    value
        .as_str()
        .map(String::from)
        .ok_or_else(|| HerdrError::new("bad_response", format!("{command}: no {} in the answer", path.join("."))))
}

/// arg, a prompt's text or a key, once it is safe to pass herdr's `agent prompt` and `agent send-keys` as a positional
/// argument; what names it in the error ("a prompt", "a key"). herdr 0.9.1 reads those positionals as they are and
/// has no `--` to end options there (it would take a `--` as the text or a key itself), but takes its long launch
/// options (--session, --remote, --remote-keybindings, --handoff) out of all of its argv before any command sees it.
/// So arg must not look like a long option: `--` then a letter or digit, wider than the options herdr 0.9.1 takes,
/// against ones a later herdr adds; `-`, `- item` and `---` stay safe. Errors: Failed for an arg that looks like one.
fn argument<'a>(what: &str, arg: &'a str) -> Result<&'a str, HerdrError> {
    if arg.strip_prefix("--").and_then(|rest| rest.chars().next()).is_some_and(|c| c.is_ascii_alphanumeric()) {
        let why = format!("{what} cannot start with -- and a letter or digit: herdr may take it for its own option");
        return Err(HerdrError::Failed(OrchestratorError::new(why)));
    }
    Ok(arg)
}

impl Herdr for ProcessHerdr {
    fn create_workspace(&self, cwd: &str, label: &str) -> Result<(String, String), HerdrError> {
        let args = ["workspace", "create", "--cwd", cwd, "--label", label, "--no-focus"];
        let r = self.call(&args)?;
        Ok((
            field(&r, "workspace create", &["workspace", "workspace_id"])?,
            field(&r, "workspace create", &["root_pane", "pane_id"])?,
        ))
    }

    fn split(&self, pane: &str, direction: Direction, cwd: &str) -> Result<String, HerdrError> {
        let args = ["pane", "split", pane, "--direction", direction.as_str(), "--cwd", cwd, "--no-focus"];
        field(&self.call(&args)?, "pane split", &["pane", "pane_id"])
    }

    fn rename_pane(&self, pane: &str, label: &str) -> Result<(), HerdrError> {
        self.call(&["pane", "rename", pane, label]).map(drop)
    }

    fn export(&self, pane: &str, vars: &[(String, String)]) -> Result<(), HerdrError> {
        typable(pane, vars)?;
        self.await_prompt(pane)?;
        let quoted = |value: &str| format!("'{}'", value.replace('\'', r"'\''"));
        let words: Vec<String> = vars.iter().map(|(name, value)| format!("{name}={}", quoted(value))).collect();
        // The shell prints the marker once it has run the export, and only then: a line the shell has not read yet,
        // or one it read as something else, is caught here rather than as an agent started without the variables.
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let line = format!("export {} && echo '{EXPORTED}' '{nonce}'", words.join(" "));
        // `pane run` prints nothing on success; its errors are JSON as every command's.
        self.exec_forwarded(&["pane", "run", pane, &line], Some(Duration::from_secs(CALL_LIMIT_SECONDS)))?;
        let printed = format!("(?m)^{EXPORTED} {nonce}$");
        let timeout = EXPORT_TIMEOUT_MS.to_string();
        let args =
            ["pane", "wait-output", pane, "--regex", &printed, "--source", "recent-unwrapped", "--timeout", &timeout];
        match self.call_within(&args, Some(Duration::from_millis(EXPORT_TIMEOUT_MS + WAIT_MARGIN_MS))) {
            Ok(_) => Ok(()),
            Err(e) if e.code() == Some("timeout") => {
                let why = format!("the shell in pane {pane} did not run the export of {} typed into it", names(vars));
                Err(HerdrError::Failed(OrchestratorError::new(why)))
            }
            Err(e) => Err(e),
        }
    }

    fn start_agent(&self, name: &str, pane: &str, kind: &str, args: &[String]) -> Result<bool, HerdrError> {
        let timeout = AGENT_START_TIMEOUT_MS.to_string();
        let mut argv = vec!["agent", "start", name, "--kind", kind, "--pane", pane, "--timeout", &timeout];
        if !args.is_empty() {
            argv.push("--");
            argv.extend(args.iter().map(String::as_str));
        }
        let limit = Duration::from_millis(AGENT_START_TIMEOUT_MS + WAIT_MARGIN_MS);
        Ok(unless_code("agent_not_ready", self.call_within(&argv, Some(limit)))?.is_some())
    }

    fn prompt(&self, name: &str, text: &str) -> Result<(), HerdrError> {
        self.call(&["agent", "prompt", name, argument("a prompt", text)?]).map(drop)
    }

    fn wait(&self, name: &str, timeout_ms: Option<u64>, until: &[&str]) -> Result<String, HerdrError> {
        let timeout = timeout_ms.map(|ms| ms.to_string());
        let mut args = vec!["agent", "wait", name];
        if let Some(timeout) = &timeout {
            args.extend(["--timeout", timeout]);
        }
        for status in until {
            args.extend(["--until", status]);
        }
        let limit = timeout_ms.map(|ms| Duration::from_millis(ms + WAIT_MARGIN_MS));
        field(&self.call_within(&args, limit)?, "agent wait", &["agent", "agent_status"])
    }

    fn agent(&self, name: &str) -> Result<Option<Agent>, HerdrError> {
        let Some(r) = unless_code("agent_not_found", self.call(&["agent", "get", name]))? else { return Ok(None) };
        let status = field(&r, "agent get", &["agent", "agent_status"])?;
        let session = r.get("agent").and_then(|a| a.get("agent_session")).and_then(|s| s.get("value"));
        let session = session.and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        Ok(Some(Agent { status, session }))
    }

    fn workspace_exists(&self, workspace: &str) -> Result<bool, HerdrError> {
        Ok(unless_code("workspace_not_found", self.call(&["workspace", "get", workspace]))?.is_some())
    }

    fn pane_exists(&self, pane: &str) -> Result<bool, HerdrError> {
        Ok(unless_code("pane_not_found", self.call(&["pane", "get", pane]))?.is_some())
    }

    fn focus(&self, name: &str) -> Result<(), HerdrError> {
        self.call(&["agent", "focus", name]).map(drop)
    }

    fn close_workspace(&self, workspace: &str) -> Result<(), HerdrError> {
        self.call(&["workspace", "close", workspace]).map(drop)
    }

    fn server(&self) -> Option<HerdrServer> {
        match self.machine {
            Some(_) => None,
            None => HerdrServer::of_environment(&self.env),
        }
    }
}

impl AgentConsole for ProcessHerdr {
    fn read(&self, agent: &AgentAddress, lines: u32) -> Result<String, HerdrError> {
        let lines = lines.to_string();
        // `agent read` prints the text itself, not a JSON answer; its errors are JSON as every command's.
        let args = ["agent", "read", &agent.name, "--source", "recent", "--lines", &lines];
        self.reaching(agent).exec_forwarded(&args, Some(Duration::from_secs(CALL_LIMIT_SECONDS)))
    }

    fn submit(&self, agent: &AgentAddress, text: &str) -> Result<(), HerdrError> {
        self.reaching(agent).prompt(&agent.name, text)
    }

    fn send_keys(&self, agent: &AgentAddress, keys: &[&str]) -> Result<(), HerdrError> {
        let mut args = vec!["agent", "send-keys", agent.name.as_str()];
        for key in keys {
            args.push(argument("a key", key)?);
        }
        self.reaching(agent).call(&args).map(drop)
    }
}

#[cfg(test)]
mod tests;
