//! What serve gives of a project's runs: the run records `list --json` prints, each run's files and their text, and
//! the events that tell a stream's client what
//! changed; and each run's agents, read and typed into through herdr, in the herdr server their run saved. Every path
//! a client names is checked before anything is read, and an agent is named only by its run and role, never by its
//! herdr name.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use super::start::{RunRequest, StartWait, Started};
use super::{Launch, Reached};
use crate::acp;
use crate::agents::RunPlaces;
use crate::cli::close::{close_all, Closer, RunClosed, Sweep};
use crate::cli::forget::{self, forget_all, Forgetter, RunForgotten};
use crate::cli::listing::run_record;
use crate::cli::START_LOG;
use crate::error::OrchestratorError;
use crate::herdr::{AgentAddress, AgentConsole, HerdrError, Launcher};
use crate::host::spawn_detached;
use crate::host::{Host, RunFileText, RunScope};
use crate::interrupt::Interrupt;
use crate::json::{object, Object, ValueExt};
use crate::local_gate::SetUpFile;
use crate::paths::file_name;
use crate::runs::{local_host, pid_alive};
use crate::serve::FINISHED_RUN_WRITES;
use crate::state::{run_dir, SavedRun};
use serde_json::Value;

/// The keys of a run record that grow with every read of it while nothing happens; a change in them alone is no
/// event. The client ages heartbeat_age itself from when it got the record.
pub const AGING_RECORD_KEYS: [&str; 2] = ["heartbeat_age", "health_reason"];

/// Why serve did not give what a client asked of a run: the variants say which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunReadError {
    /// A run id or file name that is not one plain name in the run directory (plain_name), or that names
    /// state.json; nothing was read. serve answers it with 403.
    Refused(String),
    /// There is no such run, or no agent of the role in it, or the agent has exited. serve answers it with 404.
    Missing(String),
    /// What was asked cannot be had of the run as it is: its journal is from another version of octoflow. serve
    /// answers it with 409.
    Conflict(String),
    /// The project could not be read, or herdr failed.
    Failed(OrchestratorError),
}

impl fmt::Display for RunReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunReadError::Refused(why) | RunReadError::Missing(why) | RunReadError::Conflict(why) => f.write_str(why),
            RunReadError::Failed(e) => e.fmt(f),
        }
    }
}

impl From<OrchestratorError> for RunReadError {
    fn from(e: OrchestratorError) -> RunReadError {
        RunReadError::Failed(e)
    }
}

/// name, if it names an entry of a directory itself (crate::paths::plain_name). Errors: RunReadError::Refused
/// otherwise, with paths::plain_name's message.
pub fn plain_name(name: &str) -> Result<&str, RunReadError> {
    crate::paths::plain_name(name).map_err(|e| RunReadError::Refused(e.0))
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
    /// Tells and closes the project's runs' agents' places, each from what its run saved.
    places: Arc<dyn RunPlaces + Send + Sync>,
    /// How a run's process starts (Reached::launch).
    launch: Launch,
    /// Where a run's process starts (Reached::launch_dir).
    launch_dir: String,
    /// The project's absolute directory on host.
    cwd: String,
    /// The --machine and --cwd flags as the client gave them (target_args), for the records' resume commands.
    target: Vec<String>,
    /// Whether a process of an id runs on this machine, as RunState::health asks it: crate::runs::pid_alive, but in
    /// tests.
    pid_alive: Arc<dyn Fn(i64) -> bool + Send + Sync>,
    /// Held through each forget sweep, so that this serve's sweeps of the project run one after the other: a run that
    /// two of its requests list at once (a double submit) is forgotten by the first, and the second finds it gone.
    /// Another process's sweep (a second app, `octoflow forget`) is not held back (TODO.md).
    forgetting: Mutex<()>,
}

impl ProjectRuns {
    /// The project reached; target is as the field says.
    pub fn new(reached: Reached, target: Vec<String>) -> ProjectRuns {
        let Reached { host, dir, console, places, launch, launch_dir } = reached;
        let pid_alive = Arc::new(pid_alive);
        ProjectRuns {
            host,
            console,
            places,
            launch,
            launch_dir,
            cwd: dir,
            target,
            pid_alive,
            forgetting: Mutex::new(()),
        }
    }

