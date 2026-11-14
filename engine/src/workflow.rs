//! The phase machine: [`Workflow`] drives one run through the steps of its workflow in a herdr workspace, and is the
//! same code for a new run and a resumed one.
//!
//! How it is given what it talks to (decided by design-it-twice): borrowed [`Collaborators`], the
//! real ones from the CLI and the fakes from the tests, which keep the fakes and read them afterwards; the state it
//! owns, read back through [`Workflow::state`]. An environment trait the Workflow is generic over, and a sans-IO
//! machine a driver feeds, were weighed and not taken: the first needs a type per variant of a fake and forwards
//! everything, the second fits none of the tests, which script fakes rather than feed a machine events.

use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

use crate::constants::{
    APPROVE, CHANGES_REQUESTED, DEFAULT_HARNESS, DEFAULT_MAX_QUALITY_ROUNDS, DEFAULT_MAX_ROUNDS, DEFAULT_TURN_TIMEOUT,
    FINISHED, HEARTBEAT_SECONDS, QUALITY_GATE_FAILED, REMOTE, SUCCEEDED,
};
use crate::local_gate::{self, LOCAL_GATE};
use crate::error::OrchestratorError;
use crate::handoff::parse_verdict;
use crate::harness::{harness, AgentSettings};
use crate::herdr::{Herdr, HerdrError};
use crate::host::{git_status, Host, PushError, Scanned};
use crate::interrupt::Interrupt;
use crate::paths::file_name;
use crate::pipeline::{is_built_in, Pipeline, Step, DONE, METRICS_FILE, PUBLISH, QUALITY, SUMMARY_FILE};
use crate::prompts::Prompts;
use crate::proofs::PROOF_TIMEOUT;
use crate::pull_request::{branch_name, commit_note, pr_body, provenance, spec_title, GateEvidence, QualityFile};
use crate::package_index::PackageIndexes;
use serde_json::Value;
use crate::json::{object, Object, ValueExt};
use crate::quality::parse_gate;
use crate::runs::{local_host, RUNNING};
use crate::spec_file::SpecFile;
use crate::state::RunState;
use crate::summary::{iso_utc, run_metrics, run_summary, turn_tokens};
use crate::workflow_file::{run_pipeline, workflow_definition};
use step_prompt::RunView;

mod gate_switch;
mod locks;
mod origin;
mod proof_runs;
mod quality_rounds;
mod step_prompt;
mod system;
mod turn;

pub use gate_switch::{check_gate_mid_run, turn_quality_gate_off, turn_quality_gate_on};
pub(crate) use origin::{origin_unreachable, reaching_origin};
pub use system::{machine_zone, LocalHuman, SystemClocks};

#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;

/// The time a Workflow keeps: given to it, so that the tests run one on a clock that sleeps at once.
pub trait Clocks {
    /// Wait that many seconds. A fake's time passes only here.
    fn sleep(&self, seconds: f64);
    /// Seconds on a clock that only goes forward, for deadlines and stalls.
    fn monotonic(&self) -> f64;
    /// Seconds since the epoch, for the times state.json records.
    fn wall(&self) -> f64;
    /// The zone this machine shows its local times in, such as show's turn starts.
    fn zone(&self) -> jiff::tz::TimeZone;
}

/// The human watching the run: told what needs them in a notification, and everything else in log lines.
pub trait Human {
    /// A notification, with its title and body.
    fn notify(&self, title: &str, body: &str);
    /// One log line, without the indent the terminal shows it with.
    fn log(&self, line: &str);
}

/// What a Workflow talks to, each borrowed for as long as it runs.
#[derive(Clone, Copy)]
pub struct Collaborators<'a> {
    /// The machine the agents run on: the project's files and git, gh and gitleaks.
    pub host: &'a dyn Host,
    /// The herdr the agents run in.
    pub herdr: &'a dyn Herdr,
    /// The time the run keeps: its deadlines, pauses and the times state.json records.
    pub clocks: &'a dyn Clocks,
    /// Who is told what needs them, and everything the run does.
    pub human: &'a dyn Human,
    /// The Ctrl-C: once raised, the run stops at its next pause, or at the failure of what was running when it was
    /// raised, as interrupted.
    pub interrupt: &'a Interrupt,
    /// Whether a process of a pid exists on this machine, for judging the checkout's other runs (RunState::health):
    /// crate::runs::pid_alive, which a test replaces.
    pub pid_alive: &'a dyn Fn(i64) -> bool,
    /// The indexes the quality rounds' new-dependencies check asks about each package the change adds
    /// (crate::checks); a run without the quality gate never asks them.
    pub indexes: &'a PackageIndexes,
}

/// How a run is to go: each setting but spec saved in the state, so that a resume starts from them.
#[derive(Clone, Debug, PartialEq)]
pub struct RunSettings {
    /// How many rounds the verdict steps' loops take at most, together: a verdict other than APPROVE in a round
    /// before round max_rounds sends the run back to its step's loop target in the next round, and from round
    /// max_rounds on the run goes on past the last verdict step whatever the verdict (Workflow::take_turn).
    pub max_rounds: u32,
    /// Seconds a turn of a step that is not human-paced may take.
    pub turn_timeout: u64,
    pub agents: AgentSettings,
    /// Whether the run ends in a pull request: the editing steps work on a branch of their own, which is committed,
    /// pushed and opened as a pull request against the branch the run started on, checked out again afterwards.
    pub pull_request: bool,
    /// Whether the run has the quality gate: the local gate (crate::local_gate) judges the change in its quality
    /// rounds.
    pub quality_gate_on: bool,
    /// Analyses per round, with the quality gate.
    pub max_quality_rounds: u32,
    /// The run's workflow; None for the one the state names.
    pub pipeline: Option<Pipeline>,
    /// The spec given with run --spec, for a new run whose state starts at the step after the contract step
    /// (Pipeline::seeded_start): written as the contract step's file before the run's first state.json, where a
    /// resume, which is given none, finds it. None for every other run.
    pub spec: Option<SpecFile>,
}

impl Default for RunSettings {
    fn default() -> RunSettings {
        RunSettings {
            max_rounds: DEFAULT_MAX_ROUNDS,
            turn_timeout: DEFAULT_TURN_TIMEOUT,
            agents: AgentSettings::default(),
            pull_request: true,
            quality_gate_on: false,
            max_quality_rounds: DEFAULT_MAX_QUALITY_ROUNDS,
            pipeline: None,
            spec: None,
        }
    }
}

/// Why a run stopped before it was done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// It failed; state.json records why, and that no orchestrator drives it now.
    Failed(OrchestratorError),
    /// Another orchestrator took the run over, so this one stopped without saving anything more.
    TakenOver(OrchestratorError),
    /// The human's Ctrl-C (Interrupt); state.json records "interrupted".
    Interrupted,
}

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stopped::Failed(e) | Stopped::TakenOver(e) => write!(f, "{e}"),
            Stopped::Interrupted => f.write_str("interrupted"),
        }
    }
}

