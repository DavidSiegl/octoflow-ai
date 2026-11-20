//! The agent backend: where a run's role agents run and how the run reaches them, behind one interface, so that the
//! workflow, `close` and serve name roles and runs, never a backend's own terms. [`Agents`] is what a run's
//! orchestrator drives its agents through; [`RunPlaces`] is what anything else does to a run's agents from its saved
//! state; the human is told through crate::human. herdr is the one backend so far: every `crate::herdr::Herdr` is an `Agents`, and
//! every `crate::herdr::Workspaces` a `RunPlaces` (crate::herdr::agents).
//!
//! A backend keeps what it needs in the run's state (RunState), in fields only it writes; for herdr those are
//! `herdr_server`, `workspace_id`, `root_pane`, and each AgentRecord's `name` and `pane`. Two readers outside the
//! backend remain: list and show print `workspace_id` and `pane` (crate::cli::listing), and serve reaches a run's
//! agents through `herdr_server`, its AgentConsole being herdr's own (crate::serve::runs).
//!
//! Every method's AgentError is Failed when the backend cannot be reached or fails, unless its comment says when it
//! is Refused.

use std::fmt;

use crate::error::OrchestratorError;
use crate::human::Human;
use crate::state::{AgentRecord, RunState};

/// Why a backend did not do what it was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    /// The backend answered and refused, as herdr does with an error code of its own (an agent or session it does not
    /// have, say), so that another request, such as a fresh session in place of a resumed one, may still succeed.
    Refused(OrchestratorError),
    /// The backend could not be asked, or what was asked could not be put to it.
    Failed(OrchestratorError),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Refused(e) | AgentError::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<AgentError> for OrchestratorError {
    fn from(e: AgentError) -> OrchestratorError {
        match e {
            AgentError::Refused(e) | AgentError::Failed(e) => e,
        }
    }
}

impl From<OrchestratorError> for AgentError {
    fn from(e: OrchestratorError) -> AgentError {
        AgentError::Failed(e)
    }
}

/// What a role's agent is doing, as its backend sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Working on a prompt.
    Working,
    /// Waiting on the human: a question, a permission, a dialog.
    Blocked,
    /// Doing nothing: done with its last prompt, or not yet given one.
    Idle,
    /// Something else the backend reports, which counts neither as working nor as idle.
    Unknown,
}

/// A role's agent that is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// What it was doing when the backend was asked.
    pub activity: Activity,
    /// The id of the harness's session it runs in; None while the backend does not know it.
    pub session: Option<String>,
}

/// How a role's agent is to start (Agents::start).
#[derive(Clone, Copy, Debug)]
pub struct AgentLaunch<'l> {
    /// The role's label, as the human sees it.
    pub label: &'l str,
    /// The harness it runs in, one of crate::harness::HARNESS_NAMES.
    pub harness: &'l str,
    /// The run's --permission-mode, in Claude Code's names; None (or empty) for the harness's own default.
    pub permission_mode: Option<&'l str>,
    /// The model; None (or empty) for the harness's own.
    pub model: Option<&'l str>,
    /// The harness's session to resume; None (or empty) for a new one.
    pub session: Option<&'l str>,
    /// A directory outside its working directory that it may write to; None for none.
    pub extra_dir: Option<&'l str>,
}

/// How a role's agent came out of Agents::start. It may still exit at once, which Agents::find tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    /// Whether it is asking the human something before it takes a prompt, such as Claude Code whether to trust a
    /// folder it has not run in (Agents::wait_ready).
    pub asking: bool,
    /// What of the launch it started without, each as a sentence about its harness for the log: a permission mode the
    /// harness has no equivalent of, a model it cannot be given.
    pub unpassed: Vec<String>,
}

/// A run's agents as the orchestrator that drives the run reaches them: their place, starting them, prompting them,
/// and watching them. Every method takes the run's state and the role, never a backend's own names, and changes only
/// the backend's fields of the state (see the module); saving the state is the caller's.
pub trait Agents: RunPlaces {
    /// Record in a new run, with its first state, where its agents will run (herdr: the server it reaches); a resume
    /// leaves that to Agents::open.
    fn bind(&self, run: &mut RunState);

    /// Have a place for the run's agents: the one run saved, when it is still there and this backend can reach it,
    /// else a new one, every agent's place in the old one forgotten, and run records where the place is (herdr: the
    /// run's workspace and its server). Tells human which, in a log line. Returns whether run changed, to be saved.
    /// Errors: run's agents run in another backend (RunState::backend); the backend cannot tell whether the saved place
    /// is there, or cannot open a new one.
    fn open(&self, run: &mut RunState, human: &dyn Human) -> Result<bool, AgentError>;

    /// RunPlaces::end for the orchestrator, once the run is over: a failure is only logged to human, in the
    /// backend's words, since it leaves clutter and nothing the run needs.
    fn close(&self, run: &RunState, human: &dyn Human);

    /// The record of the role's next agent, which the caller fills in and saves before Agents::start starts it: in
    /// harness, and placed where it will run (herdr: its name and pane, the role's old pane if it survives, else a new
    /// one laid out by order). order is the run's roles in the order the human sees them, role among them.
    /// Errors: no place for it is left; the backend fails.
    fn next_record(
        &self,
        run: &RunState,
        role: &str,
        order: &[String],
        harness: &str,
    ) -> Result<AgentRecord, AgentError>;

    /// Start the role's agent, whose record (Agents::next_record's) run already holds, as launch says. Returns once it
    /// has started, before any prompt. Errors: Refused when the backend will not start it as asked, as with a session
    /// it cannot resume.
    fn start(&self, run: &RunState, role: &str, launch: &AgentLaunch) -> Result<Started, AgentError>;

    /// Wait at most seconds for the role's agent to stop waiting on the human (Started::asking, Activity::Blocked), as
    /// the human answers; true once it has, or has exited, false when the time runs out first.
    fn wait_ready(&self, run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError>;

    /// The role's agent as it runs now; None when it has exited, or the role has none.
    fn find(&self, run: &RunState, role: &str) -> Result<Option<Seen>, AgentError>;

    /// Send the role's agent text as a prompt, without waiting for it to be taken. Errors: Failed for text the backend
    /// cannot send (herdr: one that starts like a long option).
    fn prompt(&self, run: &RunState, role: &str, text: &str) -> Result<(), AgentError>;

    /// Wait at most seconds for the role's agent to take the prompt just sent; false when the time runs out, or it
    /// has exited, first.
    fn wait_taken(&self, run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError>;

    /// Bring the role's agent before the human (herdr: focus its pane).
    fn show(&self, run: &RunState, role: &str) -> Result<(), AgentError>;

    /// Where the human finds the role's agent, as a message names it (herdr: `pane w1:p2`).
    fn whereabouts(&self, run: &RunState, role: &str) -> String;
}

/// A run's agents as anything but its orchestrator reaches them, from the run's saved state: close, and serve's.
pub trait RunPlaces {
    /// The run's place, as a message names it (herdr: `workspace w1`), when it is still there; None when the run never
    /// had one, or it is gone.
    fn place(&self, run: &RunState) -> Result<Option<String>, AgentError>;

    /// Close the run's place, with any agent still in it; nothing to do when the run never had one.
    fn end(&self, run: &RunState) -> Result<(), AgentError>;
}
