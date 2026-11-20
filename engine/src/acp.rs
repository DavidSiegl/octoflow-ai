//! The ACP backend: a run's agents as child processes of its orchestrator, each speaking the Agent Client Protocol
//! ([ACP](https://agentclientprotocol.com)) over its stdin and stdout, started as its harness's ACP launch says
//! (crate::harness::Harness::acp_launch). [`AcpAgents`] is the crate::agents::Agents of such a run, which the
//! run's state names (crate::state::Backend::Acp, saved as state format 3).
//!
//! An agent's run directory gets, besides the handoff files, `<role>.acp.log` (each agent's stderr, appended) and
//! USAGE_FILE (each answered prompt's tokens, session_usage).
//!
//! The ACP backend runs no agent the human sees in a terminal: it shows nothing (Agents::show), answers nothing on its
//! own, and a permission an agent asks for waits until AcpAgents::answer gives it.

mod connection;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};

use crate::agents::{AgentError, AgentLaunch, Agents, RunPlaces, Seen, Started};
use crate::error::OrchestratorError;
use crate::harness::{harness, Harness, ModeSetting, Tokens, Usage};
use crate::human::Human;
use crate::state::{AgentRecord, Backend, RunState};
use connection::{Connection, Failure};

/// The file in the run directory that holds the tokens of every prompt an agent answered, one JSON object a line:
/// `{"session", "when", "tokens"}`, as session_usage reads them.
const USAGE_FILE: &str = "acp-usage.jsonl";

/// How long a request that starts an agent, its session, its mode or its model waits for the agent's answer: an
/// agent may log in or load a long session first.
const REQUEST_SECONDS: u64 = 120;

/// The protocol version the engine speaks (ACP's `initialize`).
const PROTOCOL_VERSION: u64 = 1;

/// The agents of one run, which its orchestrator drives over ACP: each role's, once started, until it exits or is
/// ended. Dropping it kills them (Connection's drop).
pub struct AcpAgents<'h> {
    /// Told an agent's error, as find comes upon it.
    human: &'h dyn Human,
    /// The directory the harnesses' programs are taken from; None for PATH, as every run but a test's.
    bin: Option<String>,
    /// Each role's running agent, by role.
    agents: Mutex<HashMap<String, Agent>>,
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
    /// A run's ACP agents, none started yet, with the harnesses' programs on PATH; an agent's error is told to human.
    pub fn new(human: &'h dyn Human) -> AcpAgents<'h> {
        AcpAgents { human, bin: None, agents: Mutex::new(HashMap::new()) }
    }

    /// As new, the harnesses' programs taken from the directory bin rather than PATH: a test's fake agents.
    #[cfg(test)]
    fn in_bin(human: &'h dyn Human, bin: &str) -> AcpAgents<'h> {
        AcpAgents { bin: Some(bin.to_string()), ..AcpAgents::new(human) }
    }

    /// Answer the role's agent's oldest request for a permission with the option of option_id, one of those the
    /// request offers. Errors: the role has no agent, or it waits on no permission; the answer cannot be sent.
    pub fn answer(&self, role: &str, option_id: &str) -> Result<(), AgentError> {
        let agents = self.lock();
        let agent = agents.get(role).ok_or_else(|| no_agent(role))?;
        if !agent.connection.answer(option_id)? {
            return Err(AgentError::Failed(OrchestratorError(format!("the {role} agent asks for no permission"))));
        }
        Ok(())
    }

    /// The agents, whatever thread panicked holding them: every change to them is whole.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Agent>> {
        self.agents.lock().unwrap_or_else(|e| e.into_inner())
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
        // A run without a mode runs as `default`, but only a mode the run names is reported unpassed, as herdr does.
        let named = launch.permission_mode.filter(|m| !m.is_empty());
        let mode = named.unwrap_or("default");
        let Some(how) = h.acp_launch(mode, launch.model, launch.extra_dir) else {
            return Err(AgentError::Failed(OrchestratorError(format!(
                "{} cannot run in a run the app starts: the app does not offer it yet",
                h.kind
            ))));
        };
        let program = match &self.bin {
            Some(bin) => format!("{bin}/{}", how.program),
            None => how.program.to_string(),
        };
        let dir = run.dir();
        let connection = Connection::spawn(
            &program,
            &how.args,
            &how.env,
            run.work_dir(),
            &format!("{dir}/{role}.acp.log"),
            format!("{dir}/{USAGE_FILE}"),
        )?;
        let ask = |method: &str, params: Value| {
            connection.request(method, params, Duration::from_secs(REQUEST_SECONDS)).map_err(|e| failed(h, method, e))
        };
        let init = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
        });
        let capabilities = ask("initialize", init)?.get("agentCapabilities").cloned().unwrap_or(Value::Null);
        let cwd = run.work_dir();
        let (session, opened) = match launch.session.filter(|s| !s.is_empty()) {
            Some(session) => (session.to_string(), reopen(h, &connection, &capabilities, session, cwd)?),
            None => {
                let opened = ask("session/new", json!({"cwd": cwd, "mcpServers": []}))?;
                let session = opened.get("sessionId").and_then(Value::as_str).ok_or_else(|| {
                    AgentError::Failed(OrchestratorError(format!("{} opened a session with no sessionId", h.kind)))
                })?;
                (session.to_string(), opened)
            }
        };
        let mut unpassed = vec![];
        match how.mode {
            ModeSetting::Set(id) if offers(&opened, "modes", "availableModes", "id", id) => {
                if opened.pointer("/modes/currentModeId").and_then(Value::as_str) != Some(id) {
                    ask("session/set_mode", json!({"sessionId": session, "modeId": id}))?;
                }
            }
            ModeSetting::Launched => {}
            ModeSetting::Set(_) | ModeSetting::NoEquivalent => unpassed.extend(named.map(|m| h.without_mode(m))),
        }
        if let Some(model) = how.model {
            if offers(&opened, "models", "availableModels", "modelId", &model) {
                ask("session/set_model", json!({"sessionId": session, "modelId": model}))?;
            } else {
                unpassed.push(format!("{} offers no model {model}; starting it on its own", h.kind));
            }
        }
        Ok((Agent { connection: Arc::new(connection), session, harness: h.kind }, unpassed))
    }
}