impl From<OrchestratorError> for Stopped {
    fn from(e: OrchestratorError) -> Stopped {
        Stopped::Failed(e)
    }
}

impl From<HerdrError> for Stopped {
    fn from(e: HerdrError) -> Stopped {
        Stopped::Failed(e.into())
    }
}

/// What the Workflow's steps return: their result, or why the run stops.
type Outcome<T> = Result<T, Stopped>;

/// How a role's agent came to be ready for a turn, which decides what it is prompted with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum How {
    /// The role's first agent in this run.
    New,
    /// Still running from before.
    Alive,
    /// Had exited; relaunched into its saved agent session.
    Resumed,
    /// Had exited; relaunched in a fresh session that has lost its earlier turns.
    Restarted,
}

/// Drives one run through the steps of its workflow in a herdr workspace.
///
/// A role's turn ends when it writes its handoff file, not when herdr reports it settled: an agent can end a turn
/// while a background task it started is still running and resume when the task completes, as Claude Code does, so
/// idle or done can come mid-work.
///
/// The same code starts a new run and resumes an interrupted one. A new run is a resume from the first step with no
/// workspace; every step first looks for what an earlier orchestrator, or a role working while none was watching,
/// already did.
///
/// With pull_request, the editing steps work on a new branch, and the finished change is committed, pushed and
/// opened as a pull request against the branch the run started on, which is checked out again afterwards. The
/// workspace is then closed. A run that fails keeps its workspace and branch, to see what happened.
///
/// With the quality gate (RunSettings::quality_gate_on), each turn of the quality-gated step is followed by a quality
/// round (phase quality) that has the gate judge a snapshot of the change; a gate that does not pass goes back to that step,
/// up to max_quality_rounds analyses per round, before the next step.
pub struct Workflow<'a> {
    host: &'a dyn Host,
    herdr: &'a dyn Herdr,
    clocks: &'a dyn Clocks,
    human: &'a dyn Human,
    interrupt: &'a Interrupt,
    pid_alive: &'a dyn Fn(i64) -> bool,
    /// RunSettings::quality_gate_on.
    quality_gate_on: bool,
    /// Collaborators::indexes.
    indexes: &'a PackageIndexes,
    state: RunState,
    /// The run's workflow, shared so that a step borrowed from it outlives no borrow of the Workflow.
    pipeline: Rc<Pipeline>,
    /// Every prompt the workflow sends that its definition does not word: the follow-ups and notes.
    prompts: Prompts,
    /// RunSettings::spec.
    spec: Option<SpecFile>,
    /// This orchestrator as state.json's owner records it, {"host", "pid", "started_at"}: the run is this one's
    /// while the saved owner equals it.
    me: Object,
    /// When state.json was last written by this orchestrator, or a failed heartbeat last tried to, on the monotonic
    /// clock; 0 before.
    last_write: f64,
    /// The roles whose harness has no equivalent of the permission mode and that the log has said so for.
    told_dropped: HashSet<String>,
    /// Seconds one proof may run before GNU timeout stops it (Workflow::run_proof): PROOF_TIMEOUT.
    proof_timeout: u64,
}

impl<'a> Workflow<'a> {
    /// A Workflow for the run state describes, with settings saved into it; settings.pipeline, else the workflow the
    /// state names (and its saved definition), is the run's.
    ///
    /// Errors: the state's prompt overrides or saved workflow cannot be read, the run has the quality gate
    /// (settings.quality_gate_on) and its workflow no quality-gated step, or the run is to end in a pull request and its workflow
    /// has no editing step.
    pub fn new(with: Collaborators<'a>, mut state: RunState, settings: RunSettings) -> Result<Workflow<'a>, OrchestratorError> {
        let prompts = state.prompts()?;
        let pipeline = match settings.pipeline {
            Some(p) => p,
            None => run_pipeline(&state.workflow, state.workflow_definition.as_ref(), &prompts)?,
        };
        if settings.quality_gate_on && pipeline.gated().is_none() {
            return Err(OrchestratorError(format!(
                "workflow {} has no quality-gated step, so the quality gate does not apply",
                pipeline.name()
            )));
        }
        // Publishing commits on the run's branch, which only the first editing step creates.
        if settings.pull_request && pipeline.first_edit().is_none() {
            return Err(OrchestratorError(format!(
                "workflow {} has no editing step, so it cannot end in a pull request; pass --no-pr",
                pipeline.name()
            )));
        }
        state.workflow = pipeline.name().to_string();
        state.workflow_definition =
            if is_built_in(&pipeline, &prompts) { None } else { Some(workflow_definition(&pipeline)) };
        state.max_rounds = settings.max_rounds;
        state.turn_timeout = settings.turn_timeout;
        let agents = settings.agents;
        state.permission_mode = agents.permission_mode;
        state.models = agents.models;
        state.next_harnesses = pipeline
            .roles()
            .keys()
            .map(|role| (role, agents.harnesses.get(role).cloned().unwrap_or_else(|| DEFAULT_HARNESS.to_string())))
            .collect();
        state.pull_request = settings.pull_request;
        state.quality_job = settings.quality_gate_on.then(|| LOCAL_GATE.to_string());
        state.max_quality_rounds = settings.max_quality_rounds;
        let me = object([
            ("host", Value::from(local_host())),
            ("pid", Value::from(std::process::id())),
            ("started_at", Value::from(iso_utc(with.clocks.wall().floor() as i64))),
        ]);
        Ok(Workflow {
            host: with.host,
            herdr: with.herdr,
            clocks: with.clocks,
            human: with.human,
            interrupt: with.interrupt,
            pid_alive: with.pid_alive,
            quality_gate_on: settings.quality_gate_on,
            indexes: with.indexes,
            state,
            pipeline: Rc::new(pipeline),
            prompts,
            spec: settings.spec,
            me,
            last_write: 0.0,
            told_dropped: HashSet::new(),
            proof_timeout: PROOF_TIMEOUT,
        })
    }

    /// The run's state as it stands.
    pub fn state(&self) -> &RunState {
        &self.state
    }

    /// The run as it stands, to read prompts and records from.
    fn view(&self) -> RunView<'_> {
        RunView { state: &self.state, pipeline: &self.pipeline, prompts: &self.prompts, host: self.host }
    }

