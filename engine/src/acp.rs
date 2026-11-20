//! The ACP backend: a run's agents as child processes of its orchestrator, each speaking the Agent Client Protocol
//! ([ACP](https://agentclientprotocol.com)) over its stdin and stdout, started as its harness's ACP launch says
//! (crate::harness::Harness::acp_launch). [`AcpAgents`] is the crate::agents::Agents of such a run, which the
//! run's state names (crate::state::Backend::Acp, saved as state format 3).
//!
//! A run's directory gets, besides the handoff files, for each role `<role>.acp.log` (its agents' stderr, appended),
//! `<role>.events.jsonl` (its journal, journal), and `<role>.acp.group` (its agent's process group, groups); and
//! USAGE_FILE (each answered prompt's tokens, session_usage), and while its orchestrator runs, its socket's record
//! (socket::SOCKET_RECORD) and maybe its socket (in `sock/`).
//!
//! The ACP backend runs no agent the human sees in a terminal: it shows nothing (Agents::show), answers nothing on its
//! own, and a permission an agent asks for waits until AcpAgents::answer gives it, as the run's socket asks; the
//! human's message reaches an agent only while the run waits on the human (AcpAgents::message).

mod connection;
mod groups;
mod journal;
mod socket;

pub use groups::end_leftovers;
pub use journal::{replay, Replay, PROTOCOL};
pub use socket::{ask, SocketError, SOCKET_RECORD};

use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};

use crate::agents::{AgentError, AgentLaunch, Agents, RunPlaces, Seen, Started, Waiting};
use crate::env::without_withheld;
use crate::error::OrchestratorError;
use crate::harness::{harness, Harness, ModeSetting, Tokens, Usage};
use crate::human::Human;
use crate::interrupt::Interrupt;
use crate::state::{AgentRecord, Backend, RunState};
use connection::{Connection, Failure, Records};
use journal::Journal;
use socket::{Commands, Listener};

/// The file in the run directory that holds the tokens of every prompt an agent answered, one JSON object a line:
/// `{"session", "when", "tokens"}`, as session_usage reads them.
const USAGE_FILE: &str = "acp-usage.jsonl";

/// How long a request that starts an agent, its session, its mode or its model waits for the agent's answer: an
/// agent may log in or load a long session first.
const REQUEST_SECONDS: u64 = 120;

/// The protocol version the engine speaks (ACP's `initialize`).
const PROTOCOL_VERSION: u64 = 1;

/// The agents of one run, which its orchestrator drives over ACP: each role's, once started, until it exits or is
/// ended; and the run's socket, from the run's open on. Dropping it kills the agents (Connection's drop) and closes
/// the socket.
pub struct AcpAgents<'h> {
    /// Told an agent's error, as find comes upon it.
    human: &'h dyn Human,
    /// The environment every agent starts in, as crate::env::child_env gives it; its PATH is where the harnesses'
    /// programs are found.
    env: Vec<(OsString, OsString)>,
    /// The run's socket, once open bound it. Before running, so that it closes, its threads stopped, before the
    /// agents are dropped.
    listener: Mutex<Option<Listener>>,
    /// The agents, shared with the socket's threads.
    running: Arc<Running>,
}

/// A run's running agents, and the run's Ctrl-C, which a `stop` raises.
struct Running {
    /// Each role's running agent, by role.
    agents: Mutex<HashMap<String, Agent>>,
    interrupt: Interrupt,
}

/// Why AcpAgents::answer or AcpAgents::message did not do what the run's socket asked.
#[derive(Debug, PartialEq)]
pub enum CommandError {
    /// The role has no agent running.
    NoAgent,
    /// The role's agent's session has no request for a permission of that number.
    NoRequest,
    /// The request was answered before, with option, by `human` or `stop`.
    AlreadyAnswered { option: String, by: String },
    /// The request offers no such option.
    BadOption,
    /// The run does not wait on the human's message to the agent (Agents::await_human), or a prompt of it is
    /// pending.
    NotWaiting,
    /// The answer or message was taken, but could not be written to the agent, which has exited.
    Unwritten(OrchestratorError),
}

/// A role's agent that started.
struct Agent {
    /// Shared with a wait that must not hold AcpAgents::agents, so that an answer can end it (wait_ready).
    connection: Arc<Connection>,
    /// The ACP session it runs in.
    session: String,
    /// Its harness, as HARNESS_NAMES names it.
    harness: &'static str,
}

