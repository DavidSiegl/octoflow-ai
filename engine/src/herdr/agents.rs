//! herdr as the agent backend: every Herdr is an Agents, and every Workspaces a RunPlaces. A run's agents run in a
//! herdr workspace of its own, one pane per role, laid out as next_record says; the run's state keeps the workspace,
//! its root pane, the server it is in, and each agent's herdr name and pane.

use super::{workspace_label, Agent, Direction, Herdr, HerdrError, Workspaces};
use crate::agents::{Activity, AgentError, AgentLaunch, Agents, RunPlaces, Seen, Started};
use crate::error::OrchestratorError;
use crate::harness::harness;
use crate::state::{AgentRecord, RunState};
use crate::workflow::Human;

impl From<HerdrError> for AgentError {
    /// herdr's own errors, with a code, are refusals; the rest are failures.
    fn from(e: HerdrError) -> AgentError {
        match e {
            HerdrError::Herdr { .. } => AgentError::Refused(e.into()),
            HerdrError::Failed(e) => AgentError::Failed(e),
        }
    }
}

impl<H: Herdr + ?Sized> Agents for H {
    fn bind(&self, run: &mut RunState) {
        run.herdr_server = self.server();
    }

    fn open(&self, run: &mut RunState, human: &dyn Human) -> Result<bool, AgentError> {
        if !run.workspace_id.is_empty() {
            if self.workspace_exists(&run.workspace_id, None)? {
                human.log(&format!("run {} resumes in herdr workspace {}", run.run_id, run.workspace_id));
                return Ok(false);
            }
            // The handoff files are the whole contract between roles, so a new workspace loses nothing a role
            // needs; its panes went with the old one.
            human.log(&format!("herdr workspace {} is gone; opening a new one", run.workspace_id));
            for agent in run.agents.values_mut() {
                agent.pane.clear();
            }
        }
        let label = workspace_label(&run.run_id, &run.task);
        let (workspace, root) = self.create_workspace(run.work_dir(), &label)?;
        (run.workspace_id, run.root_pane) = (workspace, root);
        human.log(&format!("run {} in herdr workspace {}", run.run_id, run.workspace_id));
        Ok(true)
    }

    fn close(&self, run: &RunState, human: &dyn Human) {
        if let Err(e) = self.end(run) {
            human.log(&format!("could not close herdr workspace {}: {e}", run.workspace_id));
        }
    }

    /// The layout: the first role of order in the root pane, the second to the right of it, each further one below
    /// the one before. A role whose neighbour's pane is gone is split off any pane of the run that is left.
    fn next_record(
        &self,
        run: &RunState,
        role: &str,
        order: &[String],
        harness: &str,
    ) -> Result<AgentRecord, AgentError> {
        let name = format!("{role}-{}", run.key());
        let i = order.iter().position(|r| r == role).unwrap_or(0);
        let pane_of = |r: &str| run.agents.get(r).map(|a| a.pane.clone()).filter(|p| !p.is_empty());
        let old = pane_of(role).unwrap_or_else(|| if i == 0 { run.root_pane.clone() } else { String::new() });
        if !old.is_empty() && self.pane_exists(&old)? {
            return Ok(AgentRecord::new(&name, &old, harness));
        }
        let (parent, direction) = if i >= 2 {
            (pane_of(&order[i - 1]).unwrap_or_default(), Direction::Down)
        } else {
            (run.root_pane.clone(), Direction::Right)
        };
        let mut survivors = vec![parent, run.root_pane.clone()];
        survivors.extend(run.agents.values().map(|a| a.pane.clone()));
        let mut tried = vec![];
        for pane in survivors {
            if pane.is_empty() || pane == old || tried.contains(&pane) {
                continue;
            }
            if self.pane_exists(&pane)? {
                let split = self.split(&pane, direction, run.work_dir())?;
                return Ok(AgentRecord::new(&name, &split, harness));
            }
            tried.push(pane);
        }
        Err(AgentError::Failed(OrchestratorError(format!(
            "no pane of run {} is left in herdr workspace {}",
            run.run_id, run.workspace_id
        ))))
    }

