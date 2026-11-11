//! A quality gate's judgement of one snapshot of a run's change (QualityGate::start): the local gate's job, or the
//! chain of a Jenkins build, which the quality round polls for its verdict without knowing which it is.
//!
//! A Jenkins judgement saves every link of its chain (the snapshot pushed, the queue item, the build) in RunState::ci
//! as soon as it learns it, so that a start on a resume continues from the furthest one. The local gate keeps no
//! chain: a resume judges a new snapshot.

use super::{CIError, Jenkins};
use crate::error::OrchestratorError;
use crate::host::{Host, LeaksFound};
use crate::json::{object, Object};
use crate::local_gate::{self, LocalVerdict};
use crate::quality::{build_failed_report, could_not_judge_report, parse_gate, read_report, Judgement};
use serde_json::Value;

/// What a call to Judging::verdict came to, when it did not fail.
pub enum Poll {
    /// The gate still judges; ask again after a while.
    Waiting,
    /// The call recorded a link of the chain in RunState::ci, which the caller saves before it asks again, at once.
    Recorded,
    /// What the gate came to.
    Came(Verdict),
}

/// What QualityGate::start came to.
pub enum Started<'g> {
    /// The gate is judging the snapshot.
    Judging(Judging<'g>),
    /// gitleaks found secrets in what the gate would have judged, so it judges nothing: nothing was pushed or started,
    /// and RunState::ci is left empty.
    Leaked {
        /// What gitleaks found, in snapshot or below it.
        found: LeaksFound,
        /// The snapshot scanned.
        snapshot: String,
        /// What did not happen for it, as a clause of the round's quality file: "the local gate did not judge the
        /// change".
        so_what: &'static str,
    },
}

/// What a gate came to on a snapshot (Judging::verdict).
pub enum Verdict {
    /// The gate judged it.
    Judged {
        /// The gate's quality.md, unmasked.
        quality: String,
        /// The judgement its quality.json holds, which agrees with quality's GATE line; None for a Jenkins job that
        /// archives no quality.json.
        judged: Option<Judgement>,
        /// The line that says who judged which snapshot, for under the quality file's GATE line (under_gate_line).
        judged_by: String,
    },
    /// Nothing judged it: the whole quality file, a BUILD_FAILED one saying why, unmasked.
    NotJudged(String),
}

/// A gate judging one snapshot (QualityGate::start).
pub struct Judging<'g> {
    /// The snapshot judged.
    snapshot: String,
    /// The round, as a quality file's intro names it (RunState::quality_place).
    place: String,
    /// What the judgement waits for, as the errors and log name it: "the local gate", "waiting for Jenkins to start
    /// the build of <ref>".
    waiting: String,
    by: By<'g>,
}

/// Who judges, and what that judge's judgement keeps between polls.
enum By<'g> {
    Jenkins(Chain<'g>),
    Local(local_gate::Judging),
}

/// The chain of a Jenkins build of a snapshot pushed to origin, beyond what RunState::ci keeps of it.
struct Chain<'g> {
    jenkins: &'g Jenkins,
    /// The branch on origin the snapshot was pushed to, which the build checks out.
    git_ref: String,
    /// True while the chain may have a build an earlier orchestrator triggered and did not record: it was started on
    /// a resume, and has not looked for one yet.
    adopt: bool,
    /// The last reads of quality.md and quality.json while the build ran; None before one found quality.md.
    earlier: Option<Reports>,
    /// What gate_or_end read of the build once it archived quality.md or ended; None before.
    ended: Option<Ended>,
    /// The failing tests of a build that ended before its Gate stage, as Jenkins::failed_tests reads them, once read.
    tests: Option<Option<Vec<(String, String)>>>,
}

/// One read of a build's reports: its quality.md, and its quality.json, None when Jenkins served none.
type Reports = (String, Option<String>);

/// A build's result, None for one still running the stages after its Gate stage, and its reports, None for one that
/// archived no quality.md, as read once it archived quality.md or ended.
type Ended = (Option<String>, Option<Reports>);

