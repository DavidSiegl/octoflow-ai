//! What `list`, `list --json` and `show` print of a project's runs, as orchestrator.py's run_record, run_rows,
//! run_outcome, print_runs, fill_pr_states and show_run word it. Each reads the runs as Host::run_states loads them,
//! so a state saved by an older version, or one whose workflow this orchestrator cannot load, still lists.

use super::start::find_run;
use super::{resume_command, Connected};
use crate::constants::{APPROVE, DEFAULT_AGENT, RUNS_DIR};
use crate::error::OrchestratorError;
use crate::handoff::parse_verdict;
use crate::pipeline::{workflow_shape, Pipeline, QUALITY};
use crate::json::{object, Object, ValueExt};
use crate::runs::{run_health, RUNNING, STALE};
use crate::state::{format_problem, judged_by_several, StepVerdict};
use crate::summary::turn_lines;
use crate::workflow_file::saved_pipeline;
use serde_json::Value;

/// The version of the run records `list --json` prints, raised when a key is removed or changes its meaning.
pub const RUN_RECORD_VERSION: i64 = 1;

/// The waiting reason, and outcome, of a running run whose turn waits on a BLOCKED report (run_record).
pub const BLOCKED_STATUS: &str = "blocked";

/// A run's saved state as `list --json` prints it, and as the run list and `show` word it (run_rows).
///
/// s is the state as Host::run_states loaded it, age the seconds since its state.json was written; local_host and
/// pid_alive are run_health's, and target the --machine and --cwd flags resume_command repeats. Reads nothing beyond
/// s. The record has every key below, in this order; null means the key does not apply to the run:
/// - id, key, task (the whole task), workflow: the run's id, its key (as resume takes it), task and workflow.
/// - steps: the workflow's steps, each {"id", "role"}, in order; null when the workflow cannot be loaded.
/// - phase, round: as saved. quality_round: the quality round while the run is in the quality loop or its gated
///   step, once it has one; null otherwise.
/// - role, pane: the current step's role and that role's pane; null in quality, publish or done, or when the
///   workflow cannot be loaded, and pane also before the role's agent is started.
/// - agents: each role's agent the run has started, {"role", "name", "kind"}, in the order they were started: its
///   herdr name and harness, "claude" for one saved before there were others; [] before the first.
/// - health, health_reason: RUNNING or STALE and why, as run_health has them; null for a finished or failed run.
///   heartbeat_age is age.
/// - waiting: what the run waits on the human for, {"reason", "detail"}, else null. Reasons, first match wins:
///   "blocked" (detail: the question) while a running run's turn waits on a BLOCKED report; "interview" (detail: the
///   role) while a running run's current step is human-paced and its last turn has not ended; "stale" (detail: the
///   resume command) for a stale run that is not closed.
/// - verdict: the run's, as saved. verdicts: each verdict a verdict step gave, {"round", "step", "verdict"}, as
///   saved (RunState::verdicts); [] for a run saved before them.
/// - error, closed, branch, worktree: as saved, but error, for a state.json this engine does not read, why not
///   (crate::state::format_problem), which tells what to do with the run.
/// - pr: null without a pull request, else {"url", "state", "error"}; state and error are null here, and
///   fill_pr_states sets one of them.
/// - turns: the turn records as saved, [] for a run saved before them.
/// - resume_command: the command that resumes a stale run that is not closed; null otherwise.
pub fn run_record(s: &Value, age: u64, local_host: &str, pid_alive: &dyn Fn(i64) -> bool, target: &[String]) -> Object {
    let empty = Object::new();
    let saved = s.as_object().unwrap_or(&empty);
    let unreadable = format_problem(saved);
    let field = |key: &str| saved.get(key).cloned().unwrap_or(Value::Null);
    let truthy = |key: &str| saved.get(key).filter(|v| v.is_set());
    let run_id = field("run_id").text();
    let phase = field("phase");
    let pipeline = saved_pipeline(saved);
    let step = pipeline.as_ref().and_then(|p| phase.as_str().and_then(|phase| p.step(phase)));
    let role = step.map(|st| st.role.clone());
    let health = run_health(s, age, local_host, pid_alive);
    let stale = health.as_ref().is_some_and(|(h, _)| *h == STALE);
    let running = health.as_ref().is_some_and(|(h, _)| *h == RUNNING);
    // resume refuses a run whose state.json it cannot read.
    let resume = (stale && truthy("closed").is_none() && unreadable.is_none()).then(|| resume_command(&run_id, target));
    // A workflow this orchestrator lacks has no gated step known, so only phase quality has a quality round then.
    let gated = pipeline.as_ref().and_then(Pipeline::gated).map_or(QUALITY.to_string(), |g| g.id.clone());
    let turns = truthy("turns").cloned().unwrap_or(Value::Array(vec![]));
    let last_turn = turns.as_array().and_then(|turns| turns.iter().rev().find(|t| t.get("step") == Some(&phase)));
    let blocked = truthy("blocked");
    let waiting = if running && blocked.is_some() {
        let question = blocked.and_then(|b| b.get("question")).cloned().unwrap_or(Value::Null);
        Some((BLOCKED_STATUS, question))
    } else if running
        && step.is_some_and(|st| st.human_paced)
        && last_turn.is_some_and(|t| t.get("ended_at").is_none_or(Value::is_null))
    {
        Some(("interview", role.clone().map_or(Value::Null, Value::from)))
    } else {
        resume.clone().map(|command| (STALE, Value::from(command)))
    };
    let in_quality = phase.as_str().is_some_and(|phase| phase == QUALITY || phase == gated);
    let pane = role.as_deref().and_then(|role| truthy("agents")?.get(role)?.get("pane").cloned());
    let optional = |text: Option<String>| text.map_or(Value::Null, Value::from);
    let mut record = Object::new();
    record.insert("id".into(), field("run_id"));
    record.insert("key".into(), Value::from(run_id.rsplit('-').next().unwrap_or(&run_id)));
    record.insert("task".into(), field("task"));
    record.insert("workflow".into(), truthy("workflow").cloned().unwrap_or(Value::from("default")));
    record.insert(
        "steps".into(),
        pipeline.as_ref().map_or(Value::Null, |p| {
            Value::Array(
                p.steps()
                    .iter()
                    .map(|st| Value::Object(object([("id", Value::from(st.id.as_str())), ("role", Value::from(st.role.as_str()))])))
                    .collect(),
            )
        }),
    );
    record.insert("phase".into(), phase.clone());
    record.insert("round".into(), field("round"));
    record.insert("quality_round".into(), truthy("quality_round").filter(|_| in_quality).cloned().unwrap_or(Value::Null));
    record.insert("role".into(), optional(role));
    record.insert("pane".into(), pane.unwrap_or(Value::Null));
    record.insert("agents".into(), agent_refs(truthy("agents")));
    record.insert("health".into(), optional(health.as_ref().map(|(h, _)| h.to_string())));
    record.insert("health_reason".into(), optional(health.map(|(_, why)| why)));
    record.insert("heartbeat_age".into(), Value::from(age as i64));
    record.insert(
        "waiting".into(),
        waiting.map_or(Value::Null, |(reason, detail)| Value::Object(object([("reason", Value::from(reason)), ("detail", detail)]))),
    );
    record.insert("verdict".into(), field("verdict"));
    record.insert("verdicts".into(), truthy("verdicts").cloned().unwrap_or(Value::Array(vec![])));
    for key in ["error", "closed", "branch", "worktree"] {
        record.insert(key.into(), field(key));
    }
    if let Some(problem) = unreadable {
        record.insert("error".into(), Value::from(problem));
    }
    record.insert(
        "pr".into(),
        truthy("pr_url").map_or(Value::Null, |url| {
            Value::Object(object([("url", url.clone()), ("state", Value::Null), ("error", Value::Null)]))
        }),
    );
    record.insert("turns".into(), turns);
    record.insert("resume_command".into(), optional(resume));
    record
}

