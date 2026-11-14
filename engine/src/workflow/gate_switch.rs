//! resume's --quality-gate and --no-quality-gate: the quality gate of a stopped run turned on or off before a Workflow
//! resumes it, its state changed in memory for the Workflow to save; turning it off also stops the gate's job a killed
//! run left (turn_quality_gate_off).

use super::turn::turn_started;
use super::{Collaborators, Human};
use crate::error::OrchestratorError;
use crate::host::Host;
use crate::local_gate::LOCAL_GATE;
use crate::paths::file_name;
use crate::pipeline::{Pipeline, Step, DONE, PUBLISH, QUALITY};
use crate::state::RunState;

/// Turn the quality gate of the run in state, of workflow pipeline, on, as resume --quality-gate does; a run that has
/// it already is left as it is. human hears what was done.
///
/// host must pass check_gate_mid_run. A run in the step after the gated one, whose turn has not started, goes to
/// quality round 1 first; any other starts the gate at the gated step's next turn.
///
/// Errors, with state unchanged: the run is in phase done or publish (even with the gate on), had its gate turned
/// off, is of a workflow without a quality-gated step, or is an in-place --no-pr run past round 0,
/// whose working tree may hold the human's own changes that a snapshot would take; a check of round 1 on fails; or the
/// next step's file cannot be read.
pub fn turn_quality_gate_on(
    state: &mut RunState,
    pipeline: &Pipeline,
    host: &dyn Host,
    human: &dyn Human,
) -> Result<(), OrchestratorError> {
    let s = state;
    let Some(gated) = gate_can_start(s, pipeline)? else {
        human.log(&format!("run {} already has quality gate {LOCAL_GATE}", s.run_id));
        return Ok(());
    };
    check_gate_mid_run(s, host)?;
    // Whether the run goes to quality round 1 first: read before state changes, so that a failed read leaves it as it was.
    let mut judge_first = false;
    if let Some(following) = pipeline.after(&gated.id).filter(|f| f.id == s.phase) {
        let path = following.path(&s.dir(), s.round, 0);
        let file = file_name(&path);
        let started = turn_started(s, file);
        // Read whether or not the turn started: a failed read stops the resume either way.
        let missing = host.read(&format!("{}/{file}", s.dir()))?.is_none();
        judge_first = missing && !started;
    }
    s.quality_job = Some(LOCAL_GATE.to_string());
    if judge_first {
        s.phase = QUALITY.to_string();
        s.quality_round = 1;
    }
    human.log(&format!("turned the quality gate of run {} on, with {LOCAL_GATE}", s.run_id));
    Ok(())
}

/// Check that host can run the local gate for the run in state, which gets it on resume, before any turn: from round 1
/// on, gitleaks runs and local_gate::preflight passes. Round 0 is left to the Workflow's own preflight before the
/// interview, which checks the same, as for run --quality-gate.
///
/// Errors: either check fails.
pub fn check_gate_mid_run(state: &RunState, host: &dyn Host) -> Result<(), OrchestratorError> {
    if state.round > 0 {
        host.check_gitleaks()?;
        crate::local_gate::preflight(host)?;
    }
    Ok(())
}

/// The quality-gated step of pipeline, if the run in state may turn the gate on; None when it has the gate already,
/// which needs nothing done; an error saying why not otherwise (turn_quality_gate_on).
fn gate_can_start<'p>(state: &RunState, pipeline: &'p Pipeline) -> Result<Option<&'p Step>, OrchestratorError> {
    let s = state;
    let why = if s.phase == DONE || s.phase == PUBLISH {
        format!("is in phase {}, past its last turn", s.phase)
    } else if s.quality_job.as_deref().is_some_and(|j| !j.is_empty()) {
        return Ok(None);
    } else if s.quality_gate_disabled {
        "had its quality gate turned off with --no-quality-gate, which cannot be undone".to_string()
    } else if let Some(gated) = pipeline.gated() {
        if s.round > 0 && !s.pull_request && s.worktree.is_none() {
            format!(
                "is a --no-pr run in {} itself, whose working tree was never required to be clean, and a quality \
                 snapshot would take your own changes too",
                s.cwd
            )
        } else {
            return Ok(Some(gated));
        }
    } else {
        format!("is of workflow {}, which has no quality-gated step", pipeline.name())
    };
    Err(OrchestratorError(format!("run {} {why}, so its quality gate cannot be turned on", s.run_id)))
}

/// Turn the quality gate of the run in state, of workflow pipeline, off for good, as resume --no-quality-gate does,
/// on with's host; with's human hears what was done.
///
/// A run in phase quality goes on at the step after the gated one, dropping an analysis that
/// has not finished; the verdict step then hears of the round's quality files as the quality note says (Workflow::quality_note).
/// The jobs of that round's gate left running, as by a run killed while its gate judged, are stopped
/// (local_gate::stop_left); a failure to is logged and does not stop the turn.
///
/// Errors, with state unchanged: the run has no quality gate, or is done.
pub fn turn_quality_gate_off(
    state: &mut RunState,
    pipeline: &Pipeline,
    with: &Collaborators,
) -> Result<(), OrchestratorError> {
    let s = state;
    if s.phase == DONE || s.quality_job.as_deref().is_none_or(str::is_empty) {
        let why = if s.phase == DONE { "is done" } else { "has no quality gate" };
        return Err(OrchestratorError(format!("run {} {why}, so --no-quality-gate does not apply", s.run_id)));
    }
    s.quality_job = None;
    s.quality_gate_disabled = true;
    if s.phase == QUALITY {
        let gate_dir = s.gate_dir(s.round, s.quality_round);
        if let Err(e) = crate::local_gate::stop_left(with.host, s.work_dir(), &gate_dir) {
            with.human.log(&format!("could not stop the quality gate's job: {e}"));
        }
        if let Some(gated) = pipeline.gated() {
            // An analysis that has not finished has an open record, which no quality file will ever close.
            s.turns.retain(|t| t.step != QUALITY || !t.under_way());
            s.phase = pipeline.after(&gated.id).map_or(PUBLISH, |f| f.id.as_str()).to_string();
        }
    }
    with.human.log(&format!("turned the quality gate of run {} off", s.run_id));
    Ok(())
}
