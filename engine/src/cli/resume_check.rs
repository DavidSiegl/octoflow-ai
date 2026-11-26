//! Whether resume would take up a saved run (resume_gate), and whether it would find anything left to do there
//! (resumable): what resume checks first, and what list and serve ask of a run before offering its resume.

use super::refuse_app_run;
use crate::error::OrchestratorError;
use crate::pipeline::Pipeline;
use crate::runs::{Health, ThisMachine};
use crate::state::{RunState, SavedRun};

/// run's saved state and its workflow, as resume takes the run up before its flags apply, with its health judged
/// from machine (RunState::health); acp is resume's --acp and force its --force.
///
/// Errors: its state cannot be read (SavedRun::problem), or its saved workflow or prompts; it is the desktop app's and
/// acp is false (refuse_app_run); it was closed; or it looks alive and force is false.
pub(crate) fn resume_gate<'r>(
    run: &'r SavedRun,
    acp: bool,
    force: bool,
    machine: ThisMachine,
) -> Result<(&'r RunState, Pipeline), OrchestratorError> {
    let state = run.state()?;
    refuse_app_run(state, "resume", acp)?;
    // Not even with --force: close may have deleted its worktree and branch.
    if state.is_closed() {
        return Err(OrchestratorError(format!("run {} was closed; start a new run", state.run_id)));
    }
    let pipeline = state.pipeline()?;
    if let Some(Health::Running(why)) = run.health(machine) {
        if !force {
            return Err(OrchestratorError(format!(
                "run {} looks alive ({why}); pass --force to take it over",
                state.run_id
            )));
        }
    }
    Ok((state, pipeline))
}

/// Ok when a resume of run, without --force, would take it up (resume_gate, with the --acp its backend needs) and
/// find something left to do: a run not done, or a done one with its ending left (Workflow::ending_left), which
/// Workflow::run ends again: one whose orchestrator stopped before releasing it, or one released without a file its
/// ending writes, as far as the reader of run looked (SavedRun::lacks_ending_file). machine is resume_gate's.
///
/// Errors: why not, resume_gate's or that the run is finished.
pub fn resumable(run: &SavedRun, machine: ThisMachine) -> Result<(), OrchestratorError> {
    let acp = run.shown().app_managed();
    let (state, _) = resume_gate(run, acp, false, machine)?;
    if state.done() && state.owner.is_none() && !run.lacks_ending_file() {
        return Err(OrchestratorError(format!("run {} is finished; nothing is left to resume", state.run_id)));
    }
    Ok(())
}