impl<'g> Judging<'g> {
    /// A judgement by jenkins of snapshot, pushed to git_ref; adopt as Chain::adopt says.
    pub(super) fn jenkins(
        jenkins: &'g Jenkins,
        snapshot: &str,
        git_ref: &str,
        place: &str,
        adopt: bool,
    ) -> Judging<'g> {
        let chain = Chain { jenkins, git_ref: git_ref.into(), adopt, earlier: None, ended: None, tests: None };
        Judging::new(snapshot, place, By::Jenkins(chain))
    }

    /// The local gate's judgement of snapshot, as job.
    pub(super) fn local(job: local_gate::Judging, snapshot: &str, place: &str) -> Judging<'g> {
        Judging::new(snapshot, place, By::Local(job))
    }

    fn new(snapshot: &str, place: &str, by: By<'g>) -> Judging<'g> {
        let waiting = match by {
            By::Jenkins(_) => String::new(),
            By::Local(_) => "the local gate".to_string(),
        };
        Judging { snapshot: snapshot.into(), place: place.into(), waiting, by }
    }

    /// The commit the gate judges.
    pub fn snapshot(&self) -> &str {
        &self.snapshot
    }

    /// What the last call to verdict waited for or failed on, as the round's errors and log name it.
    pub fn waiting(&self) -> &str {
        &self.waiting
    }

    /// Ask the gate how far it has come. Each call goes along the judgement until it must wait, has a verdict, or
    /// has recorded one link of the chain, and a call after one that failed in a way that may pass goes on from where
    /// that one stopped.
    ///
    /// A Jenkins judgement records each link of its chain in ci, RunState::ci, as soon as it learns it
    /// (Poll::Recorded), and logs to log a build it adopts. A build or queue item that will give no verdict cuts ci
    /// back to the pushed snapshot, so that a start on a resume builds it anew, never adopting that build again.
    ///
    /// Errors, as CIError::Request when asking again may pass and Refused when it will not: a request to Jenkins
    /// fails; the build was aborted before its Gate stage, its queue item cancelled, or it or its queue item is gone
    /// from Jenkins; quality.json is of a schema_version the engine does not know, or not of its schema (read_report),
    /// or its gate is not quality.md's; ci holds a link of another type than a chain saves; the local gate's job
    /// failed (local_gate::Judging::verdict).
    pub fn verdict(&mut self, host: &dyn Host, ci: &mut Object, log: &dyn Fn(&str)) -> Result<Poll, CIError> {
        let (snapshot, place) = (&self.snapshot, &self.place);
        match &mut self.by {
            By::Jenkins(chain) => chain.verdict(ci, snapshot, place, &mut self.waiting, log),
            By::Local(job) => Ok(match job.verdict(host)? {
                None => Poll::Waiting,
                Some(verdict) => Poll::Came(local_verdict(verdict, snapshot, place)?),
            }),
        }
    }

    /// Stop judging, once the caller no longer waits for the verdict: the local gate's job is stopped, its tests
    /// included; a Jenkins build is left to run, as a resume may adopt it. Errors: a command or git fails.
    pub fn stop(&self, host: &dyn Host) -> Result<(), OrchestratorError> {
        match &self.by {
            By::Jenkins(_) => Ok(()),
            By::Local(job) => job.stop(host),
        }
    }
}

/// The verdict of the local gate's job, verdict, on snapshot, judged in round place. Errors: its quality.json is
/// unreadable or disagrees with its quality.md (judgement).
fn local_verdict(verdict: LocalVerdict, snapshot: &str, place: &str) -> Result<Verdict, OrchestratorError> {
    Ok(match verdict {
        LocalVerdict::Judged { release, quality, json } => {
            let judged = judgement(&quality, &json)
                .map_err(|why| OrchestratorError(format!("octoflow-gate {release} on snapshot {snapshot}: {why}")))?;
            let judged_by = format!("octoflow-gate {release}, run locally, judged snapshot `{snapshot}` ({place}).");
            Verdict::Judged { quality, judged: Some(judged), judged_by }
        }
        LocalVerdict::CouldNotJudge(said) => {
            let intro = format!(
                "octoflow-gate, run locally, could not judge snapshot `{snapshot}` ({place}), so nothing judged the \
                 change. Make it judgeable first: what the gate said is below."
            );
            Verdict::NotJudged(could_not_judge_report(&intro, &said))
        }
    })
}

