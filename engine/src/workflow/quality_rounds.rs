//! The quality gate's rounds (docs/quality-gate.md): after each turn of the quality-gated step, a quality round has
//! the local gate (crate::local_gate) judge a snapshot of the change, adds what the orchestrator's own checks find,
//! and sends a change the gate does not pass back to that step, up to max_quality_rounds analyses per round; and what
//! each verdict step hears of them.

use std::cell::RefCell;
use std::rc::Rc;

use super::step_prompt::RunView;
use super::{Outcome, Stopped, Workflow};
use crate::checks::{judge_run, orchestrator_checks, RunChange, RunFindings, SLOW_STEP_MARGIN};
use crate::constants::{CI_POLL_SECONDS, QUALITY_TIMEOUT};
use crate::error::OrchestratorError;
use crate::host::{LeaksFound, PushError, Scanned};
use crate::local_gate::{self, LocalVerdict};
use crate::patch::added_lines;
use crate::paths::file_name;
use crate::pipeline::{Step, QUALITY};
use crate::prompts::Prompt;
use serde_json::Value;
use crate::json::{object, Object};
use crate::quality::{
    could_not_judge_report, gate_failures, leak_report, parse_gate, read_report, under_gate_line, GateFailures,
    Judgement, QUALITY_OK,
};

impl<'a> Workflow<'a> {
    /// The current quality round of round n, of the quality-gated step gated; false when its gate sends the change
    /// back to gated: when it failed, and not on just what the quality round before failed on (stuck).
    ///
    /// Errors: the round's own (quality), and saving.
    pub(super) fn judge_quality(&mut self, gated: &Step, n: u32) -> Outcome<bool> {
        let q = self.state.quality_round;
        let gate = self.quality(n, q)?;
        // As for review rounds: a saved round beyond a lowered limit ends here too.
        if gate == QUALITY_OK || q >= self.state.max_quality_rounds {
            return Ok(true);
        }
        if self.view().stuck(n, q)? {
            self.human.log(&format!(
                "[quality] round {n}, quality round {q} failed on just what quality round {} failed on; going on \
                 rather than back to {}",
                q - 1,
                gated.id
            ));
            return Ok(true);
        }
        self.state.phase = gated.id.clone();
        self.save()?;
        Ok(false)
    }

    /// Whether round n's last quality round passed the gate; true for a run without the gate, or whose round had no
    /// quality round.
    pub(super) fn gate_passed(&self, n: u32) -> Result<bool, OrchestratorError> {
        let s = &self.state;
        if !self.quality_gate_on || s.quality_round == 0 {
            return Ok(true);
        }
        // With the gate on, the quality loop has written the current quality round's file by now; one missing fails.
        let last = self.view().last_quality(n)?;
        Ok(last.is_some_and(|(q, text)| q == s.quality_round && parse_gate(&text) == Some(QUALITY_OK)))
    }

