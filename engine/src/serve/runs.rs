//! What serve gives of a project's runs, as orchestrator.py's ProjectRuns, plain_name, RunWatch and sse_event: the
//! run records `list --json` prints, each run's files and their text, and the events that tell a stream's client what
//! changed; and each run's agents, read and typed into through herdr, in the herdr server their run saved. Every path
//! a client names is checked before anything is read, and an agent is named only by its run and role, never by its
//! herdr name.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::cli::listing::run_record;
use crate::constants::RUNS_DIR;
use crate::error::OrchestratorError;
use crate::herdr::{AgentAddress, AgentConsole, HerdrError, HerdrServer, Launcher};
use crate::host::Host;
use crate::paths::file_name;
use serde::Deserialize;
use serde_json::Value;
use crate::json::{object, Object, ValueExt};
use crate::interrupt::Interrupt;
use crate::runs::{local_host, pid_alive};
use super::start::{RunRequest, StartWait, Started};
use super::Reached;

/// The keys of a run record that grow with every read of it while nothing happens; a change in them alone is no
/// event. The client ages heartbeat_age itself from when it got the record.
pub const AGING_RECORD_KEYS: [&str; 2] = ["heartbeat_age", "health_reason"];

/// Why serve did not give what a client asked of a run: Python's RunPathRefused, and the OrchestratorError it is a
/// kind of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunReadError {
    /// A run id or file name that is not one plain name in the run directory (plain_name), or that names
    /// state.json; nothing was read. serve answers it with 403.
    Refused(String),
    /// There is no such run, or no agent of the role in it, or the agent has exited. serve answers it with 404.
    Missing(String),
    /// The project could not be read, or herdr failed.
    Failed(OrchestratorError),
}

impl fmt::Display for RunReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunReadError::Refused(why) | RunReadError::Missing(why) => f.write_str(why),
            RunReadError::Failed(e) => e.fmt(f),
        }
    }
}

impl From<OrchestratorError> for RunReadError {
    fn from(e: OrchestratorError) -> RunReadError {
        RunReadError::Failed(e)
    }
}

/// name, if it names an entry of a directory itself: not empty, `.` or `..`, and without a slash or NUL. Errors:
/// RunReadError::Refused otherwise, naming it quoted as Rust's Debug quotes a str.
pub fn plain_name(name: &str) -> Result<&str, RunReadError> {
    if matches!(name, "" | "." | "..") || name.contains(['/', '\0']) {
        return Err(RunReadError::Refused(format!("{name:?} is not a name in the run directory")));
    }
    Ok(name)
}

/// What a client types into a run's agent (ProjectRuns::type_into).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentInput {
    /// Text to submit, as a prompt is.
    Text(String),
    /// Keys to press, in order, as herdr names them (AgentConsole::send_keys).
    Keys(Vec<String>),
}

/// One project's runs, their files and their agents, as serve's endpoints give them.
pub struct ProjectRuns {
    /// Reaches the project; its commands are stopped when the server stops.
    host: Arc<dyn Host + Send + Sync>,
    /// Reaches the project's runs' agents, each in the herdr server its run saved.
    console: Arc<dyn AgentConsole>,
    /// Starts a run's process where the human can watch it (Reached::launcher).
    launcher: Arc<dyn Launcher>,
    /// Where a run's process starts (Reached::launch_dir).
    launch_dir: String,
    /// The project's absolute directory on host.
    cwd: String,
    /// The --machine and --cwd flags as the client gave them (target_args), for the records' resume commands.
    target: Vec<String>,
    /// Whether a process of an id runs on this machine, as run_health asks it: crate::runs::pid_alive, but in tests.
    pid_alive: Arc<dyn Fn(i64) -> bool + Send + Sync>,
}

impl ProjectRuns {
    /// The project reached; target is as the field says.
    pub fn new(reached: Reached, target: Vec<String>) -> ProjectRuns {
        let Reached { host, dir, console, launcher, launch_dir } = reached;
        ProjectRuns { host, console, launcher, launch_dir, cwd: dir, target, pid_alive: Arc::new(pid_alive) }
    }