    /// As new, a run's owner on this machine taken to be alive by pid_alive instead.
    #[cfg(test)]
    pub(crate) fn with_pid_alive(self, pid_alive: impl Fn(i64) -> bool + Send + Sync + 'static) -> ProjectRuns {
        ProjectRuns { pid_alive: Arc::new(pid_alive), ..self }
    }

    /// Set the project up for the local quality gate, as crate::local_gate::set_up does without a pin, on the
    /// machine it is on.
    pub fn set_up_gate(&self) -> Result<Vec<SetUpFile>, OrchestratorError> {
        crate::local_gate::set_up(&*self.host, &self.cwd, None)
    }

    /// Close the project's runs that sweep names, as crate::cli::close::close_all does, of those records shows
    /// (RunScope::Inside), and give each run's outcome; the lines close prints are dropped, as the outcomes hold what
    /// they say. A raised stopping starts no further run. Errors: as close_all's.
    pub fn close_all(&self, sweep: Sweep, stopping: &Interrupt) -> Result<Vec<RunClosed>, OrchestratorError> {
        let here = local_host();
        let closer = Closer {
            host: &*self.host,
            places: &*self.places,
            interrupt: stopping,
            pid_alive: &*self.pid_alive,
            local_host: &here,
            stdout: &|_| {},
            scope: RunScope::Inside,
        };
        close_all(&closer, &self.cwd, sweep)
    }

    /// Forget the project's closed runs that sweep names, as crate::cli::forget::forget_all does, of those records
    /// shows (RunScope::Inside), and give each run's outcome; the lines forget prints are dropped, as the outcomes hold
    /// what they say. A sweep waits for one of the project's already under way to end. A raised stopping starts no
    /// further run. Errors: as forget_all's.
    pub fn forget_all(
        &self,
        sweep: forget::Sweep,
        stopping: &Interrupt,
    ) -> Result<Vec<RunForgotten>, OrchestratorError> {
        // A sweep that panicked left nothing half-done that the next could trip on: each run is read at its turn.
        let _one_at_a_time = self.forgetting.lock().unwrap_or_else(PoisonError::into_inner);
        let forgetter = Forgetter { host: &*self.host, interrupt: stopping, stdout: &|_| {}, scope: RunScope::Inside };
        forget_all(&forgetter, &self.cwd, sweep)
    }

    /// The project's absolute directory on its machine, as Host::resolve_dir gives it.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Whether the project is on another machine, read over SSH.
    pub fn remote(&self) -> bool {
        self.host.ssh_target().is_some()
    }

    /// Each run's record, as `list --json` prints them (run_record, this machine being the one serve runs on), of the
    /// runs whose state.json stays inside their run directory, and that inside the runs directory
    /// (Host::run_states_inside): another is no run to serve. Errors: the runs cannot be read.
    pub fn records(&self) -> Result<Vec<Object>, OrchestratorError> {
        let here = local_host();
        Ok(self.saved_runs()?.iter().map(|run| self.record(run, &here)).collect())
    }

    /// The runs records reads, as saved. Errors: as records.
    fn saved_runs(&self) -> Result<Vec<SavedRun>, OrchestratorError> {
        self.host.run_states_inside(&self.cwd)
    }

    /// run's record, as records gives it; here is local_host().
    fn record(&self, run: &SavedRun, here: &str) -> Object {
        run_record(run, here, &*self.pid_alive, &self.target)
    }

    /// The run's files, oldest first, as a JSON list of {"name", "mtime"}, mtime in seconds since the epoch; [] for a
    /// run without a directory. Only the files read would give are listed: none that leads out of the run directory
    /// through a symbolic link, and none of a run whose directory leads out of the runs directory
    /// (Host::run_files_inside). Errors: Refused for a run id that is not a plain name, Failed when the directory
    /// cannot be listed.
    pub fn files(&self, run_id: &str) -> Result<Value, RunReadError> {
        let files = self.host.run_files_inside(&self.dir(run_id)?)?;
        let entry = |path: &str, mtime: i64| {
            let name = file_name(path);
            Value::Object(object([("name", Value::from(name)), ("mtime", Value::from(mtime))]))
        };
        Ok(Value::Array(files.iter().map(|f| entry(&f.path, f.mtime)).collect()))
    }

