//! What `list`, `list --json` and `show` print of a project's runs. Each shows a run as far as its state reads
//! (SavedRun::shown), so a state saved by an older version, or one whose workflow this orchestrator cannot load, still
//! lists.

use indexmap::IndexMap;

use super::resume_check::resumable;
use super::{resume_command, Connected};
use crate::constants::APPROVE;
use crate::error::OrchestratorError;
use crate::handoff::parse_verdict;
use crate::local_gate::LOCAL_GATE;
use crate::pipeline::{workflow_shape, Pipeline, Step, QUALITY};
use crate::runs::{Health, ThisMachine};
use crate::state::{
    find_run, judged_by_several, run_dir, AgentRecord, Backend, RunState, SavedRun, StepVerdict, TurnRecord, Turns,
    NOTHING_TO_PUBLISH,
};
use crate::summary::turn_lines;
use serde::Serialize;
use serde_json::Value;

/// The version of the run records `list --json` prints, raised when a key is removed or changes its meaning.
pub const RUN_RECORD_VERSION: i64 = 1;

/// The waiting reason, and outcome, of a running run whose turn waits on a BLOCKED report (WaitReason::Blocked).
pub const BLOCKED_STATUS: &str = "blocked";

/// A run's saved state as `list --json` prints it and serve gives it, and as the run list and `show` word it
/// (run_rows). Built by run_record. It serializes to an object with one key per field, in field order; None is null,
/// meaning the key does not apply to the run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunRecord {
    /// The run's id.
    pub id: String,
    /// The run's key, as resume takes it.
    pub key: String,
    /// The whole task.
    pub task: String,
    /// The workflow's name.
    pub workflow: String,
    /// The workflow's steps, in order; None when the workflow cannot be loaded.
    pub steps: Option<Vec<StepRef>>,
    /// Whether the run opens a pull request; false for a --no-pr run.
    pub pull_request: bool,
    /// As saved.
    pub phase: String,
    /// As saved.
    pub round: u32,
    /// The quality round while the run is in the quality loop or its gated step, once it has one (never 0).
    pub quality_round: Option<u32>,
    /// The current step's role; None in quality, publish or done, or when the workflow cannot be loaded.
    pub role: Option<String>,
    /// role's pane; None where role is, and before the role's agent is started.
    pub pane: Option<String>,
    /// Each role's agent the run has started, in the order they were started; empty before the first.
    pub agents: Vec<AgentRef>,
    /// Where its agents run: an "acp" run's agents are read from their journals and answered through its socket
    /// (crate::acp), not through herdr.
    pub backend: Backend,
    /// RunState::health's: None for a run nothing drives. Serialized as two keys, health (Health::name) and
    /// health_reason (Health::why).
    #[serde(flatten, serialize_with = "health_keys")]
    pub health: Option<Health>,
    /// Whole seconds since state.json was written.
    pub heartbeat_age: u64,
    /// What the run waits on the human for, else None.
    pub waiting: Option<Waiting>,
    /// The run's, as saved.
    pub verdict: Option<String>,
    /// Each verdict a verdict step gave, as saved (RunState::verdicts); empty for a run saved before them.
    pub verdicts: Vec<StepVerdict>,
    /// As saved; but for a state.json this engine does not read, why not (SavedRun::problem), which tells what to do
    /// with the run.
    pub error: Option<String>,
    /// As saved.
    pub closed: Option<String>,
    /// As saved.
    pub branch: Option<String>,
    /// As saved.
    pub worktree: Option<String>,
    /// The run's pull request; None without one.
    pub pr: Option<PrRecord>,
    /// Whether the run succeeded with nothing tracked changed since its base, and so opened no pull request
    /// (RunState::nothing_to_publish).
    pub nothing_to_publish: bool,
    /// The turn records as saved; empty for a run saved before them.
    pub turns: Turns,
    /// The command that resumes a stale run that is not closed; None otherwise, and for an "acp" run, which the app
    /// resumes and the CLI refuses.
    pub resume_command: Option<String>,
    /// Whether a resume would take the run up and have something left to do (crate::cli::resume_check::resumable): a
    /// stale run, one released as interrupted or with a failure, or a done one its orchestrator did not release or that
    /// lacks its summary or metrics; an "acp" run is resumed with serve's POST /resume.
    pub resumable: bool,
}