impl<'h> AcpAgents<'h> {
    /// A run's ACP agents, none started yet, each to start in this process's environment as child_env gives it; an
    /// agent's error is told to human, and a `stop` on the run's socket raises interrupt.
    pub fn new(human: &'h dyn Human, interrupt: &Interrupt) -> AcpAgents<'h> {
        AcpAgents::in_environment(human, interrupt, std::env::vars_os())
    }

    /// As new, in the environment vars as child_env would give it, were vars the engine's own: what the tests give
    /// in place of the process's environment, which is shared by every test.
    fn in_environment(
        human: &'h dyn Human,
        interrupt: &Interrupt,
        vars: impl Iterator<Item = (OsString, OsString)>,
    ) -> AcpAgents<'h> {
        let running = Running { agents: Mutex::new(HashMap::new()), interrupt: interrupt.clone() };
        AcpAgents { human, env: without_withheld(vars), listener: Mutex::new(None), running: Arc::new(running) }
    }

    /// Answer the permission that the role's agent asked for and its journal numbered request with option, one of
    /// the optionIds the request offers, as the human's answer: what the socket's `answer` does. The first answer
    /// wins. Errors: CommandError's, as it says.
    pub fn answer(&self, role: &str, request: u64, option: &str) -> Result<(), CommandError> {
        self.running.answer(role, request, option)
    }

    /// Send the role's agent text, the human's message, as its next prompt, while the run waits on the human's message
    /// to it (Agents::await_human): what the socket's `message` does. Errors: NoAgent, NotWaiting and Unwritten, as
    /// CommandError says.
    pub fn message(&self, role: &str, text: &str) -> Result<(), CommandError> {
        self.running.message(role, text)
    }

    /// Stop the run, as a Ctrl-C does, its agents first told: what the socket's `stop` does. Each agent's pending
    /// permissions are answered `cancelled` and its unanswered prompts cancelled (ACP's `session/cancel`), then the
    /// run's Interrupt is raised, which releases the run as interrupted. A second stop does the same again.
    pub fn stop(&self) {
        self.running.stop();
    }

    /// The agents, whatever thread panicked holding them: every change to them is whole.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Agent>> {
        self.running.lock()
    }

    /// Start harness h's agent for the role of run as launch says, in a new session or, for launch's session, that
    /// one. Returns it and what of the launch it started without. Errors: Refused for a session it cannot resume;
    /// Failed for a harness without an ACP launch, and for an agent that does not start or answer.
    fn launch(
        &self,
        run: &RunState,
        role: &str,
        h: &'static Harness,
        launch: &AgentLaunch,
    ) -> Result<(Agent, Vec<String>), AgentError> {
        let mode = launch.permission_mode.filter(|m| !m.is_empty()).unwrap_or("default");
        let Some(how) = h.acp_launch(mode, launch.model, launch.extra_dir) else {
            return Err(AgentError::Failed(OrchestratorError(format!(
                "{} cannot run in a run the app starts: the app does not offer it yet",
                h.kind
            ))));
        };
        let dir = run.dir();
        let journal = Arc::new(Journal::open(&format!("{dir}/{}", journal_name(role)))?);
        let records = Records {
            stderr: &format!("{dir}/{role}.acp.log"),
            usage: &format!("{dir}/{USAGE_FILE}"),
            group: &group_record(&dir, role),
            journal: journal.clone(),
        };
        let connection = Connection::spawn(how.program, &how.args, self.env.iter().cloned(), run.work_dir(), records)?;
        let journal_session = |session: &str, resumed: bool| {
            journal.write("session", json!({"session": session, "harness": h.kind, "resumed": resumed}));
        };
        let ask = |method: &str, params: Value| {
            connection.request(method, params, Duration::from_secs(REQUEST_SECONDS)).map_err(|e| failed(h, method, e))
        };
        let capabilities =
            ask("initialize", initialize_params())?.get("agentCapabilities").cloned().unwrap_or(Value::Null);
        let mut unpassed = vec![];
        let mut params = json!({"cwd": run.work_dir(), "mcpServers": []});
        if let Some(dir) = how.extra_dir {
            if capable(&capabilities, "/sessionCapabilities/additionalDirectories") {
                params["additionalDirectories"] = json!([dir]);
            } else {
                unpassed.push(format!("{} cannot be granted {dir}; it may ask before each write there", h.kind));
            }
        }
        // A session reopened is journaled before it is asked for, since the agent replays its conversation as
        // updates before it answers (ACP's session/load); one refused is followed by a new one, journaled after it.
        let (session, opened) = match launch.session.filter(|s| !s.is_empty()) {
            Some(session) => {
                journal_session(session, true);
                (session.to_string(), reopen(h, &connection, &capabilities, session, params)?)
            }
            None => {
                let opened = ask("session/new", params)?;
                let session = opened.get("sessionId").and_then(Value::as_str).ok_or_else(|| {
                    AgentError::Failed(OrchestratorError(format!("{} opened a session with no sessionId", h.kind)))
                })?;
                journal_session(session, false);
                (session.to_string(), opened)
            }
        };
        unpassed.extend(set_mode(h, &ask, &session, &opened, mode, &how.mode)?);
        if let Some(model) = how.model {
            unpassed.extend(set_model(h, &ask, &session, &opened, &model)?);
        }
        Ok((Agent { connection: Arc::new(connection), session, harness: h.kind }, unpassed))
    }
}

