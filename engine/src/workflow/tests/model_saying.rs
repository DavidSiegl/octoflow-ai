//! A fake of the run's agents whose sessions say what a test chooses of the model an agent is launched on.

use crate::agents::{AgentError, AgentLaunch, Agents, LaunchedModel, RunPlaces, Seen, Started, Waiting};
use crate::error::OrchestratorError;
use crate::harness::Usage;
use crate::herdr::fake::FakeHerdr;
use crate::host::Host;
use crate::human::Human;
use crate::state::{AgentRecord, RunState, UnofferedModel};

/// The fixture's FakeHerdr as the run's Agents, but for a role whose harness's session says of the model its agent is
/// launched on what says gives for the launch's model; a launch naming none is Given, as every backend has it.
pub(super) struct ModelSaying<'f> {
    /// The fake that plays every agent, this role's included.
    pub herdr: &'f FakeHerdr,
    /// The role whose session says what says gives.
    pub role: &'static str,
    /// What that role's session says of the model asked of it.
    pub says: &'f dyn Fn(&str) -> LaunchedModel,
}

/// What a session that does not offer the model asked of it, and runs on taken instead (None for one it does not name),
/// says of that model.
pub(super) fn unoffered(asked: &str, taken: Option<&str>) -> LaunchedModel {
    LaunchedModel::Unoffered(UnofferedModel { asked: asked.to_string(), taken: taken.map(String::from) })
}

impl RunPlaces for ModelSaying<'_> {
    fn place(&self, run: &RunState) -> Result<Option<String>, AgentError> {
        self.herdr.place(run)
    }
    fn end(&self, run: &RunState) -> Result<(), AgentError> {
        self.herdr.end(run)
    }
}

impl Agents for ModelSaying<'_> {
    fn bind(&self, run: &mut RunState) {
        self.herdr.bind(run)
    }
    fn open(&self, run: &mut RunState, human: &dyn Human) -> Result<bool, AgentError> {
        self.herdr.open(run, human)
    }
    fn close(&self, run: &RunState, human: &dyn Human) {
        self.herdr.close(run, human)
    }
    fn next_record(&self, run: &RunState, role: &str, order: &[String], kind: &str) -> Result<AgentRecord, AgentError> {
        self.herdr.next_record(run, role, order, kind)
    }
    fn start(&self, run: &RunState, role: &str, launch: &AgentLaunch) -> Result<Started, AgentError> {
        let mut started = self.herdr.start(run, role, launch)?;
        if let Some(model) = launch.model.filter(|m| role == self.role && !m.is_empty()) {
            started.model_said = (self.says)(model);
        }
        Ok(started)
    }
    fn wait_ready(&self, run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError> {
        self.herdr.wait_ready(run, role, seconds)
    }
    fn find(&self, run: &RunState, role: &str) -> Result<Option<Seen>, AgentError> {
        self.herdr.find(run, role)
    }
    fn prompt(&self, run: &RunState, role: &str, text: &str) -> Result<(), AgentError> {
        self.herdr.prompt(run, role, text)
    }
    fn wait_taken(&self, run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError> {
        self.herdr.wait_taken(run, role, seconds)
    }
    fn show(&self, run: &RunState, role: &str) -> Result<(), AgentError> {
        self.herdr.show(run, role)
    }
    fn await_human(&self, run: &RunState, role: &str, why: Waiting) {
        self.herdr.await_human(run, role, why)
    }
    fn whereabouts(&self, run: &RunState, role: &str) -> String {
        self.herdr.whereabouts(run, role)
    }
    fn settle(&self, run: &RunState) {
        self.herdr.settle(run)
    }
    fn cancel(&self, run: &RunState) {
        self.herdr.cancel(run)
    }
    fn session_usage(
        &self,
        run: &RunState,
        host: &dyn Host,
        kind: &str,
        session: &str,
    ) -> Result<Option<Vec<Usage>>, OrchestratorError> {
        self.herdr.session_usage(run, host, kind, session)
    }
    fn waited_seconds(&self, run: &RunState, role: &str, from: i64, to: i64) -> Result<Option<i64>, OrchestratorError> {
        self.herdr.waited_seconds(run, role, from, to)
    }
}
