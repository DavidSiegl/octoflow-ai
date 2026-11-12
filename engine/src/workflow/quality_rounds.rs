//! The quality gate's rounds (docs/quality-gate.md): after each turn of the quality-gated step, a quality round has
//! the run's QualityGate judge a snapshot of the change, a Jenkins job or the local gate (crate::local_gate), adds what
//! the orchestrator's own checks find, and sends a change the gate does not pass back to that step, up to
//! max_quality_rounds analyses per round; what each verdict step hears of them; and the snapshots' refs on origin,
//! deleted once they are of no more use.
//!
//! A round names no gate: QualityGate::start and its Judging hide which gate judges, and how (crate::ci::judging says
//! what a resume of each picks up).

use std::cell::RefCell;
use std::rc::Rc;

use super::origin::reaching_origin;
use super::step_prompt::RunView;
use super::{Clocks, Human, Outcome, Stopped, Workflow};
use crate::checks::{judge_run, orchestrator_checks, RunChange, RunFindings, SLOW_STEP_MARGIN};
use crate::ci::{CIError, Judging, Poll, QualityGate, Started, Verdict};
use crate::constants::{CI_POLL_SECONDS, CI_REF_PREFIX, QUALITY_TIMEOUT, REMOTE};
use crate::error::OrchestratorError;
use crate::host::{Host, LeaksFound};
use crate::interrupt::Interrupt;
use crate::patch::added_lines;
use crate::paths::file_name;
use crate::pipeline::{Step, QUALITY};
use crate::prompts::Prompt;
use serde_json::Value;
use crate::json::{object, Object};
use crate::quality::{gate_failures, leak_report, parse_gate, under_gate_line, GateFailures, Judgement, QUALITY_OK};
use crate::state::RunState;

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
        if self.quality_gate.is_none() || s.quality_round == 0 {
            return Ok(true);
        }
        // With the gate on, the quality loop has written the current quality round's file by now; one missing fails.
        let last = self.view().last_quality(n)?;
        Ok(last.is_some_and(|(q, text)| q == s.quality_round && parse_gate(&text) == Some(QUALITY_OK)))
    }

    /// The run's quality gate. Errors: the run has none, which only a quality round asks it for.
    fn gate(&self) -> Result<&'a QualityGate, OrchestratorError> {
        self.quality_gate.ok_or_else(|| OrchestratorError("a quality round in a run without the quality gate".into()))
    }

    /// Quality round q of round n: have the run's gate judge a snapshot of the change (judged_snapshot), write
    /// quality-<n>-<q>.md, record what failed it in RunState::quality_rounds, and return its gate.
    ///
    /// The gate and its failures are read from the gate's quality.json, with the orchestrator's checks added
    /// (RunFindings::added_to), and recorded in RunState::quality_rounds, replacing the record of a quality file
    /// deleted to be judged again. They are read from the quality file instead (parse_gate, gate_failures) for a round
    /// with no quality.json: one whose Jenkins build archived none, the fallback for a quality job that writes only
    /// quality.md (TODO.md says when it goes); one the gate could not judge or whose snapshot held a secret, whose file
    /// only the engine writes; and one whose file an earlier orchestrator wrote but did not record. A quality file
    /// already written is taken as it is.
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
        self.finish_analysis(&self.state.ci_ref(n, q))?;
        Ok(gate)
    }

    /// The quality file of quality round q of round n, masked (QualityGate::mask), and the judgement of the gate's
    /// quality.json with the orchestrator's checks added; no judgement for a file without one. The run's gate judges a
    /// snapshot of the change (QualityGate::start), asked after every CI_POLL_SECONDS with the heartbeat kept, and is
    /// stopped when the round's deadline, QUALITY_TIMEOUT seconds from now, passes or the run is interrupted first. A
    /// snapshot gitleaks finds a secret in is not judged, and its file is QUALITY_LEAK; one the gate could not judge
    /// gets the BUILD_FAILED file the gate gives.
    ///
    /// Errors: QualityGate::start's and Judging::verdict's, one that may pass only once the deadline passes; a secret
    /// is in the run's base or below it, which is the human's to rewrite; the checks or saving fail; a takeover or the
    /// Ctrl-C.
    fn judged_snapshot(&mut self, n: u32, q: u32) -> Outcome<(String, Option<Judgement>)> {
        let gate = self.gate()?;
        let (text, judged) = self.unmasked_judgement(gate, n, q)?;
        let judged = judged.map(|mut judged| {
            // The conditions go into state.json, which the quality file's masking does not reach.
            judged.failures.conditions.iter_mut().for_each(|c| *c = gate.mask(c));
            judged
        });
        Ok((gate.mask(&text), judged))
    }

    /// judged_snapshot's quality file and judgement by gate, unmasked.
    fn unmasked_judgement(&mut self, gate: &QualityGate, n: u32, q: u32) -> Outcome<(String, Option<Judgement>)> {
        let deadline = self.clocks.monotonic() + QUALITY_TIMEOUT as f64;
        let git_ref = self.state.ci_ref(n, q);
        // A snapshot branch of an earlier round, left on origin by a run stopped before that round ended.
        if let Some(old) = self.state.ci.get("ref").and_then(Value::as_str).filter(|r| !r.is_empty() && *r != git_ref) {
            delete_ci_ref(self.host, self.human, &self.state.cwd, old);
            self.state.ci = Object::new();
        }
        let mut judging = match gate.start(self.host, &mut self.state, n, q)? {
            Started::Judging(judging) => judging,
            Started::Leaked { found, snapshot, so_what } => {
                let base = self.state.base.clone().unwrap_or_default();
                let found = leaks_in_snapshot(found, &snapshot, &base)?;
                return Ok((self.leak_report(n, q, &found, so_what), None));
            }
        };
        self.save()?;
        let waited = self.poll(&mut judging, deadline);
        if waited.is_err() {
            let host = self.host;
            // Run as a finally block, so that a Ctrl-C still pending lets the stop start.
            if let Err(e) = self.interrupt.finally(|| judging.stop(host)) {
                self.human.log(&format!("[quality] could not stop the quality gate: {e}"));
            }
        }
        let (quality, judged, judged_by) = match waited? {
            Verdict::NotJudged(text) => return Ok((text, None)),
            Verdict::Judged { quality, judged, judged_by } => (quality, judged, judged_by),
        };
        let found = self.orchestrator_findings(judging.snapshot(), deadline)?;
        let text = under_gate_line(&found.added_to_file(&quality), &judged_by);
        Ok((text, judged.map(|judged| found.added_to(judged))))
    }

    /// Ask judging for its verdict every CI_POLL_SECONDS until it gives one, and return that, keeping the heartbeat
    /// between the calls; a call that recorded a link of the chain in RunState::ci is saved and asked again at once.
    ///
    /// A failure that may pass, such as Jenkins being unreachable, is retried until the round's deadline, a
    /// monotonic time; any other fails the run at once. Either way RunState::ci keeps what is known so far. Errors:
    /// one that does not pass, named after what it waited for (Judging::waiting), or a refusal as it is; no verdict by
    /// the deadline, naming the last failure; saving fails; a takeover or the Ctrl-C.
    fn poll(&mut self, judging: &mut Judging, deadline: f64) -> Outcome<Verdict> {
        // What the last call waited for and how it failed, while the calls fail in a way that may pass.
        let mut failing: Option<(String, CIError)> = None;
        loop {
            let human = self.human;
            let polled = judging.verdict(self.host, &mut self.state.ci, &|line| human.log(line));
            let waiting = judging.waiting().to_string();
            // A call goes on to the next link once one answers, so the link that failed last may have answered now.
            let answered = |failing: Option<(String, CIError)>| {
                if let Some((waited, _)) = failing {
                    human.log(&format!("[quality] {waited}: answered again"));
                }
            };
            match polled {
                Ok(poll) => {
                    answered(failing.take());
                    match poll {
                        Poll::Came(verdict) => return Ok(verdict),
                        Poll::Recorded => {
                            self.save()?;
                            continue;
                        }
                        Poll::Waiting => {}
                    }
                }
                Err(CIError::Refused(e)) => return Err(Stopped::Failed(e)),
                Err(e) if !e.transient() => {
                    return Err(Stopped::Failed(OrchestratorError(format!("quality gate: {waiting}: {e}"))));
                }
                Err(e) => {
                    if failing.as_ref().is_none_or(|(waited, _)| *waited != waiting) {
                        answered(failing.take());
                        self.human.log(&format!("[quality] {waiting}: {e}; retrying"));
                    }
                    failing = Some((waiting.clone(), e));
                }
            }
            if self.clocks.monotonic() >= deadline {
                let why = failing.map(|(_, e)| format!("; last error: {e}")).unwrap_or_default();
                return Err(Stopped::Failed(OrchestratorError(format!(
                    "quality gate: {waiting} did not finish within {QUALITY_TIMEOUT}s of the round's start{why}; resume \
                     to go on"
                ))));
            }
            self.heartbeat()?;
            self.pause(CI_POLL_SECONDS)?;
        }
    }

    /// The quality file of quality round q of round n, whose snapshot was not judged for the secrets found; so_what
    /// says what then did not happen, as a clause: "the gate did not judge the change".
    fn leak_report(&self, n: u32, q: u32, found: &LeaksFound, so_what: &str) -> String {
        let intro = format!(
            "{} in the snapshot of {}, so {so_what}. Take each secret \
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
        let base = self
            .state
            .base
            .clone()
            .ok_or_else(|| Stopped::Failed(OrchestratorError("a quality round before the run has a base".into())))?;
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

    /// After the analysis of git_ref: delete the ref on origin and forget it, as its build no longer needs it.
    fn finish_analysis(&mut self, git_ref: &str) -> Outcome<()> {
        if self.state.ci.get("ref").and_then(Value::as_str) == Some(git_ref) {
            // Jenkins has checked the ref out by now, though the build may still run the stages after its Gate stage
            // (ci::Judging); the Jenkinsfile's Checkout comment says why that is enough. A leftover is only clutter,
            // which the run's end tries again.
            delete_ci_ref(self.host, self.human, &self.state.cwd, git_ref);
            self.state.ci = Object::new();
        }
        self.save()
    }

    /// At the end of a run that may have refs on origin (RunState::may_have_ci_refs), delete them, again for one whose
    /// gate resume turned off, in case deleting them then failed; a failure is only logged.
    ///
    /// Errors: the Ctrl-C, raised meanwhile, which keeps RunState::ci, as Python's KeyboardInterrupt in
    /// delete_ci_refs does, for a resume to find the build.
    pub(super) fn clean_up_quality(&mut self) -> Outcome<()> {
        if self.state.may_have_ci_refs() {
            let ci = self.state.ci.clone();
            delete_ci_refs(self.host, self.human, self.clocks, self.interrupt, &mut self.state);
            if self.interrupt.is_raised() {
                self.state.ci = ci;
                return Err(Stopped::Interrupted);
            }
        }
        Ok(())
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

/// found, the secrets gitleaks found in snapshot sha and the commits a push of it would publish, when all are in sha
/// itself, for the Builder to take out. Errors: one is in another commit, so in base or below it, as sha's only
/// parent is base: the human's history, which the Builder cannot rewrite.
fn leaks_in_snapshot(found: LeaksFound, sha: &str, base: &str) -> Result<LeaksFound, Stopped> {
    if found.leaks.iter().all(|leak| leak.commit == sha) {
        return Ok(found);
    }
    let (snapshot, base): (String, String) = (sha.chars().take(12).collect(), base.chars().take(12).collect());
    Err(Stopped::Failed(OrchestratorError(format!(
        "{found}\nThose not in snapshot {snapshot} are in the run's base {base} or below it, which {REMOTE} does not \
         have: the human's commits, not the Builder's to fix. Rewrite them without the secret, or list a finding that \
         is not one in .gitleaksignore there, and start a new run."
    ))))
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

/// Delete the run's quality snapshot refs on origin and forget its build in flight (RunState::ci); a failure is only
/// logged, to human. Listing the refs waits out a short outage of origin on clocks, as reaching_origin does, unless
/// interrupt is raised.
pub(super) fn delete_ci_refs(
    host: &dyn Host,
    human: &dyn Human,
    clocks: &dyn Clocks,
    interrupt: &Interrupt,
    state: &mut RunState,
) {
    let prefix = format!("{CI_REF_PREFIX}{}-", state.key());
    let listed = reaching_origin(clocks, human, interrupt, || host.remote_branches(&state.cwd, &prefix));
    let refs = listed.unwrap_or_else(|e| {
        human.log(&format!("could not list the {CI_REF_PREFIX} branches on {REMOTE}: {e}"));
        Vec::new()
    });
    for git_ref in refs {
        delete_ci_ref(host, human, &state.cwd, &git_ref);
    }
    state.ci = Object::new();
}

/// Delete the branch git_ref on cwd's origin; a failure is only logged, to human.
fn delete_ci_ref(host: &dyn Host, human: &dyn Human, cwd: &str, git_ref: &str) {
    if let Err(e) = host.delete_remote_branch(cwd, git_ref) {
        human.log(&format!("could not delete {git_ref} on {REMOTE}: {e}"));
    }
}