    /// Run every phase not yet done and return the final verdict: the run's (RunState::verdict), for a workflow
    /// without a verdict step FINISHED, or QUALITY_GATE_FAILED when its quality gate still failed. A run already done
    /// returns its saved verdict, after finishing what of its ending an orchestrator that stopped there left undone
    /// (ending_left): one it had not released yet is taken over (claim) and ended again up to and with the release,
    /// the notice and the closing; one whose ending is whole is left as it is, not even saved.
    ///
    /// Errors: why the run stopped (Stopped); for a failure or an interrupt before the run was done, after recording
    /// it in state.json and writing summary.md, as far as saving still works. An interrupt once the run is done stops
    /// what is left of its ending, and the run stays done.
    pub fn run(&mut self) -> Result<Option<String>, Stopped> {
        if self.state.phase == DONE {
            let finished = match self.ending_left() {
                Ok(None) => Ok(()),
                Ok(Some(left)) if left.release => self.claim().and_then(|()| self.end_done_run(left)),
                Ok(Some(left)) => self.end_done_run(left),
                Err(e) => Err(e.into()),
            };
            return self.ended(finished);
        }
        self.check_phase()?;
        match self.walk_to_the_end() {
            Ok(()) => {}
            Err(Stopped::TakenOver(e)) => return Err(Stopped::TakenOver(e)),
            // A failure that met the Ctrl-C is the command the interrupt stopped.
            Err(Stopped::Failed(e)) if !self.interrupt.is_raised() => {
                self.release(&e.0);
                return Err(Stopped::Failed(e));
            }
            Err(_) => {
                // Caught, so that the commands that record the stop run.
                self.interrupt.catch();
                self.release("interrupted");
                return Err(Stopped::Interrupted);
            }
        }
        self.state.phase = DONE.to_string();
        // Saved first: the run is done whatever its ending meets.
        let finished = self.save().and_then(|()| self.end_done_run(Ending::WHOLE));
        self.ended(finished)
    }

    /// run's result for a run whose ending stopped as finished says: its verdict, or why the ending stopped, a failure
    /// that met the Ctrl-C counting as the Ctrl-C.
    fn ended(&self, finished: Outcome<()>) -> Result<Option<String>, Stopped> {
        match finished {
            Err(Stopped::Failed(_)) if self.interrupt.is_raised() => Err(Stopped::Interrupted),
            ended => ended.map(|()| self.state.verdict.clone()),
        }
    }

    /// What of the ending (end_done_run) of this run, saved as done, an orchestrator that stopped after saving it so
    /// left undone; None when nothing is.
    ///
    /// A run still owned was stopped before its release: all of the ending is taken again, but for tokens already
    /// counted, whose session logs may be gone since. A released run lacks at most a file whose write failed and was
    /// only logged, or one an older engine, which released the run before writing the files, had not written yet:
    /// that file is written, and the human is not told again. Errors: a file's presence cannot be read.
    fn ending_left(&self) -> Result<Option<Ending>, OrchestratorError> {
        let s = &self.state;
        if s.owner.is_some() {
            let count_tokens = s.turns.iter().any(|t| t.harness.is_some() && !t.tokens.is_set());
            return Ok(Some(Ending { count_tokens, ..Ending::WHOLE }));
        }
        let missing = |name: &str| self.host.exists(&format!("{}/{name}", s.dir())).map(|there| !there);
        let (write_summary, write_metrics) = (missing(SUMMARY_FILE)?, missing(METRICS_FILE)?);
        Ok((write_summary || write_metrics).then_some(Ending {
            count_tokens: false,
            write_summary,
            write_metrics,
            release: false,
        }))
    }

    /// What follows the last phase, for a run saved as done, as far as left says: its tokens counted, its summary and
    /// metrics written, and then, for a run this orchestrator owns, the run released, the human told, and the
    /// workspace of a published run closed. What only costs a file or a notice is logged when it fails, but none
    /// goes on past the Ctrl-C.
    ///
    /// The files are written before the release, so that a run released is one whose files were written: a resume
    /// (ending_left) tells an ending cut short by the owner alone. The notice and the closing
    /// leave no trace, so they come once, after the release: an orchestrator that stopped between the release and
    /// the notice costs the human the notice, which is accepted over telling them twice of every run ended whole.
    fn end_done_run(&mut self, left: Ending) -> Outcome<()> {
        if left.count_tokens {
            self.count_tokens();
            self.check_interrupt()?;
        }
        if left.write_summary {
            self.write_summary();
            self.check_interrupt()?;
        }
        if left.write_metrics {
            self.write_metrics();
            self.check_interrupt()?;
        }
        if !left.release {
            return Ok(());
        }
        self.state.owner = None;
        self.save()?;
        let s = &self.state;
        if let (Some(worktree), false) = (&s.worktree, s.pull_request) {
            self.human.log(&format!("the change is in worktree {worktree}"));
        }
        let title = s.verdict.as_ref().map_or("Run finished without a verdict".to_string(), |v| format!("Run finished: {v}"));
        let body = s.pr_url.as_deref().filter(|url| !url.is_empty()).unwrap_or(&s.task);
        self.human.notify(&title, body);
        self.check_interrupt()?;
        if self.state.pull_request {
            self.close_workspace();
        }
        self.check_interrupt()
    }

    /// Everything of run between checking the phase and the end: what a failure in it releases the run for.
    fn walk_to_the_end(&mut self) -> Outcome<()> {
        self.claim()?;
        self.check_start()?;
        // Publishing needs no agent, so a resume there opens no workspace only to close it.
        if self.state.phase != PUBLISH {
            self.prepare()?;
        }
        self.walk()?;
        if self.pipeline.verdict_steps().next().is_none() {
            let passed = self.gate_passed(self.state.round)?;
            self.state.verdict = Some(if passed { FINISHED } else { QUALITY_GATE_FAILED }.to_string());
        }
        if self.state.pull_request {
            self.publish()?;
        }
        self.clean_up_locks();
        self.check_interrupt()
    }

    fn check_phase(&self) -> Result<(), OrchestratorError> {
        let (s, p) = (&self.state, &self.pipeline);
        let known = p.steps().iter().any(|st| st.id == s.phase)
            || s.phase == PUBLISH
            || (s.phase == QUALITY && p.gated().is_some());
        if known {
            return Ok(());
        }
        Err(OrchestratorError(format!(
            "run {} is in phase {}, which workflow {} has no step for",
            s.run_id,
            s.phase,
            p.name()
        )))
    }

    /// Fail before the next turn if the run could not finish.
    fn check_start(&mut self) -> Outcome<()> {
        self.check_alone()?;
        // Round 0: the run has not reached its first editing step, so it has no base or branch yet.
        if self.state.round == 0 {
            return self.check_before_interview();
        }
        if self.state.worktree.is_some() {
            self.check_worktree()?;
        }
        self.check_branch()?;
        Ok(())
    }

    /// Open the run's worktree, if it has one, and fail if the run could not push what it makes, run its quality
    /// gate or open its pull request, before the Spec Collector's first turn.
    fn check_before_interview(&mut self) -> Outcome<()> {
        if self.state.worktree.is_some() {
            self.open_worktree()?;
        }
        if self.quality_gate_on {
            self.check_gate()?;
        } else if self.state.pull_request {
            // Publishing scans what it pushes; the gate's preflight checks gitleaks itself.
            self.host.check_gitleaks()?;
        }
        if self.state.pull_request && self.state.worktree.is_none() {
            self.check_repo()?;
        }
        Ok(())
    }