impl Chain<'_> {
    /// Judging::verdict's, for a Jenkins build of snapshot, keeping waiting to what each call waits for.
    fn verdict(
        &mut self,
        ci: &mut Object,
        snapshot: &str,
        place: &str,
        waiting: &mut String,
        log: &dyn Fn(&str),
    ) -> Result<Poll, CIError> {
        let (jenkins, git_ref) = (self.jenkins, self.git_ref.clone());
        // The build: one found or triggered, then the one its queue item starts.
        if !ci.contains_key("build") && !ci.contains_key("queue_url") {
            if self.adopt {
                // The ref names one round of one run, so a build of it is this round's, triggered by an orchestrator
                // that stopped before it recorded the queue item.
                *waiting = format!("looking for a Jenkins build of {git_ref}");
                let found = jenkins.find_build(&git_ref, &rejected(ci))?;
                self.adopt = false;
                if let Some(found) = found {
                    log(&format!("[quality] adopting Jenkins build #{found} of {git_ref}"));
                    ci.insert("build".into(), Value::from(found));
                    return Ok(Poll::Recorded);
                }
            }
            *waiting = format!("triggering Jenkins job {} for {git_ref}", jenkins.job());
            ci.insert("queue_url".into(), Value::from(jenkins.trigger(&git_ref)?));
            return Ok(Poll::Recorded);
        }
        if !ci.contains_key("build") {
            *waiting = format!("waiting for Jenkins to start the build of {git_ref}");
            let Some(build) = self.queued_build(ci)? else { return Ok(Poll::Waiting) };
            ci.insert("build".into(), Value::from(build));
            return Ok(Poll::Recorded);
        }
        let number = ci_build(ci)?;
        if self.ended.is_none() {
            *waiting = format!("waiting for Jenkins build #{number} of {git_ref} to archive quality.md or end");
            let Some(ended) = self.gate_or_end(ci, number)? else { return Ok(Poll::Waiting) };
            self.ended = Some(ended);
        }
        let Some((result, reports)) = &self.ended else { unreachable!("set above") };
        let result = result.as_deref();
        if let Some((quality, json)) = reports {
            let judged = match json.as_deref().map(|json| judgement(quality, json)).transpose() {
                Ok(judged) => judged,
                Err(why) => {
                    // A build of the same snapshot again is the resume's to make, once the job is fixed.
                    cut_back(ci, Some(number));
                    let why = format!("Jenkins build #{number} of {git_ref}: {why}; resume to build it again");
                    return Err(CIError::Refused(OrchestratorError(why)));
                }
            };
            let how_it_stood = match result {
                None => "was still running when its quality.md was read".to_string(),
                Some(result) => format!("ended {result}"),
            };
            let judged_by =
                format!("Jenkins build #{number} judged snapshot `{snapshot}` ({place}) and {how_it_stood}.");
            return Ok(Poll::Came(Verdict::Judged { quality: quality.clone(), judged, judged_by }));
        }
        if result == Some("ABORTED") {
            cut_back(ci, Some(number));
            return Err(CIError::Refused(OrchestratorError(format!(
                "Jenkins build #{number} of {git_ref} was aborted before the quality gate ran, so nothing judged the \
                 change; resume to build it again"
            ))));
        }
        // The build ended before its Gate stage: what failed it, its failing tests or else the end of its console.
        if self.tests.is_none() {
            *waiting = format!("reading the test report of Jenkins build #{number}");
            self.tests = Some(jenkins.failed_tests(number)?);
        }
        let tests = self.tests.as_ref().and_then(Option::as_deref);
        let console = if tests.is_some_and(|t| !t.is_empty()) {
            String::new()
        } else {
            *waiting = format!("reading the console of Jenkins build #{number}");
            jenkins.console_tail(number)?
        };
        let intro = format!(
            "Jenkins build #{number} of snapshot `{snapshot}` ({place}) ended {} before the quality gate ran, so \
             nothing judged the change. Make it build first.",
            result.unwrap_or("None")
        );
        Ok(Poll::Came(Verdict::NotJudged(build_failed_report(&intro, tests, &console))))
    }

    /// The number of the build ci's queue item started; None while it waits. A queue item Jenkins forgot is found
    /// among the job's builds instead.
    ///
    /// Errors: a failed request; refused, with ci cut back, for an item cancelled or forgotten without a build, or a
    /// queue_url that is not a string.
    fn queued_build(&self, ci: &mut Object) -> Result<Option<i64>, CIError> {
        let (jenkins, git_ref) = (self.jenkins, &self.git_ref);
        let queue_url = ci_text(ci, "queue_url")?;
        let item = match jenkins.queue_item(&queue_url) {
            Ok(item) => item,
            // Jenkins forgets a queue item some minutes after its build started.
            Err(e) if e.status() == Some(404) => {
                return match jenkins.find_build(git_ref, &rejected(ci))? {
                    Some(found) => Ok(Some(found)),
                    None => {
                        cut_back(ci, None);
                        Err(CIError::Refused(OrchestratorError(format!(
                            "Jenkins forgot queue item {queue_url}, and none of the job's last 20 builds is of \
                             {git_ref}; resume to trigger a new one"
                        ))))
                    }
                };
            }
            Err(e) => return Err(e),
        };
        if item.cancelled {
            cut_back(ci, None);
            return Err(CIError::Refused(OrchestratorError(format!(
                "the build of {git_ref} was cancelled in Jenkins's queue, so nothing judged the change; resume to \
                 trigger a new one"
            ))));
        }
        Ok(item.build)
    }

    /// The build's result and reports once it has archived its quality file or has ended, whichever comes first;
    /// None before. Errors: a failed request; refused, with ci cut back, for a build Jenkins no longer has.
    fn gate_or_end(&mut self, ci: &mut Object, number: i64) -> Result<Option<Ended>, CIError> {
        let jenkins = self.jenkins;
        let result = match jenkins.build_result(number) {
            // The Jenkinsfile's buildDiscarder drops the job's oldest builds, which a run stopped mid-round and resumed
            // much later can still name.
            Err(e) if e.status() == Some(404) => {
                cut_back(ci, None);
                return Err(CIError::Refused(OrchestratorError(format!(
                    "Jenkins build #{number} of {} is gone, discarded by the job; resume to build it again",
                    self.git_ref
                ))));
            }
            answer => answer?,
        };
        let quality = match jenkins.quality_file(number)? {
            Some(text) => Some((text, jenkins.quality_json(number)?)),
            None => None,
        };
        if result.is_some() {
            return Ok(Some((result, quality)));
        }
        // Jenkins serves an artifact while archiveArtifacts is still copying it into place, so one read of a running
        // build's file may be cut short, or find quality.json not yet copied. Two reads of both that agree, at least a
        // poll interval apart, with a quality.md that holds a whole non-blank line, as the GATE line is, are taken as
        // whole: so an empty copy or one cut inside its first line is not, while a whole first line that is no GATE
        // line is, for the round to report. A heuristic, which a copy stalled that long and then resumed defeats
        // (TODO.md). A build that has ended has finished archiving, so one read then is enough.
        if quality.as_ref().is_some_and(|both| self.earlier.as_ref() == Some(both) && holds_a_whole_line(&both.0)) {
            return Ok(Some((None, quality)));
        }
        self.earlier = quality;
        Ok(None)
    }
}