/// One step of a run record's workflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StepRef {
    pub id: String,
    pub role: String,
    /// The workflow's label for the role.
    pub role_label: String,
    /// The step a CHANGES_REQUESTED sends the run back to; None for a step that gives no verdict.
    pub loop_to: Option<String>,
    /// Whether the quality loop follows, or followed, the step's turns in this run: the step is gated, and the run has
    /// the gate (from start or from resume) or has a quality turn saved, so false for a gate resume turned off before
    /// its first check.
    pub quality_gated: bool,
}

/// One agent a run has started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentRef {
    pub role: String,
    /// Its herdr name.
    pub name: String,
    /// Its harness.
    pub kind: String,
}

/// What a run waits on the human for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Waiting {
    pub reason: WaitReason,
    /// Blocked: the BLOCKED report's question. Interview: the role. Stale: the resume command, None for an "acp" run.
    pub detail: Option<String>,
}

/// Why a run waits on the human, serialized lowercase. When several hold, the first listed is the run's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitReason {
    /// A running run's turn waits on a BLOCKED report (BLOCKED_STATUS).
    Blocked,
    /// A running run's current step is human-paced and its last turn has not ended.
    Interview,
    /// A stale run that is not closed (crate::runs::STALE).
    Stale,
}

/// A run record's pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PrRecord {
    pub url: String,
    /// gh's OPEN, CLOSED or MERGED; None until fill_pr_states sets it or error.
    pub state: Option<String>,
    /// Why fill_pr_states could not look the state up; None otherwise.
    pub error: Option<String>,
}

impl RunRecord {
    /// The record as the object `list --json` prints and serve sends, its keys in field order.
    pub fn to_json(&self) -> Value {
        // Strings, numbers, booleans and records of them serialize without fail.
        serde_json::to_value(self).expect("a run record serializes")
    }

    /// The record as it is compared to tell whether the run changed: with what grows on every read while nothing
    /// happens cleared, heartbeat_age to 0 and the health's reason to "". Not for printing.
    pub fn settled(&self) -> RunRecord {
        let health = self.health.as_ref().map(|h| match h {
            Health::Running(_) => Health::Running(String::new()),
            Health::Stale(_) => Health::Stale(String::new()),
        });
        RunRecord { heartbeat_age: 0, health, ..self.clone() }
    }
}

/// RunRecord::health as its two keys.
fn health_keys<S: serde::Serializer>(health: &Option<Health>, to: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Keys<'h> {
        health: Option<&'static str>,
        health_reason: Option<&'h str>,
    }
    Keys { health: health.as_ref().map(Health::name), health_reason: health.as_ref().map(Health::why) }.serialize(to)
}

/// The record of run, as Host::run_states read it, shown as far as it reads (SavedRun::shown). machine is
/// RunState::health's, and target the --machine and --cwd flags resume_command repeats. Reads nothing beyond run.
pub fn run_record(run: &SavedRun, machine: ThisMachine, target: &[String]) -> RunRecord {
    let s = run.shown();
    let unreadable = run.problem();
    let pipeline = s.pipeline().ok();
    let step = pipeline.as_ref().and_then(|p| p.step(&s.phase));
    let role = step.map(|st| st.role.clone());
    let health = run.health(machine);
    let stale = matches!(health, Some(Health::Stale(_)));
    let running = matches!(health, Some(Health::Running(_)));
    // resume refuses a run whose state.json it cannot read, and the CLI an app-managed one (RunState::app_managed).
    let resumable_stale = stale && !s.is_closed() && unreadable.is_none();
    let resume_command = (resumable_stale && !s.app_managed()).then(|| resume_command(&s.run_id, target));
    let last_turn = s.turns.iter().rev().find(|t| t.step == s.phase);
    let blocked = s.blocked.as_ref().filter(|b| !b.is_empty());
    let waiting = if running && blocked.is_some() {
        // The Workflow saves the question as a string; anything else reads as none.
        let question = blocked.and_then(|b| b.get("question")).and_then(Value::as_str).map(String::from);
        Some(Waiting { reason: WaitReason::Blocked, detail: question })
    } else if running && step.is_some_and(|st| st.human_paced) && last_turn.is_some_and(TurnRecord::under_way) {
        Some(Waiting { reason: WaitReason::Interview, detail: role.clone() })
    } else {
        resumable_stale.then(|| Waiting { reason: WaitReason::Stale, detail: resume_command.clone() })
    };
    // A workflow this orchestrator lacks has no gated step known, so only phase quality has a quality round then.
    let in_quality = pipeline.as_ref().map_or(s.phase == QUALITY, |p| s.in_quality(p));
    let pane = role.as_deref().and_then(|role| Some(s.agents.get(role)?.pane.clone()));
    let pr = s.pr_url.as_deref().filter(|url| !url.is_empty());
    RunRecord {
        id: s.run_id.clone(),
        key: s.key().to_string(),
        task: s.task.clone(),
        workflow: s.workflow.clone(),
        steps: pipeline.as_ref().map(|p| step_refs(p, s)),
        pull_request: s.pull_request,
        phase: s.phase.clone(),
        round: s.round,
        quality_round: (in_quality && s.quality_round != 0).then_some(s.quality_round),
        role,
        pane,
        agents: agent_refs(&s.agents),
        backend: s.backend,
        health,
        heartbeat_age: run.age(),
        waiting,
        verdict: s.verdict.clone(),
        verdicts: s.verdicts.clone(),
        error: unreadable.map(String::from).or_else(|| s.error.clone()),
        closed: s.closed.clone(),
        branch: s.branch.clone(),
        worktree: s.worktree.clone(),
        pr: pr.map(|url| PrRecord { url: url.to_string(), state: None, error: None }),
        nothing_to_publish: s.nothing_to_publish,
        turns: s.turns.clone(),
        resume_command,
        resumable: resumable(run, machine).is_ok(),
    }
}

