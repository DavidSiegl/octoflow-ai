//! ServeHost, a Host that serve's threads can share, its files in memory; and FakeConsole, the herdr
//! its runs' agents are in.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::OrchestratorError;
use crate::herdr::{AgentAddress, AgentConsole, HerdrError, HerdrServer, Workspaces};
use crate::host::fake::saved_runs;
use crate::host::{Exec, ExecError, Host, RunFile, RunFileText, RunScope};
use crate::serve::Reached;
use crate::state::SavedRun;

/// A Host whose files are `files`, whose saved runs are those files as the real Host would give them
/// (host/fake/saved_runs.rs), its run files listed by their `mtimes`, and which records the path of every read,
/// read_inside's included, in `reads`. It is this machine's, runs no command, and none of its
/// files is a link, so every one stays inside its run directory.
#[derive(Default)]
pub struct ServeHost {
    state: Mutex<Files>,
}

/// What a ServeHost holds, changed by the tests while a server reads it.
#[derive(Default)]
pub struct Files {
    /// By path, each file's text.
    pub files: BTreeMap<String, String>,
    /// By path, a file's mtime; 0 for a file not here.
    pub mtimes: BTreeMap<String, i64>,
    /// By run id, the seconds since its state.json was written; 0 for a run not here.
    pub ages: BTreeMap<String, u64>,
    /// The path of each read, in order.
    pub reads: Vec<String>,
    /// While set, what run_states_inside fails with.
    pub run_states_inside_error: Option<String>,
    /// How many times run_states_inside was called: a poll of the project each, or a close or forget sweep's first
    /// read of its runs.
    pub run_states_inside_calls: usize,
    /// How many times run_files_inside was called: one listing of a run's directory each.
    pub run_files_inside_calls: usize,
    /// While set, what every read fails with (still recorded in reads).
    pub read_error: Option<String>,
}

impl ServeHost {
    /// Its files, to read or change.
    pub fn state(&self) -> MutexGuard<'_, Files> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The runs under the project at cwd as run_states_inside reads them, only the one named only when given; fails
    /// with run_states_inside_error while it is set.
    fn runs_inside(&self, cwd: &str, only: Option<&str>) -> Result<Vec<SavedRun>, OrchestratorError> {
        let s = self.state();
        if let Some(e) = &s.run_states_inside_error {
            return Err(OrchestratorError(e.clone()));
        }
        let age = |dir: &str| s.ages.get(dir).copied().unwrap_or(0);
        Ok(saved_runs::run_states(&s.files, age, cwd, RunScope::Inside, only))
    }
}

impl Host for ServeHost {
    fn ssh_target(&self) -> Option<&str> {
        None
    }

    fn exec(&self, argv: &[&str], _stdin: Option<&[u8]>, _timeout: u64) -> Result<Exec, ExecError> {
        panic!("ServeHost runs no command, asked for {argv:?}")
    }

    fn read(&self, path: &str) -> Result<Option<String>, OrchestratorError> {
        let mut s = self.state();
        s.reads.push(path.to_string());
        if let Some(e) = &s.read_error {
            return Err(OrchestratorError(e.clone()));
        }
        Ok(s.files.get(path).cloned())
    }

    fn run_states_inside(&self, cwd: &str) -> Result<Vec<SavedRun>, OrchestratorError> {
        self.state().run_states_inside_calls += 1;
        self.runs_inside(cwd, None)
    }

    fn run_state_inside(&self, cwd: &str, run_id: &str) -> Result<Option<SavedRun>, OrchestratorError> {
        Ok(self.runs_inside(cwd, Some(run_id))?.into_iter().next())
    }

    fn read_inside(&self, run_dir: &str, name: &str) -> Result<RunFileText, OrchestratorError> {
        saved_runs::read_inside(self, run_dir, name)
    }