    /// As new, a run's owner on this machine taken to be alive by pid_alive instead.
    #[cfg(test)]
    pub(crate) fn with_pid_alive(self, pid_alive: impl Fn(i64) -> bool + Send + Sync + 'static) -> ProjectRuns {
        ProjectRuns { pid_alive: Arc::new(pid_alive), ..self }
    }

    /// Whether the project is on another machine, read over SSH.
    pub fn remote(&self) -> bool {
        self.host.ssh_target().is_some()
    }

    /// Each run's record, as `list --json` prints them (run_record, this machine being the one serve runs on).
    /// Errors: the runs cannot be read.
    pub fn records(&self) -> Result<Vec<Object>, OrchestratorError> {
        let here = local_host();
        Ok(self
            .host
            .run_states(&self.cwd)?
            .iter()
            .map(|(age, s)| run_record(s, *age, &here, &*self.pid_alive, &self.target))
            .collect())
    }

    /// The run's files, oldest first, as a JSON list of {"name", "mtime"}, mtime in seconds since the epoch; [] for a
    /// run without a directory. Errors: Refused for a run id that is not a plain name, Failed when the directory
    /// cannot be listed.
    pub fn files(&self, run_id: &str) -> Result<Value, RunReadError> {
        let files = self.host.run_files(&self.dir(run_id)?)?;
        let entry = |path: &str, mtime: i64| {
            let name = file_name(path);
            Value::Object(object([("name", Value::from(name)), ("mtime", Value::from(mtime))]))
        };
        Ok(Value::Array(files.iter().map(|f| entry(&f.path, f.mtime)).collect()))
    }

    /// The text of the run's file name; None when there is no such file. A symbolic link in the run directory is
    /// followed, as Python's Host.read follows it. Errors: Refused, having read nothing, for state.json or a run id or
    /// name that is not a plain name (the run id checked first); Failed when the file cannot be read.
    pub fn read(&self, run_id: &str, name: &str) -> Result<Option<String>, RunReadError> {
        if name == "state.json" {
            return Err(RunReadError::Refused("state.json is not a handoff file".to_string()));
        }
        let dir = self.dir(run_id)?;
        Ok(self.host.read(&format!("{dir}/{}", plain_name(name)?))?)
    }

    /// The last lines of what the run's agent of role shows, at most lines of them (AgentConsole::read). Errors:
    /// agent_name's, Missing for an agent that has exited, Failed when herdr fails otherwise.
    pub fn agent_text(&self, run_id: &str, role: &str, lines: u32) -> Result<String, RunReadError> {
        let agent = self.agent(run_id, role)?;
        self.console.read(&agent, lines).map_err(|e| herdr_failure(role, e))
    }

    /// Type input into the run's agent of role, without waiting for it to be taken. Errors: as agent_text's.
    pub fn type_into(&self, run_id: &str, role: &str, input: &AgentInput) -> Result<(), RunReadError> {
        let agent = self.agent(run_id, role)?;
        let typed = match input {
            AgentInput::Text(text) => self.console.submit(&agent, text),
            AgentInput::Keys(keys) => {
                let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
                self.console.send_keys(&agent, &keys)
            }
        };
        typed.map_err(|e| herdr_failure(role, e))
    }