/// run_record's steps, of s's workflow p.
fn step_refs(p: &Pipeline, s: &RunState) -> Vec<StepRef> {
    // A workflow has one gated step at most (Pipeline::uniqueness_problem), so any quality turn checked that step.
    let gate_checks = s.has_quality_gate() || s.turns.iter().any(|t| t.step == QUALITY);
    let step = |st: &Step| StepRef {
        id: st.id.clone(),
        role: st.role.clone(),
        role_label: p.roles().get(&st.role).expect("Pipeline::new refuses a step whose role has no label").clone(),
        loop_to: st.loop_to.clone(),
        quality_gated: st.quality_gated && gate_checks,
    };
    p.steps().iter().map(step).collect()
}

/// run_record's agents, of a state's agents, by role.
fn agent_refs(agents: &IndexMap<String, AgentRecord>) -> Vec<AgentRef> {
    let started = agents.iter().filter(|(_, agent)| !agent.name.is_empty());
    started.map(|(role, a)| AgentRef { role: role.clone(), name: a.name.clone(), kind: a.harness.clone() }).collect()
}

/// Set pr.state of each run record with a pull request to gh's OPEN, CLOSED or MERGED, one gh pr view each, run in
/// the project directory cwd on world's host; a run whose lookup fails gets the failure's message in pr.error
/// instead. Errors: a lookup failed while world's interrupt is raised, which stops the listing.
pub fn fill_pr_states(world: &Connected, cwd: &str, records: &mut [RunRecord]) -> Result<(), OrchestratorError> {
    for pr in records.iter_mut().filter_map(|record| record.pr.as_mut()) {
        // GhError::Unusable too: each run says why its state is missing.
        match world.with.host.pull_request(cwd, &pr.url) {
            Ok(found) => pr.state = Some(found.state),
            Err(e) => {
                let e = OrchestratorError::from(e);
                if world.with.interrupt.is_raised() {
                    return Err(e);
                }
                pr.error = Some(e.0);
            }
        }
    }
    Ok(())
}

/// Each run as `list` shows it: its id, phase, round, outcome and the first 100 characters of its task, from its
/// record.
pub fn run_rows(runs: &[SavedRun], machine: ThisMachine, target: &[String]) -> Vec<[String; 5]> {
    runs.iter()
        .map(|run| {
            let record = run_record(run, machine, target);
            let task: String = record.task.chars().take(100).collect();
            [record.id.clone(), record.phase.clone(), run_round(&record), run_outcome(&record), task]
        })
        .collect()
}