    fn run_files_inside(&self, run_dir: &str) -> Result<Vec<RunFile>, OrchestratorError> {
        let mut s = self.state();
        s.run_files_inside_calls += 1;
        Ok(saved_runs::run_files(&s.files, |path| s.mtimes.get(path).copied().unwrap_or(0), run_dir))
    }
}

/// An AgentConsole whose agents show `screens`, by herdr name, whatever server they are addressed in, and which
/// records what is typed into them in `typed`, and the server of each in `servers`. An agent without a screen has
/// exited (herdr's agent_not_found). As Workspaces, its open workspaces are `workspaces`, and it records each call in
/// `workspace_calls`.
#[derive(Default)]
pub struct FakeConsole {
    state: Mutex<Consoled>,
}

/// What a FakeConsole holds and records.
#[derive(Default)]
pub struct Consoled {
    /// By agent name, what its terminal shows.
    pub screens: BTreeMap<String, String>,
    /// Each command, as `read NAME LINES`, `submit NAME TEXT` or `keys NAME KEY...`, in order.
    pub typed: Vec<String>,
    /// The server each command of `typed` addressed its agent in, in the same order (AgentAddress::server).
    pub servers: Vec<Option<HerdrServer>>,
    /// While true, every command fails as herdr does when it cannot be reached (still recorded in `typed`).
    pub down: bool,
    /// The herdr workspaces still open, by id, whatever server they are asked about in; closing one removes it.
    pub workspaces: BTreeSet<String>,
    /// Each workspace call, (`exists W` or `close W`, the server it was addressed to), in order.
    pub workspace_calls: Vec<(String, Option<HerdrServer>)>,
}

/// The project /proj on host, its agents in console.
pub fn reached(host: &Arc<ServeHost>, console: &Arc<FakeConsole>) -> Reached {
    let dir = "/proj".to_string();
    Reached { host: host.clone(), dir, console: console.clone(), places: console.clone() }
}

impl FakeConsole {
    /// What the console holds and has recorded, to set its screens or `down` and to read `typed`; the console waits
    /// for its commands until the guard is dropped.
    pub fn state(&self) -> MutexGuard<'_, Consoled> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The agent's screen, after recording command and the agent's server; herdr's errors as the type says.
    fn answer(&self, agent: &AgentAddress, command: String) -> Result<String, HerdrError> {
        let mut s = self.state();
        s.typed.push(command);
        s.servers.push(agent.server.clone());
        if s.down {
            return Err(HerdrError::Unreached(OrchestratorError::new("herdr is not on PATH")));
        }
        let name = &agent.name;
        s.screens.get(name).cloned().ok_or_else(|| HerdrError::new("agent_not_found", format!("no agent {name}")))
    }
}

impl AgentConsole for FakeConsole {
    fn read(&self, agent: &AgentAddress, lines: u32) -> Result<String, HerdrError> {
        self.answer(agent, format!("read {} {lines}", agent.name))
    }

    fn submit(&self, agent: &AgentAddress, text: &str) -> Result<(), HerdrError> {
        self.answer(agent, format!("submit {} {text}", agent.name)).map(drop)
    }

    fn send_keys(&self, agent: &AgentAddress, keys: &[&str]) -> Result<(), HerdrError> {
        self.answer(agent, format!("keys {} {}", agent.name, keys.join(" "))).map(drop)
    }
}

impl Workspaces for FakeConsole {
    fn workspace_exists(&self, workspace: &str, server: Option<&HerdrServer>) -> Result<bool, HerdrError> {
        let mut s = self.state();
        s.workspace_calls.push((format!("exists {workspace}"), server.cloned()));
        Ok(s.workspaces.contains(workspace))
    }

    fn close_workspace(&self, workspace: &str, server: Option<&HerdrServer>) -> Result<(), HerdrError> {
        let mut s = self.state();
        s.workspace_calls.push((format!("close {workspace}"), server.cloned()));
        s.workspaces.remove(workspace);
        Ok(())
    }
}
