//! resume's --quality-gate and --no-quality-gate: the quality gate of a stopped run turned on or off before a Workflow
//! resumes it, its state changed in memory for the Workflow to save. Python's turn_quality_gate_on and _off.

use super::quality_rounds::delete_ci_refs;
use super::{Collaborators, Human};
use crate::ci::QualityGate;
use crate::error::OrchestratorError;
use crate::host::Host;
use crate::paths::file_name;
use crate::pipeline::{Pipeline, Step, DONE, PUBLISH, QUALITY};
use crate::state::RunState;

/// Turn the quality gate of the run in state, of workflow pipeline, on with gate, a Jenkins job or the local gate, as
/// resume --quality-gate does; the run's own gate again changes nothing. human hears what was done.
///
/// From round 1 on, the gate must pass its preflight (QualityGate::preflight); round 0 is left to the Workflow's own
/// preflight, as for run --quality-gate. A run in the step after the gated one, whose turn has not started, goes to
/// quality round 1 first; any other starts the gate at the gated step's next turn.
///
/// Errors, with state unchanged: the run is in phase done or publish (even with its own gate), had its gate turned
/// off, has another gate, is of a workflow without a quality-gated step, or is an in-place --no-pr run past round 0,
/// whose working tree may hold the human's own changes that a snapshot would take; a check of round 1 on fails; or the
/// next step's file cannot be read.
pub fn turn_quality_gate_on(
    state: &mut RunState,
    pipeline: &Pipeline,
    host: &dyn Host,
    human: &dyn Human,
    gate: &QualityGate,
) -> Result<(), OrchestratorError> {
    let s = state;
    let Some(gated) = gate_can_start(s, pipeline, gate.name())? else {
        human.log(&format!("run {} already has quality gate {}", s.run_id, gate.name()));
        return Ok(());
    };
    if s.round > 0 {
        gate.preflight(host, &s.cwd)?;
    }
    // Whether the run goes to quality round 1 first: read before state changes, so that a failed read leaves it as it was.
    let mut judge_first = false;
    if let Some(following) = pipeline.after(&gated.id).filter(|f| f.id == s.phase) {
        let path = following.path(&s.dir(), s.round, 0);
        let file = file_name(&path);
        // A turn that was prompted, asked again for its verdict, or is waiting on its BLOCKED answer has seen the
        // change; only one with none of these has not.
        let blocked = s.blocked.as_ref().and_then(|b| b.get("file")).and_then(|f| f.as_str());
        let started = [s.prompted.as_deref(), s.retrying.as_deref(), blocked].contains(&Some(file));
        // Read whether or not the turn started, as Python reads it first: a failed read stops the resume either way.
        let missing = host.read(&format!("{}/{file}", s.dir()))?.is_none();
        judge_first = missing && !started;
    }
    s.quality_job = Some(gate.name().to_string());
    if judge_first {
        s.phase = QUALITY.to_string();
        s.quality_round = 1;
    }
    human.log(&format!("turned the quality gate of run {} on, with {}", s.run_id, gate.name()));
    Ok(())
}

/// The quality-gated step of pipeline, if the run in state may turn the gate named name on; None when it has that gate
/// already, which needs nothing done; an error saying why not otherwise (turn_quality_gate_on).
fn gate_can_start<'p>(state: &RunState, pipeline: &'p Pipeline, name: &str) -> Result<Option<&'p Step>, OrchestratorError> {
    let s = state;
    let why = if s.phase == DONE || s.phase == PUBLISH {
        format!("is in phase {}, past its last turn", s.phase)
    } else if s.quality_job.as_deref() == Some(name) {
        return Ok(None);
    } else if s.quality_gate_disabled {
        "had its quality gate turned off with --no-quality-gate, which cannot be undone".to_string()
    } else if let Some(other) = s.quality_job.as_deref().filter(|j| !j.is_empty()) {
        format!("has quality gate {other}, and cannot switch to {name}")
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
/// The run's refs on origin, which the local gate has none of, are deleted, a failure only logged, and a build in
/// progress is left to run. A run in phase quality goes on at the step after the gated one, dropping an analysis that
/// has not finished; the verdict step then hears of the round's quality files as the quality note says (Workflow::quality_note).
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
    if s.may_have_ci_refs() {
        delete_ci_refs(with.host, with.human, with.clocks, with.interrupt, s);
    }
    s.quality_job = None;
    s.quality_gate_disabled = true;
    if s.phase == QUALITY {
        if let Some(gated) = pipeline.gated() {
            // An analysis that has not finished has an open record, which no quality file will ever close.
            s.turns.retain(|t| t.step != QUALITY || !t.under_way());
            s.phase = pipeline.after(&gated.id).map_or(PUBLISH, |f| f.id.as_str()).to_string();
        }
    }
    with.human.log(&format!("turned the quality gate of run {} off", s.run_id));
    Ok(())
}