    /// The text of the run's file name; None when there is no such file. A symbolic link is followed only as far as
    /// it stays inside the run directory, and the run directory only as far as it stays inside the runs directory
    /// (Host::read_inside). Errors: Refused, having read nothing, for state.json, a run id or name that is not a
    /// plain name (the run id checked first), or a file that leads out; Failed when the file cannot be read.
    pub fn read(&self, run_id: &str, name: &str) -> Result<Option<String>, RunReadError> {
        if name == "state.json" {
            return Err(RunReadError::Refused("state.json is not a handoff file".to_string()));
        }
        match self.host.read_inside(&self.dir(run_id)?, plain_name(name)?)? {
            RunFileText::Text(text) => Ok(Some(text)),
            RunFileText::Missing => Ok(None),
            RunFileText::Outside => Err(RunReadError::Refused(format!(
                "{name:?} leads out of run {run_id}'s directory, or the directory out of the runs directory"
            ))),
        }
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

    /// The current session of the run's agent of role over ACP, as its journal holds it (crate::acp::replay), from
    /// the event after after: {"session_start", "events", "answerable"}, answerable being whether the run's
    /// orchestrator listens on its socket (crate::acp::SOCKET_RECORD), which answers and messages go to. Errors: as
    /// read's; Missing for no such run, or a role with no journal, as every role of a run in herdr; Conflict for a
    /// journal from another version of octoflow.
    pub fn conversation(&self, run_id: &str, role: &str, after: u64) -> Result<Object, RunReadError> {
        let Some(text) = self.read(run_id, &acp::journal_name(role))? else {
            return Err(RunReadError::Missing(format!("run {run_id} has no conversation of a {role} agent")));
        };
        let replay = acp::replay(&text, after).map_err(|version| {
            let version = version.map_or_else(|| "none".to_string(), |v| v.to_string());
            RunReadError::Conflict(format!(
                "run {run_id} is from another version of octoflow: its {role} agent's journal is version {version}, \
                 and this one reads {}",
                acp::PROTOCOL
            ))
        })?;
        let answerable = self.read(run_id, acp::SOCKET_RECORD)?.is_some();
        Ok(object([
            ("session_start", replay.session_start.map_or(Value::Null, Value::from)),
            ("events", Value::Array(replay.events)),
            ("answerable", Value::from(answerable)),
        ]))
    }

    /// The directory of the run, which a run over ACP records its socket in (crate::acp::ask). Errors: Refused for a
    /// run id that is not a plain name; Missing for a project on a machine, which has no run over ACP.
    pub fn acp_dir(&self, run_id: &str) -> Result<String, RunReadError> {
        let dir = self.dir(run_id)?;
        if self.remote() {
            return Err(RunReadError::Missing(format!("run {run_id} is on a machine, and runs no agent over ACP")));
        }
        Ok(dir)
    }

    /// The run's agent of role as its state.json saves it: its herdr name and its run's herdr server. Errors:
    /// Refused for a run id that is not a plain name; Missing for no such run, one whose state.json leads out as
    /// records leaves it out, or no started agent of that role in it; Failed when state.json cannot be read, is not
    /// JSON, or is not a state this engine reads (SavedRun::problem).
    fn agent(&self, run_id: &str, role: &str) -> Result<AgentAddress, RunReadError> {
        let dir = self.dir(run_id)?;
        let path = format!("{dir}/state.json");
        let RunFileText::Text(text) = self.host.read_inside(&dir, "state.json")? else {
            return Err(RunReadError::Missing(format!("no run {run_id}")));
        };
        let run = SavedRun::read(0, &dir, text.as_bytes());
        // A problem with the file's text names the file already; one with its fields names only the run.
        let state = run.state().map_err(|e| match e.0.starts_with(&path) {
            true => e,
            false => OrchestratorError(format!("{path}: {e}")),
        })?;
        let name = state.agents.get(role).map(|a| &a.name).filter(|n| !n.is_empty());
        let Some(name) = name else {
            return Err(RunReadError::Missing(format!("run {run_id} has no {role} agent")));
        };
        Ok(AgentAddress { name: name.clone(), server: state.herdr_server.clone() })
    }

    /// Whether a run this project starts is detached (Launch::Detached), rather than in herdr.
    pub fn detached(&self) -> bool {
        matches!(self.launch, Launch::Detached)
    }

    /// Start the run request asks for, as run_id, by program, the orchestrator's binary, as the project's Launch says,
    /// and wait (StartWait::until) for it to save its state.json or for its command to exit without one. Once the run
    /// is started a look that fails, at its state.json or at it, is no answer: the wait goes on, so that the client
    /// still gets the id of a run that is running. Gives how the wait ended, and the named herdr session the run's
    /// pane is in (Launched::herdr_session), None for a run not in herdr. Errors: Refused for a run id that is not a
    /// plain name; for the rest, start_in_herdr's and start_detached's.
    pub fn start(
        &self,
        request: &RunRequest,
        program: &str,
        run_id: &str,
        wait: StartWait,
        stopping: &Interrupt,
    ) -> Result<(Started, Option<String>), RunReadError> {
        let dir = self.dir(run_id)?;
        let saved = || self.host.read(&format!("{dir}/state.json")).is_ok_and(|text| text.is_some());
        match &self.launch {
            Launch::Herdr(launcher) => self.start_in_herdr(&**launcher, request, program, run_id, wait, stopping),
            Launch::Detached => {
                if self.remote() {
                    return Err(
                        OrchestratorError::new("a run over ACP starts on this machine only, not on a machine").into()
                    );
                }
                let log = format!("{dir}/{START_LOG}");
                std::fs::create_dir_all(&dir).map_err(|e| OrchestratorError(format!("cannot make {dir}: {e}")))?;
                let target = ["--cwd".to_string(), self.cwd.clone(), "--acp".to_string()];
                let run = spawn_detached(&request.argv(program, run_id, &target), &self.launch_dir, &log)?;
                let ended = wait.until(stopping, || {
                    if saved() {
                        return Some(Started::Listed);
                    }
                    if !run.exited() {
                        return None;
                    }
                    // As for a run in herdr.
                    Some(if saved() { Started::Listed } else { Started::Exited(log_tail(&log)) })
                });
                Ok((ended, None))
            }
        }
    }

    /// start's run in a new workspace of launcher's herdr, labelled as the run labels its own
    /// (crate::herdr::workspace_label). The project is given to `run` as `--cwd` and the directory serve resolved, for
    /// a project here, and as the client named it, with its `--machine`, for one on a machine. Errors: Failed, before
    /// any workspace opens, when the panes of a herdr the run types into start a shell that its lines cannot be typed
    /// into or that cannot be told (crate::herdr::pane_shell): the launcher's, which the run's command is typed into,
    /// and for a project on a machine also the machine's, where its agents run; and when herdr cannot launch it
    /// (Launcher::launch).
    fn start_in_herdr(
        &self,
        launcher: &dyn Launcher,
        request: &RunRequest,
        program: &str,
        run_id: &str,
        wait: StartWait,
        stopping: &Interrupt,
    ) -> Result<(Started, Option<String>), RunReadError> {
        let state = format!("{}/state.json", self.dir(run_id)?);
        let label = crate::herdr::workspace_label(run_id, &request.task);
        // The pane starts in the resolved directory too, where a relative cwd or one with ~ would resolve elsewhere.
        let target = if self.remote() { self.target.clone() } else { vec!["--cwd".to_string(), self.cwd.clone()] };
        let argv = request.argv(program, run_id, &target);
        // The launcher checks its own panes; a project here has its agents in those too.
        if self.remote() {
            crate::herdr::pane_shell(&*self.host)?;
        }
        let launched = launcher.launch(&self.launch_dir, &label, &argv).map_err(OrchestratorError::from)?;
        let saved = || self.host.read(&state).is_ok_and(|text| text.is_some());
        let ended = wait.until(stopping, || {
            if saved() {
                return Some(Started::Listed);
            }
            let shown = launcher.exited(&launched, START_ERROR_LINES).ok().flatten()?;
            // It may have saved its state.json just before it exited, as a run that fails after its claim does.
            Some(if saved() { Started::Listed } else { Started::Exited(format!("its pane showed:\n{shown}")) })
        });
        Ok((ended, launched.herdr_session().map(String::from)))
    }

    fn dir(&self, run_id: &str) -> Result<String, RunReadError> {
        Ok(run_dir(&self.cwd, plain_name(run_id)?))
    }
}

/// What Started::Exited says of a detached run that exited before it saved its state.json: the last
/// START_ERROR_LINES lines of its log, at log.
fn log_tail(log: &str) -> String {
    match std::fs::read_to_string(log) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let from = lines.len().saturating_sub(START_ERROR_LINES as usize);
            format!("its log ends:\n{}", lines[from..].join("\n"))
        }
        Err(e) => format!("its log {log} cannot be read: {e}"),
    }
}