/// run_record's agents, of the state's saved agents, by role.
fn agent_refs(agents: Option<&Value>) -> Value {
    let agents = agents.and_then(Value::as_object).into_iter().flat_map(|a| a.iter());
    let started = agents.filter_map(|(role, agent)| {
        let name = agent.get("name").and_then(Value::as_str).filter(|n| !n.is_empty())?;
        let kind = agent.get("kind").and_then(Value::as_str).unwrap_or("claude");
        Some(Value::Object(object([("role", Value::from(role.as_str())), ("name", Value::from(name)), ("kind", Value::from(kind))])))
    });
    Value::Array(started.collect())
}

/// Set pr.state of each run record with a pull request to gh's OPEN, CLOSED or MERGED, one gh pr view each, run in
/// the project directory cwd on world's host; a run whose lookup fails gets the failure's message in pr.error
/// instead. Errors: a lookup failed while world's interrupt is raised, which stops the listing.
pub fn fill_pr_states(world: &Connected, cwd: &str, records: &mut [Object]) -> Result<(), OrchestratorError> {
    for record in records {
        let Some(Value::Object(pr)) = record.get_mut("pr") else { continue };
        let url = pr.get("url").map(Value::text).unwrap_or_default();
        // GhError::Unusable too: each run says why its state is missing.
        match world.with.host.pull_request(cwd, &url) {
            Ok(found) => {
                pr.insert("state".into(), Value::from(found.state));
            }
            Err(e) => {
                let e = OrchestratorError::from(e);
                if world.with.interrupt.is_raised() {
                    return Err(e);
                }
                pr.insert("error".into(), Value::from(e.0));
            }
        }
    }
    Ok(())
}