impl Agents for AcpAgents<'_> {
    fn bind(&self, run: &mut RunState) {
        run.backend = Backend::Acp;
    }

    /// Nothing outlives an orchestrator, so there is no place to keep: a run resumes with no agent. Errors: run's
    /// agents run in another backend.
    fn open(&self, run: &mut RunState, human: &dyn Human) -> Result<bool, AgentError> {
        if run.backend != Backend::Acp {
            return Err(AgentError::Failed(OrchestratorError(format!(
                "run {} runs its agents in herdr, not over ACP; resume it from the command line",
                run.run_id
            ))));
        }
        human.log(&format!("run {} runs its agents over ACP", run.run_id));
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
    /// still running, is killed first.
    fn start(&self, run: &RunState, role: &str, launch: &AgentLaunch) -> Result<Started, AgentError> {
        if let Some(old) = self.lock().remove(role) {
            old.connection.kill();
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

/// Where run's USAGE_FILE is.
pub fn usage_file(run: &RunState) -> String {
    format!("{}/{USAGE_FILE}", run.dir())
}

/// Open session again in connection's agent of harness h, which answered `initialize` with capabilities, working in
/// cwd: loaded where it can load one, else resumed where it can. Returns the agent's answer, as `session/new`'s.
/// Errors: Refused when it can do neither, or refuses; Failed when it does not answer.
fn reopen(
    h: &Harness,
    connection: &Connection,
    capabilities: &Value,
    session: &str,
    cwd: &str,
) -> Result<Value, AgentError> {
    let method = if capabilities.get("loadSession").and_then(Value::as_bool) == Some(true) {
        "session/load"
    } else if capabilities.pointer("/sessionCapabilities/resume").is_some_and(|r| !r.is_null() && r != false) {
        "session/resume"
    } else {
        return Err(AgentError::Refused(OrchestratorError(format!(
            "{} can neither load nor resume a session over ACP",
            h.kind
        ))));
    };
    let params = json!({"sessionId": session, "cwd": cwd, "mcpServers": []});
    match connection.request(method, params, Duration::from_secs(REQUEST_SECONDS)) {
        Ok(opened) => Ok(opened),
        Err(Failure::Refused(e)) => Err(AgentError::Refused(OrchestratorError(format!("{} {method}: {e}", h.kind)))),
        Err(e) => Err(failed(h, method, e)),
    }
}

/// Whether the session the agent opened, as its answer opened has it, offers the choice of id among
/// `<group>.<list>[].<key>`: a mode (`modes.availableModes[].id`) or a model (`models.availableModels[].modelId`).
fn offers(opened: &Value, group: &str, list: &str, key: &str, id: &str) -> bool {
    let choices = opened.get(group).and_then(|g| g.get(list)).and_then(Value::as_array);
    choices.is_some_and(|choices| choices.iter().any(|c| c.get(key).and_then(Value::as_str) == Some(id)))
}

/// The error of harness h's agent's request of method that got no result.
fn failed(h: &Harness, method: &str, failure: Failure) -> AgentError {
    let why = match failure {
        Failure::Refused(e) => e,
        Failure::Exited => "it exited".to_string(),
        Failure::TimedOut => format!("no answer within {REQUEST_SECONDS}s"),
        Failure::Unwritten(e) => e.0,
    };
    AgentError::Failed(OrchestratorError(format!("{} {method}: {why}", h.kind)))
}

/// The error of a role that has no agent.
fn no_agent(role: &str) -> AgentError {
    AgentError::Failed(OrchestratorError(format!("the {role} role has no ACP agent")))
}

#[cfg(test)]
mod tests;