impl Agents for AcpAgents<'_> {
    fn bind(&self, run: &mut RunState) {
        run.backend = Backend::Acp;
    }

    /// Nothing outlives an orchestrator, so there is no place to keep: a run resumes with no agent. Opens the run's
    /// socket (socket's module comment); a socket that cannot be opened is told to human, and the run goes on
    /// without one, which the app then shows read-only. Errors: run's agents run in another backend.
    fn open(&self, run: &mut RunState, human: &dyn Human) -> Result<bool, AgentError> {
        if run.backend != Backend::Acp {
            return Err(AgentError::Failed(OrchestratorError(format!(
                "run {} runs its agents in herdr, not over ACP; resume it from the command line",
                run.run_id
            ))));
        }
        human.log(&format!("run {} runs its agents over ACP", run.run_id));
        let commands: Arc<dyn Commands> = self.running.clone();
        match Listener::bind(&run.dir(), &run.run_id, run.key(), commands) {
            Ok(listener) => *self.listener.lock().unwrap_or_else(|e| e.into_inner()) = Some(listener),
            Err(e) => human.log(&format!("run {} has no socket, so the app cannot answer it: {e}", run.run_id)),
        }
        Ok(false)
    }

    fn close(&self, run: &RunState, human: &dyn Human) {
        if let Err(e) = self.end(run) {
            human.log(&format!("could not end the ACP agents of run {}: {e}", run.run_id));
        }
    }

    /// The record names the agent `<role>-<run key>`, as herdr's, and places it nowhere.
    fn next_record(
        &self,
        run: &RunState,
        role: &str,
        _order: &[String],
        harness: &str,
    ) -> Result<AgentRecord, AgentError> {
        Ok(AgentRecord::new(&format!("{role}-{}", run.key()), "", harness))
    }

    /// The agent is started in its working directory (RunState::work_dir), set to the run's permission mode's
    /// equivalent, `default`'s for a run without one, and to its model. A session it resumes is loaded where it
    /// can load one (ACP's `loadSession`), else resumed where it can (`sessionCapabilities.resume`); with neither, or
    /// when the agent refuses it, the start is Refused, so a fresh session follows. The role's agent before it, if
    /// still running, is killed first, and so is the process group of one an orchestrator of the run before left
    /// running (groups::end_leftover), told to the human.
    fn start(&self, run: &RunState, role: &str, launch: &AgentLaunch) -> Result<Started, AgentError> {
        if let Some(old) = self.lock().remove(role) {
            old.connection.kill();
        }
        match groups::end_leftover(&group_record(&run.dir(), role)) {
            Ok(false) => {}
            Ok(true) => self.human.log(&format!("[{role}] ended the agent an earlier orchestrator of the run left")),
            Err(e) => self.human.log(&format!("[{role}] cannot end what an earlier agent left: {e}")),
        }
        let h = harness(launch.harness)?;
        let (agent, unpassed) = self.launch(run, role, h, launch)?;
        self.lock().insert(role.to_string(), agent);
        Ok(Started { asking: false, unpassed })
    }

    fn wait_ready(&self, _run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError> {
        let Some(connection) = self.lock().get(role).map(|a| a.connection.clone()) else { return Ok(true) };
        Ok(connection.wait_unblocked(seconds))
    }

    /// An agent that answered a prompt with an error is told to the human, killed, and found exited.
    fn find(&self, _run: &RunState, role: &str) -> Result<Option<Seen>, AgentError> {
        let mut agents = self.lock();
        let Some(agent) = agents.get(role) else { return Ok(None) };
        if let Some(error) = agent.connection.failure() {
            self.human.log(&format!("[{role}] {} failed: {error}", agent.harness));
            agent.connection.kill();
        }
        match agent.connection.activity() {
            Some(activity) => Ok(Some(Seen { activity, session: Some(agent.session.clone()) })),
            None => {
                agents.remove(role);
                Ok(None)
            }
        }
    }

    fn prompt(&self, _run: &RunState, role: &str, text: &str) -> Result<(), AgentError> {
        let agents = self.lock();
        let agent = agents.get(role).ok_or_else(|| no_agent(role))?;
        Ok(agent.connection.prompt(&agent.session, text)?)
    }

    /// An ACP agent takes a prompt as it is sent: taken while it is alive.
    fn wait_taken(&self, _run: &RunState, role: &str, _seconds: u64) -> Result<bool, AgentError> {
        Ok(self.lock().get(role).is_some_and(|a| a.connection.activity().is_some()))
    }

    /// Nothing to show: the app shows each agent in a tab of its own.
    fn show(&self, _run: &RunState, _role: &str) -> Result<(), AgentError> {
        Ok(())
    }

    /// Journaled as `waiting`, which the app shows a message box for.
    fn await_human(&self, _run: &RunState, role: &str, why: Waiting) {
        if let Some(agent) = self.lock().get(role) {
            agent.connection.await_human(why.name());
        }
    }

    fn whereabouts(&self, _run: &RunState, role: &str) -> String {
        format!("the {role} agent's tab in the app")
    }
}