/// Each run as `list` shows it: its id, phase, round, outcome and the first 100 characters of its task, from its
/// record.
pub fn run_rows(
    runs: &[(u64, Value)],
    local_host: &str,
    pid_alive: &dyn Fn(i64) -> bool,
    target: &[String],
) -> Vec<[String; 5]> {
    runs.iter()
        .map(|(age, s)| {
            let record = run_record(s, *age, local_host, pid_alive, target);
            let text = |key: &str| record.get(key).map_or("None".to_string(), Value::text);
            let task: String = text("task").chars().take(100).collect();
            [text("id"), text("phase"), run_round(&record), run_outcome(&record), task]
        })
        .collect()
}

/// A run record's outcome as `list` words it: its error, verdict (as outcome_verdict words it), waiting reason or
/// health, then what it has besides, its closing, pull request and a workflow other than the default.
pub fn run_outcome(record: &Object) -> String {
    let truthy = |key: &str| record.get(key).filter(|v| v.is_set());
    let text = |value: Option<&Value>| value.map_or("None".to_string(), Value::text);
    let waiting = truthy("waiting");
    let reason = waiting.and_then(|w| w.get("reason")).and_then(Value::as_str);
    let mut outcome = if let Some(error) = truthy("error") {
        format!("error: {}", error.text())
    } else if record.get("health").is_none_or(Value::is_null) {
        outcome_verdict(record)
    } else if truthy("closed").is_some() {
        // Stale by its heartbeat, but resume refuses it, so neither its staleness nor the command is worth showing.
        String::new()
    } else if reason == Some(BLOCKED_STATUS) {
        let mut blocked = format!("{BLOCKED_STATUS}: {}", text(waiting.and_then(|w| w.get("detail"))));
        if let Some(pane) = truthy("pane") {
            blocked += &format!(" (pane {})", pane.text());
        }
        blocked
    } else {
        let mut health = format!("{}: {}", text(record.get("health")), text(record.get("health_reason")));
        if let Some(command) = truthy("resume_command") {
            health += &format!("; resume: {}", command.text());
        }
        health
    };
    if let Some(closed) = truthy("closed") {
        let closed = format!("closed: {}", closed.text());
        outcome = if outcome.is_empty() { closed } else { format!("{outcome}; {closed}") };
    }
    if let Some(pr) = truthy("pr") {
        outcome += &format!("  {}", text(pr.get("url")));
    }
    if record.get("workflow").and_then(Value::as_str) != Some("default") {
        outcome += &format!("  [{} workflow]", text(record.get("workflow")));
    }
    outcome
}

/// A run record's verdict as its outcome words it: "" without one. A verdict other than APPROVE, of a run whose
/// verdicts came from several verdict steps (judged_by_several), names the step that gave it last, as in
/// "CHANGES_REQUESTED by accept".
fn outcome_verdict(record: &Object) -> String {
    let truthy = |key: &str| record.get(key).filter(|v| v.is_set());
    let verdict = truthy("verdict").map_or(String::new(), Value::text);
    // A record that is not a list of verdicts reads as none, as a run saved before them.
    let verdicts: Vec<StepVerdict> =
        truthy("verdicts").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    match verdicts.last() {
        Some(judge) if !verdict.is_empty() && verdict != APPROVE && judged_by_several(&verdicts) => {
            format!("{verdict} by {}", judge.step)
        }
        _ => verdict,
    }
}

/// A run record's round, and its quality round when it has one.
pub fn run_round(record: &Object) -> String {
    let mut round = format!("round {}", record.get("round").map_or("None".to_string(), Value::text));
    if let Some(q) = record.get("quality_round").filter(|q| q.is_set()) {
        round += &format!(" q{}", q.text());
    }
    round
}

/// A run's line as `list` and `show` print it.
fn run_line([run_id, phase, round, outcome, _]: &[String; 5]) -> String {
    format!("{run_id}  {phase:<7}  {round}  {outcome}")
}

/// Print each of runs, as Host::run_states gives them, on world's standard output: its line and its task under it,
/// or "No runs." without any. target is resume_command's.
pub fn print_runs(world: &Connected, runs: &[(u64, Value)], target: &[String]) {
    if runs.is_empty() {
        world.say("No runs.");
        return;
    }
    for row in run_rows(runs, world.local_host, world.with.pid_alive, target) {
        world.say(&run_line(&row));
        world.say(&format!("    {}", row[4]));
    }
}

/// The quality gate of the run saved as saved, as show prints it: its job, or that resume turned it off; None for a
/// run that never had one.
fn saved_quality_gate(saved: &Object) -> Option<String> {
    if saved.get("quality_gate_disabled").is_some_and(Value::is_set) {
        return Some("off (turned off by resume)".to_string());
    }
    saved.get("quality_job").filter(|j| j.is_set()).map(Value::text)
}

