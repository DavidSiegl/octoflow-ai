//! What a step's turn is prompted with (RunView::step_prompt), read from a run without driving it: its state, its
//! workflow, its prompt texts, and the files its turns and quality rounds wrote. Workflow asks the same view what the
//! run's records say of its quality rounds, proofs and locked files (RunView's other methods, beside the phase they
//! belong to: quality_rounds.rs, proof_runs.rs, locks.rs).

use crate::constants::{APPROVE, CHANGES_REQUESTED};
use crate::error::OrchestratorError;
use crate::host::Host;
use crate::map::Map;
use crate::pipeline::{Pipeline, Step};
use crate::prompts::template::fill;
use crate::prompts::{prompt_error, Prompt, Prompts};
use crate::state::RunState;

/// A run as its prompts read it. Reading only: nothing through it changes the state or writes a file.
#[derive(Clone, Copy)]
pub(crate) struct RunView<'a> {
    pub state: &'a RunState,
    pub pipeline: &'a Pipeline,
    /// Every prompt the workflow sends that its definition does not word: the follow-ups and notes.
    pub prompts: &'a Prompts,
    /// Used only to read the run's files: a missing file reads as None.
    pub host: &'a dyn Host,
}

impl RunView<'_> {
    /// The step's prompt for its turn in round n, and the one for a fresh session that has not seen the turns before
    /// it (None when the two are the same: the step's first turn). The quality round is the state's, for the
    /// quality-gated step; 0 for every other. Each ends with BLOCKED_NOTE when the step can block.
    ///
    /// Errors: a file cannot be read, a saved record is malformed, or a template cannot be filled in (prompt_error).
    pub(crate) fn step_prompt(&self, step: &Step, n: u32) -> Result<(String, Option<String>), OrchestratorError> {
        let (text, fresh) = self.as_worded(step, n)?;
        if !step.can_block() {
            return Ok((text, fresh));
        }
        // Formatted with nothing, as each prompt is a template (Prompts): an override may write a brace as {{.
        let note = self.prompts.fill(Prompt::BlockedNote, &[])?;
        Ok((format!("{text}\n\n{note}"), fresh.map(|f| if f.is_empty() { f } else { format!("{f}\n\n{note}") })))
    }

    /// step_prompt as the step's definition words it, without BLOCKED_NOTE.
    fn as_worded(&self, step: &Step, n: u32) -> Result<(String, Option<String>), OrchestratorError> {
        let s = self.state;
        let q = if step.quality_gated { s.quality_round } else { 0 };
        let mut values = Map::new();
        for name in step.placeholders() {
            let value = self.placeholder(&name, step, n)?;
            values.insert(name, value);
        }
        let template = |t: &str| fill(t, &|name| values.get(name).cloned()).map_err(|e| prompt_error(&step.id, &e));
        // What a verdict step hears of its round besides its prompt: its last quality file and the locked files
        // restored, and the proof step the proofs run again.
        let mut notes = String::new();
        if step.loop_to.is_some() {
            notes = self.quality_note(n)? + &self.locks_note(n)?;
        }
        if self.pipeline.proves(step) {
            notes += &self.carried_proofs_note(n)?;
        }
        let first = template(&step.prompt)? + &notes;
        let follow = if q > 0 {
            let report_path = self.own_path(step, n);
            let quality_path = s.quality_path(n, q);
            self.prompts.fill(Prompt::QualityFixPrompt, &[("quality_path", &quality_path), ("report_path", &report_path)])?
        } else if n > 1 && self.judged_rounds(step, n).is_none_or(|rounds| !rounds.is_empty()) {
            // A later round, for a step that has taken a turn before: a verdict step that earlier ones' requests for
            // changes have skipped so far gets its first prompt.
            template(step.again.as_deref().filter(|a| !a.is_empty()).unwrap_or(&step.prompt))? + &notes
        } else {
            return Ok((first, None));
        };
        let mut fresh = vec![first];
        if let Some(note) = step.fresh_note.as_deref().filter(|f| !f.is_empty()) {
            fresh.push(template(note)?);
        }
        // A quality fix always asks for work that the first prompt does not.
        if q > 0 || step.fresh_repeats_again {
            fresh.push(follow.clone());
        }
        // Last, so a fresh session hears it too, whatever its prompt repeats.
        let proofs = if q > 0 { String::new() } else { self.proofs_note(step, n)? };
        Ok((follow + &proofs, Some(fresh.join("\n\n") + &proofs)))
    }

    /// The value of a prompt placeholder (see Step) in the step's turn in round n.
    fn placeholder(&self, name: &str, step: &Step, n: u32) -> Result<String, OrchestratorError> {
        let s = self.state;
        match name {
            "task" => return Ok(s.task.clone()),
            "cwd" => return Ok(s.work_dir().to_string()),
            "n" => return Ok(n.to_string()),
            "approve" => return Ok(APPROVE.to_string()),
            "changes" => return Ok(CHANGES_REQUESTED.to_string()),
            "proofs_dir" => return Ok(s.proofs_dir(n)),
            "change" => return Ok(s.change_description()),
            "path" => return Ok(self.own_path(step, n)),
            "findings" => return Ok(self.findings(n)),
            _ => {}
        }
        let placeholders = self.pipeline.path_placeholders();
        let (_, prefix, other) = placeholders
            .iter()
            .find(|(x, _, _)| x == name)
            .ok_or_else(|| OrchestratorError(format!("step {} has no placeholder {name}", step.id)))?;
        match *prefix {
            // Round 0's previous round is -1, as Python's n - 1 gives it, whose file no turn writes.
            "prev_" if n == 0 => Ok(other.path(&s.dir(), -1, 0)),
            "prev_" => match self.judged_rounds(other, n) {
                // A verdict step's last file before round n; round n-1's, which no turn wrote, before its first turn,
                // as round 0's above.
                Some(rounds) => Ok(other.path(&s.dir(), rounds.last().copied().unwrap_or(n - 1), 0)),
                None => Ok(self.reports(other, n - 1)?.pop().unwrap_or_default()),
            },
            "earlier_" => {
                let rounds = self.judged_rounds(other, n).unwrap_or_else(|| (1..n).collect());
                let mut reports = vec![];
                for i in rounds {
                    reports.extend(self.reports(other, i)?);
                }
                if other.id == step.id && step.quality_gated {
                    reports.extend((0..s.quality_round).map(|j| step.path(&s.dir(), n, j)));
                }
                Ok(reports.join(", "))
            }
            _ if other.id == step.id => Ok(self.own_path(step, n)),
            _ => Ok(other.last_path(&s.dir(), n, s.quality_round)),
        }
    }

    /// The verdict file whose request for changes sent the run into round n: the file of round n-1's last verdict
    /// (RunState::verdicts), or for a run saved without those records the proof step's file of round n-1. "" before
    /// round 2, and in a workflow without a verdict step.
    fn findings(&self, n: u32) -> String {
        let s = self.state;
        if n < 2 {
            return String::new();
        }
        let judge = s.verdicts.iter().rev().find(|v| v.round == n - 1).and_then(|v| self.pipeline.step(&v.step));
        judge.or(self.pipeline.proof_step()).map_or(String::new(), |st| st.path(&s.dir(), n - 1, 0))
    }

    /// The rounds before round n the verdict step judged, in order: those the run's records (RunState::verdicts) give
    /// it a verdict in, as an earlier verdict step's request for changes may have skipped it, and for the first
    /// verdict step every round before the run's first record too. A run saved before there were records, and resumed
    /// since, has its records start at the resumed round, and its one verdict step judged every round before that.
    /// None, every round before n, for a step that is not a verdict step.
    fn judged_rounds(&self, step: &Step, n: u32) -> Option<Vec<u32>> {
        step.loop_to.as_ref()?;
        let s = self.state;
        let first_recorded = s.verdicts.iter().map(|v| v.round).min().unwrap_or(n);
        let unrecorded = if self.pipeline.proves(step) { 1..first_recorded } else { 0..0 };
        let recorded = s.verdicts.iter().filter(|v| v.step == step.id && v.round < n).map(|v| v.round);
        Some(unrecorded.chain(recorded).collect())
    }

    /// The step's files of round n, oldest first; for the gated step, its answers to quality rounds too.
    fn reports(&self, step: &Step, n: u32) -> Result<Vec<String>, OrchestratorError> {
        let s = self.state;
        let mut reports = vec![step.path(&s.dir(), n, 0)];
        // How many quality rounds an earlier round took is recorded only by the files it left.
        while s.had_quality_gate() && step.quality_gated {
            let next = step.path(&s.dir(), n, reports.len() as u32);
            if self.host.read(&next)?.is_none() {
                break;
            }
            reports.push(next);
        }
        Ok(reports)
    }

    /// The file the step's turn in round n writes: for the gated step, its answer to the quality round if any.
    pub(crate) fn own_path(&self, step: &Step, n: u32) -> String {
        let s = self.state;
        step.path(&s.dir(), n, if step.quality_gated { s.quality_round } else { 0 })
    }

    /// Round n's last quality file that is written, as (its quality round, its text); None when there is none.
    ///
    /// It is the current quality round's, except when resume --no-quality-gate turned the gate off before that
    /// round's analysis finished, or in the gated step's turn after it (take_turn): the one before then.
    pub(crate) fn last_quality(&self, n: u32) -> Result<Option<(u32, String)>, OrchestratorError> {
        let s = self.state;
        for q in [s.quality_round, s.quality_round.saturating_sub(1)] {
            if q > 0 {
                if let Some(text) = self.host.read(&s.quality_path(n, q))? {
                    return Ok(Some((q, text)));
                }
            }
        }
        Ok(None)
    }

    /// Whether the current round's change went unjudged: resume --no-quality-gate turned the gate off, and last, the
    /// round's last quality file as last_quality gives it, is older than the Builder's latest report in the round, or
    /// is None: the gate was turned off before the round's first analysis finished, or in an earlier round.
    pub(crate) fn unjudged(&self, last: Option<&(u32, String)>) -> bool {
        let s = self.state;
        s.quality_gate_disabled && last.is_none_or(|(q, _)| *q < s.quality_round)
    }
}