/// A run record's outcome as `list` words it: its error, verdict (as outcome_verdict words it), waiting reason or
/// health, then what it has besides, its closing, pull request and a workflow other than the default.
pub fn run_outcome(record: &RunRecord) -> String {
    // A field saved as "" reads as absent.
    let set = |text: &Option<String>| text.clone().filter(|t| !t.is_empty());
    let closed = set(&record.closed);
    let mut outcome = if let Some(error) = set(&record.error) {
        format!("error: {error}")
    } else if let Some(health) = &record.health {
        match &record.waiting {
            // Stale by its heartbeat, but resume refuses it, so neither its staleness nor the command is worth showing.
            _ if closed.is_some() => String::new(),
            Some(Waiting { reason: WaitReason::Blocked, detail }) => {
                let mut blocked = format!("{BLOCKED_STATUS}: {}", detail.as_deref().unwrap_or_default());
                if let Some(pane) = set(&record.pane) {
                    blocked += &format!(" (pane {pane})");
                }
                blocked
            }
            _ => {
                let mut line = format!("{}: {}", health.name(), health.why());
                if let Some(command) = &record.resume_command {
                    line += &format!("; resume: {command}");
                }
                line
            }
        }
    } else {
        outcome_verdict(record)
    };
    if let Some(closed) = closed {
        let closed = format!("closed: {closed}");
        outcome = if outcome.is_empty() { closed } else { format!("{outcome}; {closed}") };
    }
    if let Some(pr) = &record.pr {
        outcome += &format!("  {}", pr.url);
    }
    if record.workflow != "default" {
        outcome += &format!("  [{} workflow]", record.workflow);
    }
    outcome
}

/// A run record's verdict as its outcome words it: "" without one. A verdict other than APPROVE, of a run whose
/// verdicts came from several verdict steps (judged_by_several), names the step that gave it last, as in
/// "CHANGES_REQUESTED by accept".
fn outcome_verdict(record: &RunRecord) -> String {
    let verdict = record.verdict.clone().unwrap_or_default();
    match record.verdicts.last() {
        Some(judge) if !verdict.is_empty() && verdict != APPROVE && judged_by_several(&record.verdicts) => {
            format!("{verdict} by {}", judge.step)
        }
        _ => verdict,
    }
}

/// A run record's round, and its quality round when it has one.
pub fn run_round(record: &RunRecord) -> String {
    let mut round = format!("round {}", record.round);
    if let Some(q) = record.quality_round {
        round += &format!(" q{q}");
    }
    round
}

/// A run's line as `list` and `show` print it.
fn run_line([run_id, phase, round, outcome, _]: &[String; 5]) -> String {
    format!("{run_id}  {phase:<7}  {round}  {outcome}")
}

/// Print each of runs, as Host::run_states gives them, on world's standard output: its line and its task under it,
/// or "No runs." without any. target is resume_command's.
pub fn print_runs(world: &Connected, runs: &[SavedRun], target: &[String]) {
    if runs.is_empty() {
        world.say("No runs.");
        return;
    }
    for row in run_rows(runs, world.with.machine, target) {
        world.say(&run_line(&row));
        world.say(&format!("    {}", row[4]));
    }
}

/// The quality gate of the run s, as show prints it: its gate, the local one its resume gives a run a release before
/// 0.72.0 started on a Jenkins job (cli::start::resumed_state), or that resume turned it off; None for a run that never
/// had one.
fn quality_gate_line(s: &RunState) -> Option<String> {
    if s.quality_gate_disabled {
        return Some("off (turned off by resume)".to_string());
    }
    if !s.has_quality_gate() {
        return None;
    }
    let job = s.quality_job.as_deref()?;
    if job == LOCAL_GATE {
        return Some(job.to_string());
    }
    Some(format!("{LOCAL_GATE} once resumed (Jenkins job {job}, removed in 0.72.0)"))
}