    /// The run's agent of role as its state.json saves it: its herdr name and its run's herdr server. Errors:
    /// Refused for a run id that is not a plain name; Missing for no such run, or no started agent of that role in
    /// it; Failed when state.json cannot be read, is not JSON, or saves a herdr_server of another shape.
    fn agent(&self, run_id: &str, role: &str) -> Result<AgentAddress, RunReadError> {
        let path = format!("{}/state.json", self.dir(run_id)?);
        let text = self.host.read(&path)?.ok_or_else(|| RunReadError::Missing(format!("no run {run_id}")))?;
        let state = serde_json::from_str::<Value>(&text).map_err(|e| OrchestratorError(format!("{path}: {e}")))?;
        let name = state.get("agents").and_then(|a| a.get(role)).and_then(|a| a.get("name")).and_then(Value::as_str);
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            return Err(RunReadError::Missing(format!("run {run_id} has no {role} agent")));
        };
        let server = match state.get("herdr_server").filter(|s| !s.is_null()) {
            None => None,
            Some(saved) => Some(HerdrServer::deserialize(saved).map_err(|e| {
                OrchestratorError(format!("{path}: herdr_server is not an object of herdr's variables: {e}"))
            })?),
        };
        Ok(AgentAddress { name: name.to_string(), server })
    }

    /// Start the run request asks for, as run_id, by program, the orchestrator's binary, in a new herdr workspace
    /// (Launcher::launch, labelled as the run labels its own, crate::workflow::workspace_label), and wait
    /// (StartWait::until) for it to save its state.json or for its command to exit without one. The project is given to
    /// `run` as `--cwd` and the directory serve resolved, for a project here, and as the client named it, with its
    /// `--machine`, for one on a machine. Once the run is launched a look that fails, at its state.json or its pane, is
    /// no answer: the wait goes on, so that the client still gets the id of a run that is running. Errors: Refused for
    /// a run id that is not a plain name; Failed when herdr cannot launch it, as "no herdr server here; start herdr"
    /// without a herdr server.
    pub fn start(
        &self,
        request: &RunRequest,
        program: &str,
        run_id: &str,
        wait: StartWait,
        stopping: &Interrupt,
    ) -> Result<Started, RunReadError> {
        let state = format!("{}/state.json", self.dir(run_id)?);
        let label = crate::workflow::workspace_label(run_id, &request.task);
        // The pane starts in the resolved directory too, where a relative cwd or one with ~ would resolve elsewhere.
        let target = if self.remote() { self.target.clone() } else { vec!["--cwd".to_string(), self.cwd.clone()] };
        let argv = request.argv(program, run_id, &target);
        let pane = self.launcher.launch(&self.launch_dir, &label, &argv).map_err(|e| match e.code() {
            Some("server_not_running") => OrchestratorError::new("no herdr server here; start herdr"),
            _ => e.into(),
        })?;
        let saved = || self.host.read(&state).is_ok_and(|text| text.is_some());
        Ok(wait.until(stopping, || {
            if saved() {
                return Some(Started::Listed);
            }
            let shown = self.launcher.exited(&pane, START_ERROR_LINES).ok().flatten()?;
            // It may have saved its state.json just before it exited, as a run that fails after its claim does.
            Some(if saved() { Started::Listed } else { Started::Exited(shown) })
        }))
    }

    fn dir(&self, run_id: &str) -> Result<String, RunReadError> {
        Ok(format!("{}/{RUNS_DIR}/{}", self.cwd, plain_name(run_id)?))
    }
}

/// The most lines of its pane that a run which exited before it saved its state.json gives as its error.
const START_ERROR_LINES: u32 = 20;

/// The RunReadError of herdr's failure e on role's agent: Missing when the agent has exited.
fn herdr_failure(role: &str, e: HerdrError) -> RunReadError {
    if e.code() == Some("agent_not_found") {
        return RunReadError::Missing(format!("the {role} agent has exited"));
    }
    RunReadError::Failed(e.into())
}

/// One event of a stream, as RunWatch::poll gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// "record", "files" or "problem", as RunWatch::poll describes them.
    pub kind: &'static str,
    /// What it tells, a JSON object.
    pub payload: Value,
}

impl Event {
    fn new(kind: &'static str, pairs: [(&str, Value); 2]) -> Event {
        Event { kind, payload: Value::Object(object(pairs)) }
    }

    /// The event as one Server-Sent Event of its kind, its payload in its data field as one line of compact JSON.
    pub fn sse(&self) -> String {
        format!("event: {}\ndata: {}\n\n", self.kind, self.payload)
    }
}