/// Print one run of the project at cwd on world's standard output, by its id or its key run_ref: its list line and
/// whole task, workflow, branch, agents, turns, verdicts, files and last review. target is resume_command's. Errors:
/// no run or several match run_ref, and what world's host gives reading the runs and the run's directory.
pub fn show_run(world: &Connected, cwd: &str, run_ref: &str, target: &[String]) -> Result<(), OrchestratorError> {
    let host = world.with.host;
    let runs = host.run_states(cwd)?;
    let (age, s) = find_run(&runs, run_ref, cwd)?;
    let row = &run_rows(&[(*age, s.clone())], world.local_host, world.with.pid_alive, target)[0];
    let empty = Object::new();
    let saved = s.as_object().unwrap_or(&empty);
    let truthy = |key: &str| saved.get(key).filter(|v| v.is_set());
    world.say(&run_line(row));
    world.say(&format!("    {}", saved.get("task").map_or("None".to_string(), Value::text)));
    // The directory state.json was found in, which outlives a project moved since the run saved its cwd.
    let run_dir = format!("{cwd}/{RUNS_DIR}/{}", row[0]);
    let pipeline = saved_pipeline(saved);
    let workflow = truthy("workflow").map_or("default".to_string(), Value::text);
    world.say("");
    match &pipeline {
        Some(p) => world.say(&format!("workflow   {workflow}: {}", workflow_shape(p))),
        None => world.say(&format!("workflow   {workflow}: this orchestrator cannot load it")),
    }
    let conflicts = truthy("conflicts")
        .and_then(Value::as_array)
        .map(|c| c.iter().map(Value::text).collect::<Vec<_>>().join(", "));
    let fields = [
        ("worktree", truthy("worktree").map(Value::text)),
        ("branch", truthy("branch").map(Value::text)),
        ("base", truthy("base_branch").map(Value::text)),
        ("pr", truthy("pr_url").map(Value::text)),
        ("cleanup", truthy("branch_cleanup").map(Value::text)),
        ("quality", saved_quality_gate(saved)),
        ("conflicts", conflicts),
        ("workspace", truthy("workspace_id").map(Value::text)),
        ("closed", truthy("closed").map(Value::text)),
    ];
    for (label, value) in fields {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            world.say(&format!("{label:<9}  {value}"));
        }
    }
    let agents = truthy("agents").and_then(Value::as_object).cloned().unwrap_or_default();
    let width = agents.keys().map(|role| role.chars().count()).max().unwrap_or(0);
    let agent_lines = agents.iter().map(|(role, a)| {
        let kind = a.get("kind").filter(|k| k.is_set()).map_or(DEFAULT_AGENT.to_string(), Value::text);
        let pane = a.get("pane").map_or(String::new(), Value::text);
        format!("{role:<width$}  {kind:<8}  {pane}")
    });
    print_section(world, "agents", agent_lines.collect());
    let turns: Vec<Object> = truthy("turns")
        .and_then(Value::as_array)
        .map(|turns| turns.iter().filter_map(|t| t.as_object().cloned()).collect())
        .unwrap_or_default();
    print_section(world, "turns", turn_lines(&turns));
    print_section(world, "verdicts", verdict_lines(truthy("verdicts")));
    print_section(world, "files", host.run_files(&run_dir)?.into_iter().map(|f| f.path).collect());
    let round = saved.get("round").and_then(Value::as_i64).unwrap_or(0);
    let review = match &pipeline {
        Some(p) => last_review(world, &run_dir, p, round)?,
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

/// show's line per verdict a verdict step gave, of a saved state's verdicts (crate::state::StepVerdict), such as
/// "round 1  review  CHANGES_REQUESTED"; none for a run saved before them. A record that is not an object of those
/// fields shows "None" for each it lacks.
fn verdict_lines(verdicts: Option<&Value>) -> Vec<String> {
    let records = verdicts.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let field = |v: &Value, key: &str| v.get(key).map_or("None".to_string(), Value::text);
    let width = records.iter().map(|v| field(v, "step").chars().count()).max().unwrap_or(0);
    records
        .iter()
        .map(|v| format!("round {}  {:<width$}  {}", field(v, "round"), field(v, "step"), field(v, "verdict")))
        .collect()
}

/// The path of the last verdict file written in run_dir on world's host, up to round: of the latest round that has
/// one, the file of its last verdict step that has one; and its text without the VERDICT line. None without one, or
/// for a workflow without a verdict step. Errors: a file cannot be read.
fn last_review(
    world: &Connected,
    run_dir: &str,
    pipeline: &Pipeline,
    round: i64,
) -> Result<Option<(String, String)>, OrchestratorError> {
    let round = u32::try_from(round).unwrap_or(0);
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