/// Print one run of the project at cwd on world's standard output, by its id or its key run_ref: its list line and
/// whole task, workflow, branch, agents, turns, verdicts, files and last review, as far as its state reads
/// (SavedRun::shown). target is resume_command's. Errors: no run or several match run_ref, and what world's host
/// gives reading the runs and the run's directory.
pub fn show_run(world: &Connected, cwd: &str, run_ref: &str, target: &[String]) -> Result<(), OrchestratorError> {
    let host = world.with.host;
    let runs = host.run_states(cwd)?;
    let run = find_run(&runs, run_ref, cwd)?;
    let row = &run_rows(std::slice::from_ref(run), world.with.machine, target)[0];
    let s = run.shown();
    world.say(&run_line(row));
    world.say(&format!("    {}", s.task));
    // The directory state.json was found in, which outlives a project moved since the run saved its cwd.
    let dir = run_dir(cwd, &row[0]);
    let pipeline = s.pipeline().ok();
    world.say("");
    match &pipeline {
        Some(p) => world.say(&format!("workflow   {}: {}", s.workflow, workflow_shape(p))),
        None => world.say(&format!("workflow   {}: this orchestrator cannot load it", s.workflow)),
    }
    let fields = [
        ("worktree", s.worktree.clone()),
        ("branch", s.branch.clone()),
        ("base", s.base_branch.clone()),
        (
            "pr",
            s.pr_url
                .clone()
                .filter(|url| !url.is_empty())
                .or_else(|| s.nothing_to_publish.then(|| NOTHING_TO_PUBLISH.into())),
        ),
        ("cleanup", s.branch_cleanup.clone()),
        ("quality", quality_gate_line(s)),
        ("conflicts", Some(s.conflicts.join(", "))),
        ("workspace", Some(s.workspace_id.clone())),
        ("closed", s.closed.clone()),
    ];
    for (label, value) in fields {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            world.say(&format!("{label:<9}  {value}"));
        }
    }
    let width = s.agents.keys().map(|role| role.chars().count()).max().unwrap_or(0);
    let agent_lines = s.agents.iter().map(|(role, a)| format!("{role:<width$}  {:<8}  {}", a.harness, a.pane));
    print_section(world, "agents", agent_lines.collect());
    let turns = &s.turns;
    let zone = world.with.clocks.zone();
    print_section(world, &format!("turns, started in {}", zone_name(&zone, turns)), turn_lines(turns, &zone));
    print_section(world, "verdicts", verdict_lines(&s.verdicts));
    print_section(world, "files", host.run_files(&dir)?.into_iter().map(|f| f.path).collect());
    let review = match &pipeline {
        Some(p) => last_review(world, &dir, p, s.round)?,
        None => None,
    };
    if let Some((path, text)) = review {
        world.say(&format!("\nlast review: {path}"));
        world.print(&text);
        if !text.ends_with('\n') {
            world.print("\n");
        }
    }
    Ok(())
}

/// Print a heading and its lines, indented, on world's standard output; nothing when there are no lines.
fn print_section(world: &Connected, title: &str, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    world.say(&format!("\n{title}"));
    for line in lines {
        world.say(&format!("    {line}"));
    }
}

/// zone as show's turns heading names it: its IANA name, such as Europe/Vienna; for a zone without one (a POSIX TZ
/// string, a copied /etc/localtime), the offsets from UTC it had at the starts of turns, each once in their order,
/// such as UTC+02:00 and UTC+01:00, since the offset may change between turns; "local time" when no start is known.
fn zone_name(zone: &jiff::tz::TimeZone, turns: &[TurnRecord]) -> String {
    if let Some(name) = zone.iana_name() {
        return name.to_string();
    }
    let mut offsets: Vec<String> = Vec::new();
    let starts = turns.iter().filter_map(|t| t.started_at.as_deref()?.parse::<jiff::Timestamp>().ok());
    for at in starts {
        let offset = format!("UTC{}", at.to_zoned(zone.clone()).strftime("%:z"));
        if !offsets.contains(&offset) {
            offsets.push(offset);
        }
    }
    if offsets.is_empty() {
        "local time".into()
    } else {
        offsets.join(" and ")
    }
}

/// show's line per verdict a verdict step gave, of a state's verdicts, such as "round 1  review  CHANGES_REQUESTED";
/// none for a run saved before them.
fn verdict_lines(verdicts: &[StepVerdict]) -> Vec<String> {
    let width = verdicts.iter().map(|v| v.step.chars().count()).max().unwrap_or(0);
    verdicts.iter().map(|v| format!("round {}  {:<width$}  {}", v.round, v.step, v.verdict)).collect()
}

/// The path of the last verdict file written in run_dir on world's host, up to round: of the latest round that has
/// one, the file of its last verdict step that has one; and its text without the VERDICT line. None without one, or
/// for a workflow without a verdict step. Errors: a file cannot be read.
fn last_review(
    world: &Connected,
    run_dir: &str,
    pipeline: &Pipeline,
    round: u32,
) -> Result<Option<(String, String)>, OrchestratorError> {
    let newest_first = |n| pipeline.verdict_files(run_dir, n).into_iter().rev().map(|(_, path)| path);
    for path in (1..=round).rev().flat_map(newest_first) {
        let Some(text) = world.with.host.read(&path)? else { continue };
        if parse_verdict(&text).is_none() {
            return Ok(Some((path, text)));
        }
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let first = lines.iter().position(|line| !line.trim().is_empty());
        let without: String = lines.iter().enumerate().filter(|(i, _)| Some(*i) != first).map(|(_, l)| *l).collect();
        return Ok(Some((path, without)));
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