impl RunPlaces for AcpAgents<'_> {
    /// Its running agents, while it has any.
    fn place(&self, _run: &RunState) -> Result<Option<String>, AgentError> {
        let n = self.lock().values().filter(|a| a.connection.activity().is_some()).count();
        Ok((n > 0).then(|| format!("{n} ACP agents")))
    }

    /// Kill every agent.
    fn end(&self, _run: &RunState) -> Result<(), AgentError> {
        for (_, agent) in self.lock().drain() {
            agent.connection.kill();
        }
        Ok(())
    }
}

impl Running {
    /// As AcpAgents::lock.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Agent>> {
        self.agents.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Commands for Running {
    fn answer(&self, role: &str, request: u64, option: &str) -> Result<(), CommandError> {
        let connection = self.lock().get(role).map(|a| a.connection.clone()).ok_or(CommandError::NoAgent)?;
        connection.answer(request, option)
    }

    fn message(&self, role: &str, text: &str) -> Result<(), CommandError> {
        let (connection, session) = {
            let agents = self.lock();
            let agent = agents.get(role).ok_or(CommandError::NoAgent)?;
            (agent.connection.clone(), agent.session.clone())
        };
        connection.message(&session, text)
    }

    fn stop(&self) {
        let connections: Vec<Arc<Connection>> = self.lock().values().map(|a| a.connection.clone()).collect();
        for connection in connections {
            connection.stop();
        }
        self.interrupt.raise();
    }
}

/// The name of the role's journal in its run directory (journal's module comment says what it holds).
pub fn journal_name(role: &str) -> String {
    format!("{role}.events.jsonl")
}

/// The path of the record of the role's agent's process group in the run directory dir.
fn group_record(dir: &str, role: &str) -> String {
    format!("{dir}/{role}{}", groups::GROUP_SUFFIX)
}

/// The token usage the run recorded for session, oldest first (Usage::when being when the prompt was sent), from log,
/// the run's USAGE_FILE as it is on disk. A line that does not read is passed over.
pub fn session_usage(log: &str, session: &str) -> Vec<Usage> {
    let mut out = vec![];
    for line in log.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue };
        if record.get("session").and_then(Value::as_str) != Some(session) {
            continue;
        }
        let (Some(when), Some(tokens)) = (record.get("when").and_then(Value::as_f64), record.get("tokens")) else {
            continue;
        };
        let count = |key: &str| tokens.get(key).and_then(Value::as_i64);
        let (Some(input), Some(output), Some(cache_read), Some(cache_write)) =
            (count("input"), count("output"), count("cache_read"), count("cache_write"))
        else {
            continue;
        };
        out.push(Usage { when, tokens: Tokens { input, output, cache_read, cache_write } });
    }
    out
}