    /// Quality round q of round n: have the local gate judge a snapshot of the change (judged_snapshot), write
    /// quality-<n>-<q>.md, record what failed it in RunState::quality_rounds, and return its gate.
    ///
    /// The gate and its failures are read from the gate's quality.json, with the orchestrator's checks added
    /// (RunFindings::added_to), and recorded in RunState::quality_rounds, replacing the record of a quality file
    /// deleted to be judged again. They are read from the quality file instead (parse_gate, gate_failures) for a round
    /// with no quality.json: one the gate could not judge or whose snapshot held a secret, whose file only the engine
    /// writes; and one whose file an earlier orchestrator wrote but did not record, as a Jenkins build's quality file
    /// may be, from a release before 0.72.0. A quality file already written is taken as it is.
    ///
    /// Errors: judged_snapshot's; the quality file does not start with a GATE line; a file or saving fails.
    fn quality(&mut self, n: u32, q: u32) -> Outcome<&'static str> {
        let path = self.state.quality_path(n, q);
        let file = file_name(&path).to_string();
        let (text, judged, written) = match self.host.read(&path)? {
            Some(text) => {
                self.human.log(&format!("[quality] {file} is already written"));
                (text, None, false)
            }
            None => {
                if self.open_turn(&file).is_none() {
                    let record = self.turn_record(QUALITY, &file, Some(self.host.now()?));
                    self.state.turns.push(record);
                    self.save()?;
                }
                let (text, judged) = self.judged_snapshot(n, q)?;
                self.host.write(&path, &text, false)?;
                (text, judged, true)
            }
        };
        self.end_turn(QUALITY, &path)?;
        let Some(gate) = parse_gate(&text) else { return Err(self.reject(&path, "does not start with a GATE line")) };
        let judged = judged.unwrap_or_else(|| Judgement { gate, failures: gate_failures(&text) });
        // A record of a file deleted to be judged again is the old build's.
        self.state.quality_rounds.retain(|r| !(written && is_record_of(r, n, q)));
        if self.view().quality_record(n, q).is_none() {
            self.state.quality_rounds.push(quality_record(n, q, &judged));
        }
        self.human.log(&format!("[quality] round {n}, quality round {q}: {gate}"));
        self.save()?;
        Ok(gate)
    }

    /// The quality file of quality round q of round n, and the judgement of the gate's quality.json with the
    /// orchestrator's checks added; no judgement for a file without one.
    ///
    /// The gate judges a snapshot of the change, a commit of the work dir's tree whose only parent is the run's base,
    /// once gitleaks, by the base's rules, finds no secret in what it adds to the base. It runs as a job of its own
    /// under RunState::gate_dir (local_gate::start), asked after every CI_POLL_SECONDS with the heartbeat kept, and is
    /// stopped when the round's deadline, QUALITY_TIMEOUT seconds from now, passes or the run is interrupted first; a
    /// resume judges a new snapshot. A snapshot gitleaks finds a secret in is not judged, and its file is QUALITY_LEAK;
    /// one the gate could not judge gets a BUILD_FAILED file with what the gate said.
    ///
    /// Errors: the run has no base; git or the scan fails; local_gate::start's and local_gate::Judging::verdict's; the
    /// gate's two reports disagree or its quality.json is unreadable (judgement); no verdict by the deadline; the checks
    /// or saving fail; a takeover or the Ctrl-C.
    fn judged_snapshot(&mut self, n: u32, q: u32) -> Outcome<(String, Option<Judgement>)> {
        let deadline = self.clocks.monotonic() + QUALITY_TIMEOUT as f64;
        let base = self.base()?;
        let work_dir = self.state.work_dir().to_string();
        let message = format!("Orchestrator run {}: round {n}, quality round {q}", self.state.run_id);
        let snapshot = self.host.snapshot(&work_dir, &base, &message)?;
        let leaks_path = self.state.leaks_path("snapshot");
        match self.host.refuse_leaks(&work_dir, &snapshot, &base, Scanned::SinceBase, &leaks_path) {
            Ok(()) => {}
            Err(PushError::Failed(e)) => return Err(e.into()),
            Err(PushError::Leaks(found)) => return Ok((self.leak_report(n, q, &found), None)),
        }
        let job = local_gate::start(self.host, &work_dir, &snapshot, &base, &self.state.gate_dir(n, q))?;
        let waited = self.poll(&job, deadline);
        if waited.is_err() {
            let host = self.host;
            // Run as a finally block, so that a Ctrl-C still pending lets the stop start.
            if let Err(e) = self.interrupt.finally(|| job.stop(host)) {
                self.human.log(&format!("[quality] could not stop the quality gate: {e}"));
            }
        }
        let place = self.state.quality_place(n, q);
        let (release, quality, json) = match waited? {
            LocalVerdict::Judged { release, quality, json } => (release, quality, json),
            LocalVerdict::CouldNotJudge(said) => {
                let intro = format!(
                    "octoflow-gate, run locally, could not judge snapshot `{snapshot}` ({place}), so nothing judged \
                     the change. Make it judgeable first: what the gate said is below."
                );
                return Ok((could_not_judge_report(&intro, &said), None));
            }
        };
        let judged = judgement(&quality, &json)
            .map_err(|why| OrchestratorError(format!("octoflow-gate {release} on snapshot {snapshot}: {why}")))?;
        let judged_by = format!("octoflow-gate {release}, run locally, judged snapshot `{snapshot}` ({place}).");
        let found = self.orchestrator_findings(&snapshot, deadline)?;
        let text = under_gate_line(&found.added_to_file(&quality), &judged_by);
        Ok((text, Some(found.added_to(judged))))
    }

    /// Ask job for its verdict every CI_POLL_SECONDS until it gives one, and return that, keeping the heartbeat
    /// between the calls. Errors: the job's (local_gate::Judging::verdict), at once; no verdict by deadline, a
    /// monotonic time; saving fails; a takeover or the Ctrl-C.
    fn poll(&mut self, job: &local_gate::Judging, deadline: f64) -> Outcome<LocalVerdict> {
        loop {
            if let Some(verdict) = job.verdict(self.host)? {
                return Ok(verdict);
            }
            if self.clocks.monotonic() >= deadline {
                return Err(Stopped::Failed(OrchestratorError(format!(
                    "quality gate: the local gate did not finish within {QUALITY_TIMEOUT}s of the round's start; \
                     resume to go on"
                ))));
            }
            self.heartbeat()?;
            self.pause(CI_POLL_SECONDS)?;
        }
    }

    /// The run's base. Errors: it has none yet, which a quality round never meets.
    fn base(&self) -> Result<String, OrchestratorError> {
        self.state.base.clone().ok_or_else(|| OrchestratorError("a quality round before the run has a base".into()))
    }

    /// The quality file of quality round q of round n, whose snapshot was not judged for the secrets found.
    fn leak_report(&self, n: u32, q: u32, found: &LeaksFound) -> String {
        let intro = format!(
            "{} in the snapshot of {}, so the local gate did not judge the change. Take each secret \
             below out of the change; a comment telling gitleaks to allow its line counts as one too. If one is not a \
             secret, say so in your report: only the human can let it through, and not in this run, which keeps the \
             rules of its base: listed as `file:rule:line` in .gitleaksignore on the base branch, it passes in a run \
             started after that.",
            found.found(),
            self.state.quality_place(n, q)
        );
        leak_report(&found.leaks, &intro)
    }

    /// What the orchestrator's checks find in snapshot sha (judge_run), their slow steps kept to the round's deadline,
    /// a monotonic time.
    ///
    /// The checks' in_time keeps the run's heartbeat going between their slow steps. A stop the heartbeat meets (a
    /// takeover, the Ctrl-C) lets the checks start no more slow steps, and is the error once they are done. Errors:
    /// that stop; git or a check fails.
    fn orchestrator_findings(&mut self, sha: &str, deadline: f64) -> Outcome<RunFindings> {
        let base = self.base()?;
        let pipeline = Rc::clone(&self.pipeline);
        // A workflow without a contract step has only its task to say what the change is for.
        let spec = Some(self.last_text(pipeline.contract(), 0)?).filter(|s| !s.is_empty());
        let spec = spec.unwrap_or_else(|| self.state.task.clone());
        let (host, work_dir) = (self.host, self.state.work_dir().to_string());
        let added = added_lines(&host.change_diff(&work_dir, &base, Some(sha))?)?;
        let changed = added.iter().map(|(path, lines)| (path.to_string(), lines.keys().copied().collect())).collect();
        let files = host.changed_line_counts(&work_dir, &base, sha)?;
        let file_at = |commit: &str, path: &str| host.file_at(&work_dir, commit, path);
        let now = self.clocks.wall();
        let checks = orchestrator_checks(&pipeline, self.indexes);
        let stopped: RefCell<Option<Stopped>> = RefCell::new(None);
        let me = RefCell::new(self);
        let in_time = || {
            if stopped.borrow().is_some() {
                return false;
            }
            let mut wf = me.borrow_mut();
            match wf.heartbeat() {
                Ok(()) => wf.clocks.monotonic() < deadline - SLOW_STEP_MARGIN as f64,
                Err(stop) => {
                    *stopped.borrow_mut() = Some(stop);
                    false
                }
            }
        };
        let change =
            RunChange { spec, base, commit: sha.to_string(), changed, files, file_at: &file_at, now, in_time: &in_time };
        let found = judge_run(&checks, &change);
        if let Some(stop) = stopped.take() {
            return Err(stop);
        }
        Ok(found?)
    }
}

