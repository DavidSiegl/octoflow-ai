//! herdr as the agent backend: every Herdr is an Agents, and every Workspaces a RunPlaces. A run's agents run in a
//! herdr workspace of its own, one pane per role, laid out as next_record says; the run's state keeps the workspace,
//! its root pane, the server it is in, and each agent's herdr name and pane.

use super::{workspace_label, Agent, Direction, Herdr, HerdrError, Workspaces};
use crate::agents::{Activity, AgentError, AgentLaunch, Agents, RunPlaces, Seen, Started};
use crate::error::OrchestratorError;
use crate::harness::harness;
use crate::human::Human;
use crate::state::{AgentRecord, RunState};

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

    /// The run's workspace is kept only in the server the run saved, and only when that is the one this Herdr reaches
    /// (keeps_saved_workspace); a new one is opened in the server reached, which the run then saves.
    fn open(&self, run: &mut RunState, human: &dyn Human) -> Result<bool, AgentError> {
        if keeps_saved_workspace(self, run, human)? {
            human.log(&format!("run {} resumes in herdr workspace {}", run.run_id, run.workspace_id));
            // A run that saved no server, from before runs saved one, saves the one it is now found in.
            if run.herdr_server.is_none() && self.server().is_some() {
                run.herdr_server = self.server();
                return Ok(true);
            }
            return Ok(false);
        }
        let label = workspace_label(&run.run_id, &run.task);
        let (workspace, root) = self.create_workspace(run.work_dir(), &label)?;
        (run.workspace_id, run.root_pane) = (workspace, root);
        run.herdr_server = self.server();
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
        wait_for(self, agent_name(run, role), seconds, &["idle", "done"], true)
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
        wait_for(self, agent_name(run, role), seconds, &["working", "blocked"], false)
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

/// Whether the run resumes in the workspace it saved: true when that workspace still exists in the herdr server the run
/// saved, and that server is the one herdr reaches (Herdr::reaches); a run that saved no server looks in the one herdr
/// reaches. False for a run with no workspace yet. When false for a run that had one, its agents' panes are cleared, as
/// a new workspace is to be opened, and a workspace still open in another server is closed there, since the engine
/// could give its panes no command; a close that fails is only logged. Errors: herdr cannot be asked whether the
/// server is reached or the workspace exists.
fn keeps_saved_workspace<H: Herdr + ?Sized>(
    herdr: &H,
    run: &mut RunState,
    human: &dyn Human,
) -> Result<bool, HerdrError> {
    if run.workspace_id.is_empty() {
        return Ok(false);
    }
    let saved = run.herdr_server.as_ref();
    let elsewhere = match saved {
        Some(saved) => !herdr.reaches(saved)?,
        None => false,
    };
    let exists = herdr.workspace_exists(&run.workspace_id, saved)?;
    if exists && !elsewhere {
        return Ok(true);
    }
    if exists {
        human.log(&format!(
            "herdr workspace {} is in another herdr server than this one; closing it there and opening a new one here",
            run.workspace_id
        ));
        // A workspace left open is only a stray one; the run goes on without it.
        if let Err(e) = herdr.close_workspace(&run.workspace_id, saved) {
            human.log(&format!("cannot close herdr workspace {}: {e}", run.workspace_id));
        }
    } else {
        human.log(&format!("herdr workspace {} is gone; opening a new one", run.workspace_id));
    }
    // The handoff files are the whole contract between roles, so a new workspace loses nothing a role needs; its panes
    // went with the old one.
    for agent in run.agents.values_mut() {
        agent.pane.clear();
    }
    Ok(false)
}

/// Wait at most seconds for herdr's agent name to show one of statuses: true once it does, false when the time runs out
/// first, and exited when the agent has exited (herdr's wait on one fails with agent_not_found).
fn wait_for<H: Herdr + ?Sized>(
    herdr: &H,
    name: &str,
    seconds: u64,
    statuses: &[&str],
    exited: bool,
) -> Result<bool, AgentError> {
    match herdr.wait(name, Some(seconds * 1000), statuses) {
        Ok(_) => Ok(true),
        Err(e) if e.code() == Some("agent_not_found") => Ok(exited),
        Err(e) if e.code() == Some("timeout") => Ok(false),
        Err(e) => Err(e.into()),
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