    /// Fail before the interview if the quality gate could not judge the run's change: cwd has no commit, a run in it
    /// has changes the snapshots would take, gitleaks, which scans every snapshot first, cannot run, or the host fails
    /// local_gate::preflight.
    fn check_gate(&self) -> Result<(), OrchestratorError> {
        let cwd = &self.state.cwd;
        if self.host.git_head(cwd)?.is_none() {
            return Err(OrchestratorError(format!(
                "the quality gate needs a git repository, and {cwd} is not one with a commit"
            )));
        }
        // check_repo checks both for a pull-request run, and a worktree run's worktree starts out clean.
        if !self.state.pull_request {
            // A snapshot takes every change in the tree, so the tree must start with none.
            self.require_clean()?;
        }
        self.host.check_gitleaks()?;
        local_gate::preflight(self.host)
    }

    /// Before the interview, check out origin's base branch, detached, in the run's worktree.
    ///
    /// The base branch is the one checked out in the project. The checkout itself is left as it is, so it need not be
    /// clean or up to date, and the human can go on working in it.
    fn open_worktree(&mut self) -> Outcome<()> {
        let (cwd, worktree) = (self.state.cwd.clone(), self.state.worktree.clone().unwrap_or_default());
        if self.host.git_head(&cwd)?.is_none() {
            return Err(Stopped::Failed(OrchestratorError(format!(
                "{cwd} is not a git repository with a commit, which --worktree needs"
            ))));
        }
        let branch = self.host.git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
        if branch == "HEAD" {
            return Err(Stopped::Failed(OrchestratorError(format!(
                "{cwd} is on a detached HEAD; --worktree bases the run on the checked-out branch, so check one out"
            ))));
        }
        if !self.host.has_origin(&cwd)? {
            return Err(Stopped::Failed(OrchestratorError(format!(
                "--worktree starts the run from {REMOTE}'s branch, and {cwd} has no {REMOTE}"
            ))));
        }
        // Kept once recorded, as check_repo keeps it.
        if self.state.base_branch.as_deref().is_none_or(str::is_empty) {
            self.state.base_branch = Some(branch);
        }
        let run_id = self.state.run_id.clone();
        // A resume before the first editing step: the run's worktree holds nothing it needs to keep.
        if self.host.is_worktree(&cwd, &worktree)? {
            self.human.log(&format!("run {run_id} resumes in worktree {worktree}"));
            return Ok(());
        }
        let base_branch = self.base_branch()?;
        let upstream = self.reaching_origin(|| self.host.fetch(&cwd, &base_branch))?;
        self.host.add_worktree(&cwd, &worktree, &upstream)?;
        self.human.log(&format!("run {run_id} in worktree {worktree}, at {REMOTE}/{base_branch}"));
        self.save()
    }

    /// Fail if the run's worktree, which holds its change, is gone; unless the pull request has the change.
    fn check_worktree(&self) -> Result<(), OrchestratorError> {
        let s = &self.state;
        let worktree = s.worktree.as_deref().unwrap_or_default();
        if !s.pr_url.as_deref().unwrap_or_default().is_empty() || self.host.is_worktree(&s.cwd, worktree)? {
            return Ok(());
        }
        Err(OrchestratorError(format!(
            "the worktree of run {}, {worktree}, is gone or no longer a git worktree, and with it the run's change; \
             the run cannot go on",
            s.run_id
        )))
    }