/// What harness h's ACP agent says of itself when asked `initialize`, started as an ACP run starts it in `default`
/// mode (Harness::acp_launch, with no model and no extra directory), from env's PATH and with env alone, working in
/// cwd: its `agentInfo` as "<name> <version>", or "answers over ACP" when it gives none. The agent is killed once it
/// answers, and its stderr dropped. Errors: h has no ACP launch; its program does not start; it exits, or does not
/// answer within timeout; stopping is raised before it answers, which kills it.
pub fn probe(
    h: &Harness,
    env: &[(OsString, OsString)],
    cwd: &str,
    timeout: Duration,
    stopping: &Interrupt,
) -> Result<String, OrchestratorError> {
    let Some(how) = h.acp_launch("default", None, None) else {
        return Err(OrchestratorError(format!("{} has no ACP launch", h.kind)));
    };
    // It answers no prompt and is no run's, so nothing of it is kept.
    let records = Records {
        stderr: "/dev/null",
        usage: "/dev/null",
        group: "/dev/null",
        journal: Arc::new(Journal::open("/dev/null")?),
    };
    let connection = Arc::new(Connection::spawn(how.program, &how.args, env.iter().cloned(), cwd, records)?);
    let killed = connection.clone();
    let Some(watch) = stopping.watch(move || killed.kill()) else {
        return Err(OrchestratorError::new("interrupted"));
    };
    let answer = connection.request("initialize", initialize_params(), timeout);
    if watch.interrupted() {
        return Err(OrchestratorError::new("interrupted"));
    }
    let info = answer.map_err(|e| unanswered(h, "initialize", e, timeout))?.get("agentInfo").cloned();
    let field = |key: &str| info.as_ref().and_then(|i| i.get(key)).and_then(Value::as_str).map(String::from);
    Ok(match (field("name"), field("version")) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name,
        _ => "answers over ACP".to_string(),
    })
}

/// The params of the engine's `initialize`: the protocol version it speaks, and that it offers its agents neither
/// file access nor terminals, which they then do themselves.
fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
    })
}

/// Where run's USAGE_FILE is.
pub fn usage_file(run: &RunState) -> String {
    format!("{}/{USAGE_FILE}", run.dir())
}

/// Open session again in connection's agent of harness h, which answered `initialize` with capabilities, with params
/// as `session/new` takes them (its working directory and roots): loaded where it can load one, else resumed where it
/// can. Returns the agent's answer, as `session/new`'s. Errors: Refused when it can do neither, or refuses; Failed
/// when it does not answer.
fn reopen(
    h: &Harness,
    connection: &Connection,
    capabilities: &Value,
    session: &str,
    mut params: Value,
) -> Result<Value, AgentError> {
    let method = if capabilities.get("loadSession").and_then(Value::as_bool) == Some(true) {
        "session/load"
    } else if capable(capabilities, "/sessionCapabilities/resume") {
        "session/resume"
    } else {
        return Err(AgentError::Refused(OrchestratorError(format!(
            "{} can neither load nor resume a session over ACP",
            h.kind
        ))));
    };
    params["sessionId"] = session.into();
    match connection.request(method, params, Duration::from_secs(REQUEST_SECONDS)) {
        Ok(opened) => Ok(opened),
        Err(Failure::Refused(e)) => Err(AgentError::Refused(OrchestratorError(format!("{} {method}: {e}", h.kind)))),
        Err(e) => Err(failed(h, method, e)),
    }
}

/// Put the session that the agent of harness h opened, as its answer opened has it, in mode's equivalent as setting
/// says, asking the agent by ask. Setting a mode is this function alone: ACP's session modes are to become session
/// config options ([session modes](https://agentclientprotocol.com/protocol/session-modes)). Returns the log's
/// sentence when the agent runs without the equivalent: h has none, or the session does not offer it. Errors: the
/// agent refuses `session/set_mode`, or does not answer it.
fn set_mode(
    h: &Harness,
    ask: &dyn Fn(&str, Value) -> Result<Value, AgentError>,
    session: &str,
    opened: &Value,
    mode: &str,
    setting: &ModeSetting,
) -> Result<Option<String>, AgentError> {
    match *setting {
        ModeSetting::Launched => Ok(None),
        ModeSetting::NoEquivalent => Ok(Some(h.without_mode(mode))),
        ModeSetting::Set(id) if !offers(opened, "modes", "availableModes", "id", id) => Ok(Some(format!(
            "{}'s session offers no mode {id}, its equivalent of --permission-mode {mode}; starting it without one",
            h.kind
        ))),
        ModeSetting::Set(id) => {
            if opened.pointer("/modes/currentModeId").and_then(Value::as_str) != Some(id) {
                ask("session/set_mode", json!({"sessionId": session, "modeId": id}))?;
            }
            Ok(None)
        }
    }
}