/// The most lines of its pane, or its log, that a run which exited before it saved its state.json gives as its error.
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
    /// again until the record changes or state.json is written again; always false for a running run. RunWatch::poll
    /// says when a list settles.
    files_settled: bool,
    /// The heartbeat_at of the state.json the record was read from; each save by the run's orchestrator sets it, so
    /// another one means state.json was written again since. None when it had none.
    saved_at: Option<String>,
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
    /// or failed run's are listed until its state.json is FINISHED_RUN_WRITES old (the record's heartbeat_age) and two
    /// polls in a row with the same record list the same files; then again only when its record changes or its
    /// orchestrator saves its state.json again (heartbeat_at), as nothing else drives it. The wait is for the files the
    /// engine writes after the save that finishes the run, which FINISHED_RUN_WRITES names; one written later still is
    /// told of only at the next such save. A failed run's release makes one after its summary.md (Workflow::release);
    /// a resumed done run's ending does not (TODO.md). A poll that
    /// fails sends only its problem, and changes nothing else of what the watch holds.
    pub fn poll(&mut self) -> Vec<Event> {
        match self.changes() {
            Ok(events) => self.problem_events(None).into_iter().chain(events).collect(),
            Err(e) => self.problem_events(Some(e.to_string())),
        }
    }

    fn changes(&mut self) -> Result<Vec<Event>, RunReadError> {
        let mut events = vec![];
        let mut sent = BTreeMap::new();
        let here = local_host();
        for run in self.project.saved_runs()? {
            let record = self.project.record(&run, &here);
            let saved_at = run.shown().heartbeat_at.clone();
            // Always a string: the run's id, or its directory's name for a state.json that names no run
            // (SavedRun::read).
            let id = record.get("id").cloned().unwrap_or(Value::Null);
            let run_id = id.text();
            let mut key = record.clone();
            for aging in AGING_RECORD_KEYS {
                key.remove(aging);
            }
            let old = self.sent.get(&run_id);
            let same_record = old.is_some_and(|old| old.key == key);
            let finished = record.get("health").is_none_or(Value::is_null);
            // heartbeat_age: whole seconds since state.json was written.
            let age = record.get("heartbeat_age").and_then(Value::as_u64);
            let written = age.is_none_or(|age| age >= FINISHED_RUN_WRITES.as_secs());
            let (files, files_settled) = match old {
                Some(old) if same_record && old.saved_at == saved_at && finished && old.files_settled => {
                    (old.files.clone(), true)
                }
                _ => {
                    let files = self.project.files(&run_id)?;
                    let settled = finished && written && same_record && old.is_some_and(|old| old.files == files);
                    (files, settled)
                }
            };
            if !same_record {
                events.push(Event::new("record", [("run", id.clone()), ("record", Value::Object(record))]));
            }
            if old.is_none_or(|old| old.files != files) {
                events.push(Event::new("files", [("run", id), ("files", files.clone())]));
            }
            sent.insert(run_id, Sent { key, files, files_settled, saved_at });
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