    /// herdr's `agent start` passes the agent arguments only, so the environment the harness's launch needs is
    /// exported in the pane's shell first.
    fn start(&self, run: &RunState, role: &str, launch: &AgentLaunch) -> Result<Started, AgentError> {
        let (name, pane) = (agent_name(run, role), pane(run, role));
        let h = harness(launch.harness)?;
        self.rename_pane(pane, launch.label)?;
        let args = h.launch(launch.permission_mode, launch.model, launch.session, launch.extra_dir);
        if !args.env.is_empty() {
            self.export(pane, &args.env)?;
        }
        match self.start_agent(name, pane, h.kind, &args.args)? {
            true => Ok(Started::Running),
            false => Ok(Started::Asking),
        }
    }

    fn wait_ready(&self, run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError> {
        match self.wait(agent_name(run, role), Some(seconds * 1000), &["idle", "done"]) {
            Ok(_) => Ok(true),
            Err(e) if e.code() == Some("agent_not_found") => Ok(true),
            Err(e) if e.code() == Some("timeout") => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn find(&self, run: &RunState, role: &str) -> Result<Option<Seen>, AgentError> {
        Ok(self.agent(agent_name(run, role))?.map(seen))
    }

    fn prompt(&self, run: &RunState, role: &str, text: &str) -> Result<(), AgentError> {
        Ok(Herdr::prompt(self, agent_name(run, role), text)?)
    }

    /// Taken once herdr reports the agent working or blocked; idle and done do not count, since an agent shows them
    /// before taking a prompt too.
    fn wait_taken(&self, run: &RunState, role: &str, seconds: u64) -> Result<bool, AgentError> {
        match self.wait(agent_name(run, role), Some(seconds * 1000), &["working", "blocked"]) {
            Ok(_) => Ok(true),
            // herdr's wait on an agent that has exited fails with agent_not_found.
            Err(e) if matches!(e.code(), Some("timeout" | "agent_not_found")) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn show(&self, run: &RunState, role: &str) -> Result<(), AgentError> {
        Ok(self.focus(agent_name(run, role))?)
    }

    fn whereabouts(&self, run: &RunState, role: &str) -> String {
        format!("pane {}", pane(run, role))
    }
}

impl<W: Workspaces + ?Sized> RunPlaces for W {
    /// The workspace in the herdr server the run saved.
    fn place(&self, run: &RunState) -> Result<Option<String>, AgentError> {
        let workspace = run.workspace_id.as_str();
        if workspace.is_empty() || !self.workspace_exists(workspace, run.herdr_server.as_ref())? {
            return Ok(None);
        }
        Ok(Some(format!("workspace {workspace}")))
    }

    fn end(&self, run: &RunState) -> Result<(), AgentError> {
        if run.workspace_id.is_empty() {
            return Ok(());
        }
        Ok(self.close_workspace(&run.workspace_id, run.herdr_server.as_ref())?)
    }
}

/// The herdr name of the role's agent; "" before it has one.
fn agent_name<'r>(run: &'r RunState, role: &str) -> &'r str {
    run.agents.get(role).map_or("", |a| &a.name)
}

/// The pane of the role's agent; "" before it has one, or once its workspace was found gone.
fn pane<'r>(run: &'r RunState, role: &str) -> &'r str {
    run.agents.get(role).map_or("", |a| &a.pane)
}

/// herdr's record of an agent as the backend reports it: idle and done are both Idle.
fn seen(agent: Agent) -> Seen {
    let activity = match agent.status.as_str() {
        "working" => Activity::Working,
        "blocked" => Activity::Blocked,
        "idle" | "done" => Activity::Idle,
        _ => Activity::Unknown,
    };
    Seen { activity, session: agent.session.filter(|s| !s.is_empty()) }
}