/// Put the session that the agent of harness h opened, as its answer opened has it, on model, asking the agent by ask:
/// by the session config option of category `model` where the session has one (ACP's
/// [session config options](https://agentclientprotocol.com/protocol/session-config-options)), else by the session's
/// models (`session/set_model`). Returns the log's sentence when the session does not offer model. Errors: the agent
/// refuses the request, or does not answer it.
fn set_model(
    h: &Harness,
    ask: &dyn Fn(&str, Value) -> Result<Value, AgentError>,
    session: &str,
    opened: &Value,
    model: &str,
) -> Result<Option<String>, AgentError> {
    let options = opened.get("configOptions").and_then(Value::as_array);
    let option = options.and_then(|o| o.iter().find(|o| o.get("category").and_then(Value::as_str) == Some("model")));
    let offered = match option {
        Some(option) => option_offers(option, model),
        None => offers(opened, "models", "availableModels", "modelId", model),
    };
    if !offered {
        return Ok(Some(format!("{} offers no model {model}; starting it on its own", h.kind)));
    }
    match option {
        Some(option) if option.get("currentValue").and_then(Value::as_str) != Some(model) => {
            ask("session/set_config_option", json!({"sessionId": session, "configId": option["id"], "value": model}))?;
        }
        Some(_) => {}
        None if opened.pointer("/models/currentModelId").and_then(Value::as_str) != Some(model) => {
            ask("session/set_model", json!({"sessionId": session, "modelId": model}))?;
        }
        None => {}
    }
    Ok(None)
}

/// Whether the select config option offers value: among its `options`, flat or in groups.
fn option_offers(option: &Value, value: &str) -> bool {
    let is = |o: &Value| o.get("value").and_then(Value::as_str) == Some(value);
    let mut options = option.get("options").and_then(Value::as_array).into_iter().flatten();
    options.any(|o| match o.get("options").and_then(Value::as_array) {
        Some(group) => group.iter().any(is),
        None => is(o),
    })
}

/// Whether the agent's capabilities, as it answered `initialize`, have the one at pointer: present, and neither null
/// nor false (an empty object is one).
fn capable(capabilities: &Value, pointer: &str) -> bool {
    capabilities.pointer(pointer).is_some_and(|c| !c.is_null() && c != false)
}

/// Whether the session the agent opened, as its answer opened has it, offers the choice of id among
/// `<group>.<list>[].<key>`: a mode (`modes.availableModes[].id`) or a model (`models.availableModels[].modelId`).
fn offers(opened: &Value, group: &str, list: &str, key: &str, id: &str) -> bool {
    let choices = opened.get(group).and_then(|g| g.get(list)).and_then(Value::as_array);
    choices.is_some_and(|choices| choices.iter().any(|c| c.get(key).and_then(Value::as_str) == Some(id)))
}

/// The error of harness h's agent's request of method that got no result, waited for REQUEST_SECONDS.
fn failed(h: &Harness, method: &str, failure: Failure) -> AgentError {
    AgentError::Failed(unanswered(h, method, failure, Duration::from_secs(REQUEST_SECONDS)))
}

/// Why harness h's agent's request of method, waited for at most timeout, got no result.
fn unanswered(h: &Harness, method: &str, failure: Failure, timeout: Duration) -> OrchestratorError {
    let why = match failure {
        Failure::Refused(e) => e,
        Failure::Exited => "it exited".to_string(),
        Failure::TimedOut => format!("no answer within {}s", timeout.as_secs()),
        Failure::Unwritten(e) => e.0,
    };
    OrchestratorError(format!("{} {method}: {why}", h.kind))
}

/// The error of a role that has no agent.
fn no_agent(role: &str) -> AgentError {
    AgentError::Failed(OrchestratorError(format!("the {role} role has no ACP agent")))
}

#[cfg(test)]
mod tests;