/// Keep only the pushed snapshot in ci, RunState::ci, so that a resume builds it anew, never adopting a build
/// rejected earlier or the rejected one given.
fn cut_back(ci: &mut Object, rejected: Option<i64>) {
    let mut skip = ci.get("rejected").and_then(Value::as_array).cloned().unwrap_or_default();
    skip.extend(rejected.map(Value::from));
    let field = |key: &str| ci.get(key).cloned().unwrap_or(Value::Null);
    let mut cut = object([("ref", field("ref")), ("sha", field("sha"))]);
    if !skip.is_empty() {
        cut.insert("rejected".into(), Value::Array(skip));
    }
    *ci = cut;
}

/// ci's field key, a string. Errors: it is missing or not a string, as no engine saves it.
pub(super) fn ci_text(ci: &Object, key: &str) -> Result<String, OrchestratorError> {
    match ci.get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        other => Err(ci_error(key, other)),
    }
}

/// ci's build number. Errors: it is not an integer, as no engine saves it.
fn ci_build(ci: &Object) -> Result<i64, OrchestratorError> {
    let build = ci.get("build");
    build.and_then(Value::as_i64).ok_or_else(|| ci_error("build", build))
}

/// The builds ci records as rejected, which a resume never adopts; an entry that is no build number is none.
fn rejected(ci: &Object) -> Vec<i64> {
    let rejected = ci.get("rejected").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    rejected.iter().filter_map(Value::as_i64).collect()
}

/// The error of a RunState::ci field, key, that is missing or of another type than the engines save.
fn ci_error(key: &str, value: Option<&Value>) -> OrchestratorError {
    let value = value.map_or("nothing".to_string(), Value::to_string);
    OrchestratorError(format!("state.json's ci holds {value} as its {key}, which no orchestrator saves"))
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

/// Whether text holds a whole non-blank line: one that a newline ends.
fn holds_a_whole_line(text: &str) -> bool {
    let mut lines: Vec<&str> = text.split('\n').collect();
    lines.pop();
    lines.iter().any(|line| !line.trim().is_empty())
}