/// What a run's records and quality files say of its quality rounds.
impl RunView<'_> {
    /// Whether quality round q of round n failed on the failures of quality round q-1 again (GateFailures::repeats):
    /// the gated step's turn between them fixed none of them. False for the first quality round, and when either
    /// round's failures are unknown (failures_of).
    pub(super) fn stuck(&self, n: u32, q: u32) -> Result<bool, OrchestratorError> {
        if q < 2 {
            return Ok(false);
        }
        Ok(match (self.failures_of(n, q - 1)?, self.failures_of(n, q)?) {
            (Some(before), Some(after)) => after.repeats(&before),
            _ => false,
        })
    }

    /// What quality round q of round n failed on: its record in RunState::quality_rounds, or what its quality file
    /// says (gate_failures) for a round judged by an engine that kept no such records; None when it has neither.
    /// Errors: the record is not of the shape quality_record writes, or the file cannot be read.
    fn failures_of(&self, n: u32, q: u32) -> Result<Option<GateFailures>, OrchestratorError> {
        if let Some(record) = self.quality_record(n, q) {
            return recorded_failures(record).map(Some);
        }
        Ok(self.host.read(&self.state.quality_path(n, q))?.map(|text| gate_failures(&text)))
    }

    /// The record of quality round q of round n in RunState::quality_rounds; None before that round is judged.
    pub(super) fn quality_record(&self, n: u32, q: u32) -> Option<&Object> {
        self.state.quality_rounds.iter().find(|r| is_record_of(r, n, q))
    }

    /// What a verdict step hears of round n's last quality file, after a blank line: QUALITY_PASSED_NOTE, or the
    /// findings left (QUALITY_STUCK_NOTE when the last quality round was stuck, else QUALITY_UNRESOLVED_NOTE), or for
    /// an unjudged change (unjudged) QUALITY_OFF_NOTE naming each of the round's quality files; "" without one.
    /// Errors: a file cannot be read, or the prompt cannot be filled in.
    pub(super) fn quality_note(&self, n: u32) -> Result<String, OrchestratorError> {
        let s = self.state;
        let Some(last) = self.last_quality(n)? else { return Ok(String::new()) };
        if self.unjudged(Some(&last)) {
            let paths: Vec<String> = (1..=last.0).map(|k| s.quality_path(n, k)).collect();
            let note = self.prompts.fill(Prompt::QualityOffNote, &[("quality_paths", &paths.join(", "))])?;
            return Ok(format!("\n\n{note}"));
        }
        let (q, text) = last;
        let note = if parse_gate(&text) == Some(QUALITY_OK) {
            Prompt::QualityPassedNote
        } else if self.stuck(n, q)? {
            Prompt::QualityStuckNote
        } else {
            Prompt::QualityUnresolvedNote
        };
        Ok(format!("\n\n{}", self.prompts.fill(note, &[("quality_path", &s.quality_path(n, q))])?))
    }
}