    /// Fail if another run in this checkout is live: the two would share one working tree.
    ///
    /// A worktree run shares no working tree, neither with another worktree run nor with the run in the checkout.
    /// After claim, so of two runs that start together each sees the other's state.json and both stop, rather than
    /// both going on.
    fn check_alone(&self) -> Result<(), OrchestratorError> {
        let s = &self.state;
        if s.worktree.is_some() {
            return Ok(());
        }
        let me = self.me.get("host").and_then(Value::as_str).unwrap_or_default();
        for run in self.host.run_states(&s.cwd)? {
            // As far as it reads: a run another release drives shares the checkout too.
            let other = run.shown();
            if other.run_id == s.run_id || other.worktree.as_deref().is_some_and(|w| !w.is_empty()) {
                continue;
            }
            if let Some((RUNNING, why)) = run.health(me, self.pid_alive) {
                return Err(OrchestratorError(format!(
                    "run {} is running in {} ({why}); one run at a time per checkout: wait for it, stop it, or \
                     use another clone",
                    other.run_id, s.cwd
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn claim(&mut self) -> Outcome<()> {
        let cwd = self.state.cwd.clone();
        self.ignore_orchestrator_dir(&cwd)?;
        if let Some(spec) = &self.spec {
            self.seed_spec(spec)?;
        }
        self.state.error = None;
        self.state.owner = Some(self.me.clone());
        // Not save: the caller has already decided that any earlier owner is gone.
        Ok(self.write_state()?)
    }

    /// Make git ignore everything under the .orchestrator directory of the working tree at tree, by a .gitignore of
    /// `*` there, unless it has a .gitignore already. Keeps run files out of `git status`, so the Reviewer sees only
    /// the Builder's changes and the commit holds nothing else.
    fn ignore_orchestrator_dir(&self, tree: &str) -> Result<(), OrchestratorError> {
        let ignore = format!("{tree}/.orchestrator/.gitignore");
        if self.host.read(&ignore)?.is_none() {
            self.host.write(&ignore, "*\n", false)?;
        }
        Ok(())
    }

    /// Write spec, given with run --spec (RunSettings::spec), as the contract step's file.
    ///
    /// After .gitignore, which keeps it out of the clean-tree check, and before the first state.json, which starts
    /// the run after the contract step: a resume of that run must find the file. Errors: the workflow has no contract
    /// step, which Pipeline::seeded_start refuses first; the write fails.
    fn seed_spec(&self, spec: &SpecFile) -> Result<(), OrchestratorError> {
        let Some(contract) = self.pipeline.contract() else {
            return Err(OrchestratorError(format!(
                "workflow {} has no contract step to write the --spec file as",
                self.pipeline.name()
            )));
        };
        let path = contract.path(&self.state.dir(), 0, 0);
        self.host.write(&path, &spec.text, false)?;
        let skipped = if contract.human_paced { "interview" } else { "turn" };
        self.human.log(&format!(
            "[{}] {} is a copy of {}; skipping the {}'s {skipped}",
            contract.role,
            file_name(&path),
            spec.path,
            self.label(&contract.role)
        ));
        Ok(())
    }

    /// Record why the run stopped, and that no process drives it now, as far as saving still works.
    fn release(&mut self, error: &str) {
        self.state.error = Some(error.to_string());
        self.state.owner = None;
        if let Err(e) = self.save() {
            self.human.log(&format!("could not record the failure in state.json: {e}"));
            if let Stopped::TakenOver(_) = e {
                // The run is another orchestrator's now, and so is its summary.
                return;
            }
        }
        self.write_summary();
    }

    /// Write summary.md for a run that just ended; a failure costs only the summary, so it is only logged.
    fn write_summary(&self) {
        let written = self.round_results().and_then(|rounds| {
            let s = &self.state;
            let summary = run_summary(s, &self.pipeline, &rounds, self.host.now()?);
            self.host.write(&format!("{}/{SUMMARY_FILE}", s.dir()), &summary, false)
        });
        if let Err(e) = written {
            self.human.log(&format!("could not write {SUMMARY_FILE}: {e}"));
        }
    }

    /// Write metrics.json for a run that is done; as for the summary, a failure is only logged.
    fn write_metrics(&self) {
        let s = &self.state;
        let written = self.host.now().and_then(|now| {
            // One line: `metrics` of a release before crate::state::STATE_FORMAT_SINCE reads the project's metrics.json
            // files joined, a line at a time, so a file over several lines breaks it for every run in the project.
            self.host.write(&format!("{}/{METRICS_FILE}", s.dir()), &(run_metrics(s, now).to_string() + "\n"), false)
        });
        if let Err(e) = written {
            self.human.log(&format!("could not write {METRICS_FILE}: {e}"));
        }
    }

    /// Give each agent turn the tokens its harness recorded for it (turn_tokens), "unknown" where it recorded none
    /// the orchestrator can read. A session log that cannot be read costs only its turns' counts.
    fn count_tokens(&mut self) {
        let mut usage = std::collections::HashMap::new();
        for i in 0..self.state.turns.len() {
            let t = &self.state.turns[i];
            let Some(kind) = t.harness.clone() else { continue };
            let key = (kind, t.session.clone().unwrap_or_default());
            if !usage.contains_key(&key) {
                let read = || -> Result<_, OrchestratorError> {
                    if key.1.is_empty() {
                        return Ok(None);
                    }
                    let Some(log) = self.host.session_log(&key.0, &key.1)? else { return Ok(None) };
                    Ok(Some(harness(&key.0)?.session_usage(&log)))
                };
                let counted = read().unwrap_or_else(|e| {
                    self.human.log(&format!("could not read the {} session log of {}: {e}", key.0, key.1));
                    None
                });
                usage.insert(key.clone(), counted);
            }
            let tokens = turn_tokens(t, usage[&key].as_deref());
            self.state.turns[i].tokens = tokens;
        }
    }

    /// Per round, its quality gates and the verdict of each verdict step that judged it, as its handoff files say.
    fn round_results(&self) -> Result<Vec<String>, OrchestratorError> {
        let s = &self.state;
        let mut lines = vec![];
        for n in 1..=s.round {
            let mut parts = vec![];
            let gates = self.round_gates(n)?;
            if !gates.is_empty() {
                parts.push(format!("quality gate {}", gates.join(", ")));
            }
            for (step, text) in self.round_reviews(n)? {
                parts.push(format!("{} {}", step.id, parse_verdict(&text).unwrap_or("?")));
            }
            let parts = if parts.is_empty() { "no verdict".to_string() } else { parts.join("; ") };
            lines.push(format!("Round {n}: {parts}"));
        }
        Ok(lines)
    }

    /// The gate of each of round n's quality rounds, in order, as their quality files say: "?" for a file that names
    /// none, and none from the first quality round that has no file on. Empty for a run that never had the quality
    /// gate. Errors when a quality file cannot be read.
    fn round_gates(&self, n: u32) -> Result<Vec<&'static str>, OrchestratorError> {
        let s = &self.state;
        let mut gates = vec![];
        while s.had_quality_gate() {
            let Some(text) = self.host.read(&s.quality_path(n, gates.len() as u32 + 1))? else { break };
            gates.push(parse_gate(&text).unwrap_or("?"));
        }
        Ok(gates)
    }

    /// Fail before the interview if the change could not become a pull request.
    fn check_repo(&mut self) -> Result<(), OrchestratorError> {
        let cwd = self.state.cwd.clone();
        if self.host.git_head(&cwd)?.is_none() {
            return Err(OrchestratorError(format!("{cwd} is not a git repository with a commit; pass --no-pr")));
        }
        let branch = self.host.git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
        if branch == "HEAD" {
            return Err(OrchestratorError(format!(
                "{cwd} is on a detached HEAD; check out the branch the pull request targets"
            )));
        }
        // Kept once recorded: a run resumed after it switched to its own branch still targets the first one.
        if self.state.base_branch.as_deref().is_none_or(str::is_empty) {
            self.state.base_branch = Some(branch.clone());
        }
        self.require_clean()?;
        // So the Spec Collector reads current code. A resume already on the run's own branch leaves it to
        // switch_to_branch.
        let base_branch = self.base_branch()?;
        if branch == base_branch {
            self.reaching_origin(|| self.host.fast_forward(&cwd, &base_branch))?;
        }
        Ok(())
    }

    /// Fail if HEAD is not the run's branch, which holds the Builder's uncommitted change until publishing.
    ///
    /// Otherwise the commit lands on whatever the human checked out, and the run's branch is pushed without it.
    fn check_branch(&self) -> Result<(), OrchestratorError> {
        let s = &self.state;
        let branch = match &s.branch {
            Some(b) if s.pull_request && !b.is_empty() && s.pr_url.as_deref().is_none_or(str::is_empty) => b,
            _ => return Ok(()),
        };
        let current = self.host.git(s.work_dir(), &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
        // A detached HEAD reads as "HEAD", so it never matches.
        if &current != branch {
            return Err(OrchestratorError(format!(
                "{} is on {current}, not {branch}, which holds this run's change; check out {branch} and resume",
                s.work_dir()
            )));
        }
        Ok(())
    }

    /// Fail if the working tree has uncommitted changes: the commit takes every change in it, so it must hold only
    /// the Builder's.
    fn require_clean(&self) -> Result<(), OrchestratorError> {
        let work_dir = self.state.work_dir();
        let dirty = git_status(self.host, work_dir)?;
        if !dirty.trim().is_empty() {
            return Err(OrchestratorError(format!(
                "{work_dir} has uncommitted changes; commit or stash them first:\n{}",
                dirty.trim_end()
            )));
        }
        Ok(())
    }

    /// reach, a call of the host's that reaches origin, tried again through a short outage of origin, as
    /// origin::reaching_origin tries it.
    fn reaching_origin<T>(&self, reach: impl FnMut() -> Result<T, OrchestratorError>) -> Result<T, OrchestratorError> {
        origin::reaching_origin(self.clocks, self.human, self.interrupt, reach)
    }

    /// The branch the pull request targets, recorded before the interview. Errors: none is recorded.
    fn base_branch(&self) -> Result<String, OrchestratorError> {
        self.state.base_branch.clone().ok_or_else(|| OrchestratorError("the run has no base branch yet".into()))
    }

    /// Open the run's herdr workspace, or find it from before.
    fn prepare(&mut self) -> Outcome<()> {
        let s = &mut self.state;
        if !s.workspace_id.is_empty() {
            if self.herdr.workspace_exists(&s.workspace_id)? {
                self.human.log(&format!("run {} resumes in herdr workspace {}", s.run_id, s.workspace_id));
                return Ok(());
            }
            // The handoff files are the whole contract between roles, so a new workspace loses nothing a role
            // needs; its panes went with the old one.
            self.human.log(&format!("herdr workspace {} is gone; opening a new one", s.workspace_id));
            for agent in s.agents.values_mut() {
                agent.pane.clear();
            }
        }
        let label = workspace_label(&s.run_id, &s.task);
        let (workspace, root) = self.herdr.create_workspace(s.work_dir(), &label)?;
        (s.workspace_id, s.root_pane) = (workspace, root);
        self.human.log(&format!("run {} in herdr workspace {}", s.run_id, s.workspace_id));
        self.save()
    }

    /// The quality-gated step, whose quality round phase quality is. Errors: the workflow has none, which check_phase
    /// rules out for that phase.
    fn gated_step(&self) -> Result<Step, OrchestratorError> {
        self.pipeline.gated().cloned().ok_or_else(|| {
            OrchestratorError(format!(
                "a quality round in workflow {}, which has no quality-gated step",
                self.pipeline.name()
            ))
        })
    }

    /// The step of the current phase. Errors: the workflow has none, which check_phase rules out.
    fn current_step(&self) -> Result<Step, OrchestratorError> {
        let phase = &self.state.phase;
        self.pipeline.step(phase).cloned().ok_or_else(|| {
            OrchestratorError(format!("workflow {} has no step {phase}", self.pipeline.name()))
        })
    }

    /// Take the turn of the current step and of each one after it, looping back as the verdict says.
    fn walk(&mut self) -> Outcome<()> {
        if self.state.phase == PUBLISH {
            return Ok(());
        }
        if self.state.round == 0 {
            let step = self.current_step()?;
            if step.edits {
                // A run whose first step edits.
                self.goto(&step, 0)?;
            }
        }
        let pipeline = Rc::clone(&self.pipeline);
        loop {
            let n = self.state.round;
            let done_with = if self.state.phase == QUALITY {
                let gated = self.gated_step()?;
                self.judge_quality(&gated, n)?.then_some(gated.id)
            } else {
                let step = self.current_step()?;
                self.take_turn(&step, n)?
            };
            let Some(done_with) = done_with else { continue };
            match pipeline.after(&done_with) {
                None => return Ok(()),
                Some(following) => self.goto(following, n)?,
            }
        }
    }

    /// The step's turn in round n: the id of the step the run goes on after, the step's own or, for a verdict that
    /// ends the last round early, the last verdict step's; None when the run goes on elsewhere, saved already.
    ///
    /// A round runs the verdict steps in order, and ends at the first that does not approve: in a round before
    /// max_rounds back to its loop's target, in the next round, and in round max_rounds past the last verdict step.
    /// So a later verdict step never judges a change an earlier one rejected.
    fn take_turn(&mut self, step: &Step, n: u32) -> Outcome<Option<String>> {
        let pipeline = Rc::clone(&self.pipeline);
        let proves = pipeline.proves(step);
        if proves {
            self.rerun_proofs(step, n)?;
        }
        let out = self.step_turn(step, n)?;
        if step.edits {
            // Before the quality round, so the gate judges the locked files.
            self.hold_locks(step, n)?;
        }
        if step.quality_gated && (self.quality_gate_on || self.state.quality_round > 0) {
            // The report goes to the next quality round. A round whose gate resume turned off since its last quality
            // round never analyses it, and the next step reads the report all the same (Step::last_path).
            self.state.quality_round += 1;
            if self.quality_gate_on {
                self.state.phase = QUALITY.to_string();
                self.save()?;
                return Ok(None);
            }
        }
        let Some(loop_to) = &step.loop_to else { return Ok(Some(step.id.clone())) };
        // turn accepts no verdict file without one.
        let verdict = parse_verdict(&out).unwrap_or_default();
        self.state.record_verdict(n, &step.id, verdict);
        // An approval is the run's only once the round's last verdict step gives it: until then a saved state keeps
        // the verdict of the round before.
        let last_verdict_step = pipeline.verdict_steps().next_back().map(|last| last.id.clone());
        if verdict != APPROVE || last_verdict_step.as_deref() == Some(step.id.as_str()) {
            self.state.verdict = Some(verdict.to_string());
        }
        self.human.log(&format!("[{}] round {n}: {verdict}", step.role));
        if verdict == CHANGES_REQUESTED && proves && pipeline.proof_command().is_some() {
            self.judge_proofs(step, n, &out)?;
        }
        if verdict == APPROVE {
            return Ok(Some(step.id.clone()));
        }
        if n < self.state.max_rounds {
            self.state.quality_round = 0;
            let target = pipeline.step(loop_to).ok_or_else(|| OrchestratorError(format!("no step {loop_to}")))?;
            self.goto(target, n + 1)?;
            return Ok(None);
        }
        Ok(last_verdict_step)
    }

    /// Make step the current one, in round n, and save that; the first editing step starts round 1.
    fn goto(&mut self, step: &Step, mut n: u32) -> Outcome<()> {
        if n == 0 && step.edits {
            if self.state.pull_request {
                self.switch_to_branch()?;
            }
            // Only here, before the first editing step and after the base branch was fast-forwarded: every later
            // step, resumed or not, diffs against this base, even if the human commits the Builder's work meanwhile.
            self.state.base = self.host.git_head(self.state.work_dir())?;
            n = 1;
        }
        self.state.phase = step.id.clone();
        self.state.round = n;
        self.save()
    }

    /// The step's turn in round n: its handoff file, once written.
    fn step_turn(&mut self, step: &Step, n: u32) -> Outcome<String> {
        let (text, fresh_text) = self.view().step_prompt(step, n)?;
        let turn = turn::Turn {
            role: step.role.clone(),
            path: self.view().own_path(step, n),
            text,
            fresh_text,
            // Paced by the human, so the turn has no deadline, and an idle agent is normal: it is waiting for the
            // human's reply.
            timeout: (!step.human_paced).then_some(self.state.turn_timeout),
            watch_stalls: !step.human_paced,
            announce: if step.human_paced { turn::Announce::Interview } else { turn::Announce::Nothing },
            judged: if step.loop_to.is_some() {
                turn::Judged::Verdict
            } else if step.can_block() {
                turn::Judged::Blocking
            } else {
                turn::Judged::AsWritten
            },
        };
        self.turn(turn)
    }

    /// Before the first editing step: create the run's branch from the base branch, brought up to origin's, unless
    /// the checkout is on it already.
    fn switch_to_branch(&mut self) -> Outcome<()> {
        let pipeline = Rc::clone(&self.pipeline);
        let spec = self.last_text(pipeline.contract(), 0)?;
        let title = spec_title(&spec).filter(|t| !t.is_empty()).unwrap_or_else(|| self.state.task.clone());
        let branch = branch_name(&title, &self.state.run_id);
        self.state.branch = Some(branch.clone());
        let (cwd, work_dir) = (self.state.cwd.clone(), self.state.work_dir().to_string());
        // Already there when an earlier orchestrator stopped between the switch and saving the next phase.
        let current = self.host.git(&work_dir, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
        if current != branch {
            // Checked again: the human may have touched the tree during the interview.
            self.require_clean()?;
            // Again, because the interview can take hours: the branch starts from origin's latest base.
            let base_branch = self.base_branch()?;
            if self.state.worktree.is_some() {
                // From origin's base itself: the worktree's detached HEAD is where it was before the interview, and
                // the project's own base branch is the human's.
                let upstream = self.reaching_origin(|| self.host.fetch(&work_dir, &base_branch))?;
                self.host.git(&work_dir, &["switch", "-c", &branch, &upstream])?;
            } else {
                if current != base_branch {
                    return Err(Stopped::Failed(OrchestratorError(format!(
                        "{cwd} is on {current}, not {base_branch}, which the pull request targets; check out \
                         {base_branch} and resume"
                    ))));
                }
                self.reaching_origin(|| self.host.fast_forward(&cwd, &base_branch))?;
                self.host.git(&cwd, &["switch", "-c", &branch])?;
            }
        }
        self.human.log(&format!("building on branch {branch}"));
        Ok(())
    }

    /// Commit the change, merge the latest base into it, push its branch and open a pull request for it.
    ///
    /// Each step first checks whether an earlier orchestrator already took it, so a resume in this phase finishes the
    /// job instead of committing, merging or opening a second pull request.
    fn publish(&mut self) -> Outcome<()> {
        let pipeline = Rc::clone(&self.pipeline);
        self.state.phase = PUBLISH.to_string();
        self.save()?;
        if self.state.pr_url.as_deref().is_none_or(str::is_empty) {
            let wd = self.state.work_dir().to_string();
            let branch = self.state.branch.clone();
            let branch = branch.ok_or_else(|| OrchestratorError("a publish before the run has a branch".into()))?;
            // Again: the human may have switched branches during a turn.
            self.check_branch()?;
            // An earlier orchestrator stopped mid-merge: committing now would commit the conflict markers.
            if self.host.abort_merge(&wd)? {
                self.human.log(&format!("aborted the merge an earlier orchestrator left in {wd}"));
            }
            let spec = self.last_text(pipeline.contract(), 0)?;
            let s = &self.state;
            let first_line = s.task.trim().split('\n').next().unwrap_or_default().to_string();
            let title = spec_title(&spec).filter(|t| !t.is_empty()).unwrap_or(first_line);
            let title: String = if title.is_empty() { s.run_id.clone() } else { title }.chars().take(72).collect();
            let quality = self.gate_evidence(s.round)?;
            if !git_status(self.host, &wd)?.trim().is_empty() {
                self.host.git(&wd, &["add", "--all"])?;
                let trailers: Vec<String> = provenance(s, &quality)
                    .into_iter()
                    .flat_map(|(key, said)| ["--trailer".to_string(), format!("{key}: {said}")])
                    .collect();
                let note = commit_note(s);
                let mut args = vec!["commit", "--quiet", "-m", &title, "-m", &note];
                args.extend(trailers.iter().map(String::as_str));
                self.host.git(&wd, &args)?;
            } else if self.host.git_head(&wd)? == s.base {
                return Err(Stopped::Failed(OrchestratorError(format!(
                    "the Builder changed no files; there is nothing to commit on {branch}"
                ))));
            }
            let leaks = self.state.leaks_path("publish");
            // Scanned before the merge too, so that rewriting the run's commits never has to undo a merge.
            self.rewrite_on_leak(|host, base, branch| {
                host.refuse_leaks(&wd, &format!("refs/heads/{branch}"), base, Scanned::Unpublished, &leaks)
            })?;
            // The base moved on while the run built and reviewed. A conflict is left to the human: the branch is
            // pushed without the merge and the pull request says what conflicts.
            let base_branch = self.base_branch()?;
            self.state.conflicts = self.reaching_origin(|| self.host.merge_upstream(&wd, &base_branch))?;
            self.save()?;
            let s = &self.state;
            if !s.conflicts.is_empty() {
                self.human.log(&format!(
                    "{branch} conflicts with {REMOTE}/{base_branch} in {}; the pull request will be a draft",
                    s.conflicts.join(", ")
                ));
            }
            self.rewrite_on_leak(|host, base, branch| host.push_branch(&wd, branch, base, &leaks))?;
            let report = self.last_text(pipeline.last_edit(), self.state.round)?;
            let n = self.state.round;
            let reviews: Vec<(String, String)> =
                self.round_reviews(n)?.into_iter().map(|(step, text)| (step.name(n, 0), text)).collect();
            let s = &self.state;
            let body = pr_body(s, &spec, &report, &reviews, &quality);
            let draft = !SUCCEEDED.contains(&s.verdict.as_deref().unwrap_or_default()) || !s.conflicts.is_empty();
            let url = self.host.create_pr(&wd, &base_branch, &branch, &title, &body, draft)?;
            self.state.pr_url = Some(url.clone());
            self.save()?;
            self.human.log(&format!("opened {url}"));
            if !self.state.conflicts.is_empty() {
                self.human.notify(&format!("Pull request conflicts with {base_branch}"), &url);
            }
        }
        Ok(self.leave_work_dir()?)
    }

    /// push(host, base, branch), a scan of the run's branch, or its push, at publish. A secret found becomes the
    /// error that ends the run, still in phase publish: the commit holding the secret is already on the local branch,
    /// so fixing the working tree is not enough, and the error says how to rewrite it before a resume.
    fn rewrite_on_leak(
        &self,
        push: impl FnOnce(&dyn Host, &str, &str) -> Result<(), PushError>,
    ) -> Result<(), OrchestratorError> {
        let s = &self.state;
        let (Some(base), Some(branch)) = (&s.base, &s.branch) else {
            return Err(OrchestratorError("a publish before the run has a base and a branch".into()));
        };
        match push(self.host, base, branch) {
            Ok(()) => Ok(()),
            Err(PushError::Failed(e)) => Err(e),
            Err(PushError::Leaks(e)) => Err(OrchestratorError(format!(
                "{e}\nThose commits are on {branch} in {} and not on {REMOTE}. Rewrite them without the secret, for \
                 example with `git reset --soft {base}` there, remove it from the files, and resume the run: it stays \
                 in phase {PUBLISH} and commits again.",
                s.work_dir()
            ))),
        }
    }

    /// Once the pull request has the change: check the base branch out again, or remove the run's worktree.
    ///
    /// The base branch leaves the checkout where the human had it, so the next run branches from there too. The
    /// worktree goes without --force, which git refuses for one with untracked or modified files. publish has
    /// committed them all, so a refusal means someone changed the worktree since; that, like any failure in removing
    /// it, is only logged. The run's branch stays either way.
    fn leave_work_dir(&self) -> Result<(), OrchestratorError> {
        let s = &self.state;
        let Some(worktree) = &s.worktree else {
            let base_branch = self.base_branch()?;
            return self.host.git(&s.cwd, &["switch", "--quiet", &base_branch]).map(drop);
        };
        let removed = self.host.is_worktree(&s.cwd, worktree).and_then(|is| {
            if is {
                self.host.remove_worktree(&s.cwd, worktree, false)?;
                self.human.log(&format!("removed worktree {worktree}"));
            }
            Ok(())
        });
        if let Err(e) = removed {
            self.human.log(&format!("could not remove worktree {worktree}: {e}"));
        }
        Ok(())
    }

    /// The step's last file of round n, or "" for no step or no file.
    fn last_text(&self, step: Option<&Step>, n: u32) -> Result<String, OrchestratorError> {
        let Some(step) = step else { return Ok(String::new()) };
        let s = &self.state;
        Ok(self.host.read(&step.last_path(&s.dir(), n, s.quality_round))?.unwrap_or_default())
    }

    /// Each verdict step that judged round n, with the text of its file of the round, in step order; empty for a
    /// workflow without a verdict step. Errors: a file cannot be read.
    fn round_reviews(&self, n: u32) -> Result<Vec<(&Step, String)>, OrchestratorError> {
        let mut reviews = vec![];
        let dir = self.state.dir();
        for (step, path) in self.pipeline.verdict_files(&dir, n) {
            if let Some(text) = self.host.read(&path)? {
                reviews.push((step, text));
            }
        }
        Ok(reviews)
    }

    /// Close the run's workspace once the pull request carries the change; a failure leaves only clutter behind, so
    /// it is only logged.
    fn close_workspace(&self) {
        let workspace = &self.state.workspace_id;
        if let Err(e) = self.herdr.close_workspace(workspace) {
            self.human.log(&format!("could not close herdr workspace {workspace}: {e}"));
        }
    }

    /// What the quality gate said of round n's change, the run's final one: round n's last quality file, or for an
    /// unjudged change (unjudged) the run's last one from any round.
    fn gate_evidence(&self, n: u32) -> Result<GateEvidence, OrchestratorError> {
        let s = &self.state;
        let view = self.view();
        let last = view.last_quality(n)?;
        let unjudged = view.unjudged(last.as_ref());
        if last.is_some() || !unjudged {
            let last = last.map(|(quality_round, text)| QualityFile { round: n, quality_round, text });
            return Ok(GateEvidence { last, unjudged });
        }
        // Round n has none, and quality_round no longer says how far an earlier round got: its quality files are
        // numbered from 1 without gaps, so its last is the one before the first missing.
        for m in (1..n).rev() {
            let mut found = None;
            let mut q = 1;
            while let Some(text) = self.host.read(&s.quality_path(m, q))? {
                found = Some(QualityFile { round: m, quality_round: q, text });
                q += 1;
            }
            if found.is_some() {
                return Ok(GateEvidence { last: found, unjudged: true });
            }
        }
        Ok(GateEvidence { last: None, unjudged: true })
    }

    fn state_path(&self) -> String {
        format!("{}/state.json", self.state.dir())
    }

    /// Write state.json, unless another orchestrator has taken the run over since this one claimed it.
    fn save(&mut self) -> Outcome<()> {
        let path = self.state_path();
        if let Some(saved) = self.host.read(&path)? {
            let owner = match serde_json::from_str::<Value>(&saved) {
                Ok(Value::Object(saved)) => saved.get("owner").cloned().unwrap_or(Value::Null),
                Ok(other) => {
                    let kind = other.kind();
                    return Err(Stopped::Failed(OrchestratorError(format!("corrupt {path}: {kind} where an object belongs"))));
                }
                Err(e) => return Err(Stopped::Failed(OrchestratorError(format!("corrupt {path}: {e}")))),
            };
            if owner != Value::Object(self.me.clone()) {
                let who = match &owner {
                    o if o.is_set() => {
                        let field = |key: &str| o.get(key).map_or("unknown".to_string(), Value::text);
                        format!("pid {} on {}", field("pid"), field("host"))
                    }
                    _ => "another orchestrator".to_string(),
                };
                return Err(Stopped::TakenOver(OrchestratorError(format!(
                    "run {} was taken over by {who}; stopping without saving",
                    self.state.run_id
                ))));
            }
        }
        Ok(self.write_state()?)
    }

    fn write_state(&mut self) -> Result<(), OrchestratorError> {
        self.state.heartbeat_at = Some(self.timestamp());
        self.host.write(&self.state_path(), &self.state.to_text(), false)?;
        self.last_write = self.clocks.monotonic();
        Ok(())
    }

    /// Keep state.json fresh while a turn runs, so the run does not look stale: save it once HEARTBEAT_SECONDS have
    /// passed since the last write. Also where a wait meets the Ctrl-C.
    fn heartbeat(&mut self) -> Outcome<()> {
        self.check_interrupt()?;
        if self.clocks.monotonic() - self.last_write < HEARTBEAT_SECONDS as f64 {
            return Ok(());
        }
        match self.save() {
            // A save the Ctrl-C failed is the Ctrl-C.
            Err(Stopped::Failed(e)) if !self.interrupt.is_raised() => {
                // A run looks stale only after several missed beats, so one failure costs nothing; the next try comes
                // a full interval later rather than on every poll.
                self.human.log(&format!("heartbeat failed: {e}"));
                self.last_write = self.clocks.monotonic();
                Ok(())
            }
            other => other,
        }
    }

    /// Sleep that many seconds, unless the human has asked the run to stop, before or meanwhile.
    fn pause(&self, seconds: u64) -> Outcome<()> {
        self.check_interrupt()?;
        self.clocks.sleep(seconds as f64);
        self.check_interrupt()
    }

    fn check_interrupt(&self) -> Outcome<()> {
        if self.interrupt.is_raised() {
            return Err(Stopped::Interrupted);
        }
        Ok(())
    }

    /// The wall clock's time to the second, as state.json records times.
    fn timestamp(&self) -> String {
        iso_utc(self.clocks.wall().floor() as i64)
    }
}

/// Which of the steps of a done run's ending Workflow::end_done_run takes: all of them for a run that has just
/// finished its last phase (WHOLE), those a stop after it was saved as done left undone for a resumed one
/// (Workflow::ending_left).
struct Ending {
    /// Give each agent turn its tokens (Workflow::count_tokens).
    count_tokens: bool,
    /// Write summary.md.
    write_summary: bool,
    /// Write metrics.json.
    write_metrics: bool,
    /// Release the run this orchestrator owns, then tell the human the run finished and close a published run's
    /// workspace.
    release: bool,
}

impl Ending {
    const WHOLE: Ending = Ending { count_tokens: true, write_summary: true, write_metrics: true, release: true };
}

/// The label of the herdr workspace of the run run_id, for task: its key and its task, the first 40 characters, so
/// that the human can tell a run's workspace in herdr's list. serve labels the workspace it starts a run in so too.
pub fn workspace_label(run_id: &str, task: &str) -> String {
    format!("{} {task}", crate::state::run_key(run_id)).chars().take(40).collect()
}