/// What one event stream has told its client about a project's runs, and the events that bring it up to date.
pub struct RunWatch<'p> {
    /// The project whose runs the stream tells of.
    project: &'p ProjectRuns,
    /// By run id, what the stream last told of the run.
    sent: BTreeMap<String, Sent>,
    /// The error the last problem event reported; None when there was none, or it cleared.
    problem: Option<String>,
}

/// What an event stream last told of one run.
struct Sent {
    /// The run's record without AGING_RECORD_KEYS.
    key: Object,
    /// The run's files, as last listed.
    files: Value,
    /// True means the run is finished and files is taken to be all it has under its record key, so they are not listed
    /// again until the record changes; always false for a running run. RunWatch::poll says when a list settles.
    files_settled: bool,
}

impl<'p> RunWatch<'p> {
    /// A watch of project that has sent nothing yet.
    pub fn new(project: &'p ProjectRuns) -> RunWatch<'p> {
        RunWatch { project, sent: BTreeMap::new(), problem: None }
    }

    /// The events since the last poll, in order:
    ///
    /// - record, {"run", "record"}: the run's run_record, for a run that appeared or whose record changed other than
    ///   in AGING_RECORD_KEYS; record is null for a run whose state.json is gone, these last, by run id.
    /// - files, {"run", "files"}: the run's files, as ProjectRuns::files lists them, when they changed; after the
    ///   run's record event, if it has one.
    /// - problem, {"error"}: the project could not be read, and why; error is null once it can be again. First.
    ///
    /// The first poll sends every run's record and files. A running run's files are listed on every poll. A finished
    /// or failed run's are listed until two polls in a row with the same record list the same files; then again only
    /// when its record changes, as nothing else drives it. The wait is for the files the engine writes after the save
    /// that finishes the run (workflow.rs: end_done_run's summary.md and metrics.json, release's summary.md): a poll
    /// interval (SERVE_POLL) is taken to be long enough for them, and ones written later are told of only at the
    /// record's next change. A poll that fails sends only its problem, and changes nothing else of what the watch
    /// holds.
    pub fn poll(&mut self) -> Vec<Event> {
        match self.changes() {
            Ok(events) => self.problem_events(None).into_iter().chain(events).collect(),
            Err(e) => self.problem_events(Some(e.to_string())),
        }
    }

    fn changes(&mut self) -> Result<Vec<Event>, RunReadError> {
        let mut events = vec![];
        let mut sent = BTreeMap::new();
        for record in self.project.records()? {
            // A state.json's run_id is a string; one that is not names the run as its JSON.
            let id = record.get("id").cloned().unwrap_or(Value::Null);
            let run_id = id.text();
            let mut key = record.clone();
            for aging in AGING_RECORD_KEYS {
                key.remove(aging);
            }
            let old = self.sent.get(&run_id);
            let same_record = old.is_some_and(|old| old.key == key);
            let finished = record.get("health").is_none_or(Value::is_null);
            let (files, files_settled) = match old {
                Some(old) if same_record && finished && old.files_settled => (old.files.clone(), true),
                _ => {
                    let files = self.project.files(&run_id)?;
                    let settled = finished && same_record && old.is_some_and(|old| old.files == files);
                    (files, settled)
                }
            };
            if !same_record {
                events.push(Event::new("record", [("run", id.clone()), ("record", Value::Object(record))]));
            }
            if old.is_none_or(|old| old.files != files) {
                events.push(Event::new("files", [("run", id), ("files", files.clone())]));
            }
            sent.insert(run_id, Sent { key, files, files_settled });
        }
        for gone in self.sent.keys().filter(|id| !sent.contains_key(*id)) {
            events.push(Event::new("record", [("run", Value::from(gone.as_str())), ("record", Value::Null)]));
        }
        self.sent = sent;
        Ok(events)
    }

    fn problem_events(&mut self, error: Option<String>) -> Vec<Event> {
        if error == self.problem {
            return vec![];
        }
        self.problem.clone_from(&error);
        vec![Event { kind: "problem", payload: Value::Object(object([("error", Value::from(error))])) }]
    }
}

#[cfg(test)]
mod tests;