/// Whether record, one of RunState::quality_rounds, is quality round q of round n's.
fn is_record_of(record: &Object, n: u32, q: u32) -> bool {
    let is = |key: &str, number: u32| record.get(key) == Some(&Value::from(i64::from(number)));
    is("round", n) && is("quality_round", q)
}

/// The record of quality round q of round n, judged as judged, as RunState::quality_rounds keeps it.
fn quality_record(n: u32, q: u32, judged: &Judgement) -> Object {
    let strings = |items: &[String]| Value::Array(items.iter().map(|i| Value::from(i.as_str())).collect());
    let failures = &judged.failures;
    let fingerprints = failures.fingerprints.iter().map(|(check, found)| (check.to_string(), strings(found))).collect();
    object([
        ("round", Value::from(i64::from(n))),
        ("quality_round", Value::from(i64::from(q))),
        ("gate", Value::from(judged.gate)),
        ("conditions", strings(&failures.conditions)),
        ("fingerprints_by_check", Value::Object(fingerprints)),
    ])
}

/// The failures a quality_record holds. Errors: its conditions or fingerprints are not of the shape quality_record
/// writes, as no engine saves them.
fn recorded_failures(record: &Object) -> Result<GateFailures, OrchestratorError> {
    let shape = || {
        let record = Value::Object(record.clone());
        OrchestratorError(format!("state.json's quality_rounds holds {record}, which no orchestrator saves"))
    };
    let strings = |value: Option<&Value>| -> Option<Vec<String>> {
        value?.as_array()?.iter().map(|s| s.as_str().map(str::to_string)).collect()
    };
    let mut failures = GateFailures { conditions: strings(record.get("conditions")).ok_or_else(shape)?, ..GateFailures::default() };
    let by_check = record.get("fingerprints_by_check").and_then(Value::as_object).ok_or_else(shape)?;
    for (check, found) in by_check {
        for fingerprint in strings(Some(found)).ok_or_else(shape)? {
            failures.add(check, fingerprint);
        }
    }
    Ok(failures)
}

/// The judgement of a gate's quality.json, json, whose quality.md is quality. Errors: as read_report's, or quality.md
/// has a GATE line other than quality.json's gate; the run's other readers of the round's gate (gate_passed,
/// quality_note, the pull request) read the quality file, so the two must agree.
fn judgement(quality: &str, json: &str) -> Result<Judgement, OrchestratorError> {
    let judged = read_report(json)?;
    match parse_gate(quality) {
        Some(gate) if gate != judged.gate => Err(OrchestratorError(format!(
            "its quality.md says GATE: {gate}, but its quality.json says {}: the gate wrote two reports that disagree",
            judged.gate
        ))),
        _ => Ok(judged),
    }
}
