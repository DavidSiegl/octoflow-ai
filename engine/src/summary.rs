//! What a run's records say once it ends: its turns as summary.md, the pull request and show print them, summary.md
//! itself, metrics.json for the metrics command, and the times and durations they are written in.

use crate::harness::{Tokens, Usage};
use crate::pipeline::Pipeline;
use serde_json::Value;
use crate::json::{Object, ValueExt};
use crate::state::{RunState, TurnRecord};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use serde_json::json;

/// The version of run_metrics' shape, raised when a key changes its meaning or goes.
pub const METRICS_VERSION: i64 = 1;

/// seconds since the epoch as an RFC 3339 time in UTC to the second, with its offset written out:
/// 2026-10-28T08:34:31+00:00. The engine writes every time it records so.
pub fn iso_utc(seconds: i64) -> String {
    // Seconds beyond jiff's years -9999 to 9999 are clamped to its bounds; no clock gives them.
    let seconds = seconds.clamp(Timestamp::MIN.as_second(), Timestamp::MAX.as_second());
    let at = Timestamp::from_second(seconds).unwrap_or(Timestamp::UNIX_EPOCH);
    at.strftime("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// The seconds since the epoch, with their fraction to the microsecond, of an ISO 8601 time with an offset (Z, +hh:mm
/// or +hh), as jiff's Timestamp reads it: RFC 3339 and ISO 8601's basic format among its forms. Errors: iso is not
/// such a time, a time without an offset among them, as jiff words it.
pub fn epoch_time(iso: &str) -> Result<f64, String> {
    let at: Timestamp = iso.parse().map_err(|e: jiff::Error| e.to_string())?;
    Ok(at.as_microsecond() as f64 / 1e6)
}

/// epoch_time's whole seconds, the fraction cut off; errors as its.
pub fn epoch_seconds(iso: &str) -> Result<i64, String> {
    epoch_time(iso).map(|seconds| seconds.floor() as i64)
}

/// A duration as summary.md and the tables write it: 42s, 3m 07s, 2h 05m.
pub fn format_duration(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    }
}

/// An age as list and show write it: 42s, 3m, 2h5m.
pub fn format_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h{}m", seconds / 3600, seconds % 3600 / 60)
    }
}

/// The cells of a turn record (RunState::turns) as the tables show them.
struct TurnCells {
    step: String,
    role: String,
    round: String,
    harness: String,
    model: String,
    start: String,
    duration: String,
    file: String,
}

impl TurnCells {
    /// A turn record's values as the tables show them, its start in zone. A start or duration that is not known
    /// shows as ?: a file written before turns were recorded, or a turn still open. A quality analysis runs in no
    /// harness.
    fn of(t: &TurnRecord, zone: &TimeZone) -> TurnCells {
        let set = |text: &Option<String>| text.clone().filter(|t| !t.is_empty());
        let harness = t.harness.clone();
        let round = match t.quality_round {
            0 => t.round.to_string(),
            q => format!("{} q{q}", t.round),
        };
        let model = match (set(&t.model), &harness) {
            (Some(model), _) => model,
            (None, Some(_)) => "default".into(),
            (None, None) => "-".into(),
        };
        let start = match set(&t.started_at) {
            Some(at) => match at.parse::<Timestamp>() {
                Ok(at) => at.to_zoned(zone.clone()).strftime("%Y-%m-%d %H:%M:%S").to_string(),
                // Not a time with an offset, which the engine never writes (iso_utc): shown as saved, as UTC was.
                Err(_) => {
                    let at = at.replace('T', " ");
                    at.strip_suffix("+00:00").unwrap_or(&at).to_string()
                }
            },
            None => "?".into(),
        };
        TurnCells {
            step: t.step.clone(),
            role: t.role.clone(),
            round,
            harness: harness.unwrap_or_else(|| "-".into()),
            model,
            start,
            duration: t.seconds.map_or("?".into(), format_duration),
            file: t.file.clone(),
        }
    }
}

/// The turns as a Markdown table, the human-paced ones marked: their time is the human's as much as the agent's.
pub fn turns_table(turns: &[TurnRecord]) -> String {
    let head = ["step", "role", "round", "harness", "model", "start (UTC)", "duration", "file"].map(String::from);
    let mut rows = vec![head.to_vec(), vec!["---".to_string(); head.len()]];
    for t in turns {
        let mut c = TurnCells::of(t, &TimeZone::UTC);
        if t.human_paced {
            c.step.push_str(" (human-paced)");
        }
        rows.push(vec![c.step, c.role, c.round, c.harness, c.model, c.start, c.duration, c.file]);
    }
    rows.iter()
        .map(|row| format!("| {} |", row.iter().map(|c| c.replace('|', "\\|")).collect::<Vec<_>>().join(" | ")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The turns as show prints them, a line each, in columns of step, round, harness, start in zone, duration and file,
/// each as wide as its widest cell, in characters.
pub fn turn_lines(turns: &[TurnRecord], zone: &TimeZone) -> Vec<String> {
    let rows: Vec<[String; 6]> = turns
        .iter()
        .map(|t| {
            let c = TurnCells::of(t, zone);
            [c.step, c.round, c.harness, c.start, c.duration, c.file]
        })
        .collect();
    let widths: Vec<usize> = (0..6).map(|k| rows.iter().map(|r| r[k].chars().count()).max().unwrap_or(0)).collect();
    rows.iter()
        .map(|r| {
            let cells: Vec<String> = r.iter().zip(&widths).map(|(cell, w)| format!("{cell:<w$}")).collect();
            cells.join("  ").trim_end().to_string()
        })
        .collect()
}

/// The start of each turn that has one, in seconds since the epoch; a start that is not a time is left out, where
/// the Python engine would fail.
fn turn_starts(state: &RunState) -> Vec<i64> {
    state
        .turns
        .iter()
        .filter_map(|t| t.started_at.as_deref())
        .filter_map(|at| epoch_seconds(at).ok())
        .collect()
}

/// summary.md of a run that ended at now, in seconds since the epoch: how it turned out, who played each role, and
/// where its time went. rounds holds a line per round, with what its handoff files say (Workflow::round_results).
pub fn run_summary(state: &RunState, pipeline: &Pipeline, rounds: &[String], now: i64) -> String {
    let s = state;
    let outcome = match (&s.error, &s.verdict) {
        (Some(e), _) if !e.is_empty() => format!("error: {e}"),
        (_, Some(v)) if !v.is_empty() => v.clone(),
        _ => "unfinished".into(),
    };
    let wall = turn_starts(s).into_iter().min().map_or("?".to_string(), |start| format_duration(now - start));
    let roles = pipeline.roles().iter().map(|(role, label)| {
        let harness = s.harness(role);
        let model = s.models.get(role).filter(|m| !m.is_empty()).map_or("default", String::as_str);
        format!("| {label} ({role}) | {harness} | {model} |")
    });
    let pr = s.pr_url.as_deref().filter(|u| !u.is_empty()).unwrap_or("none");
    let branch = s.branch.as_deref().filter(|b| !b.is_empty()).map_or("none".to_string(), |b| format!("`{b}`"));
    let mut lines = vec![
        format!("# Run {}", s.run_id),
        String::new(),
        format!("- **Workflow:** {}", s.workflow),
        format!("- **Outcome:** {outcome}"),
        format!("- **Pull request:** {pr}"),
        format!("- **Branch:** {branch}"),
        format!("- **Wall time:** {wall}, from the first turn's start to {}", iso_utc(now)),
        String::new(),
        "## Task".into(),
        String::new(),
        s.task.trim().to_string(),
        String::new(),
        "## Roles".into(),
        String::new(),
        "| role | harness | model |".into(),
        "| --- | --- | --- |".into(),
    ];
    lines.extend(roles);
    lines.extend([String::new(), "## Turns".into(), String::new()]);
    lines.push(if s.turns.is_empty() { "No turns were recorded.".into() } else { turns_table(&s.turns) });
    if !rounds.is_empty() {
        lines.extend([String::new(), "## Rounds".into(), String::new()]);
        lines.extend(rounds.iter().map(|line| format!("- {line}")));
    }
    lines.join("\n") + "\n"
}

/// The turn's "tokens", as state.json and metrics.json hold them: the tokens of the answers its agent gave from the
/// turn's start to the second its handoff file was written, summed, from usage, Harness::session_usage of the session
/// it ran in throughout; "unknown" when there is no such usage, or the turn's session or either end is not known, or
/// not a time.
pub fn turn_tokens(turn: &TurnRecord, usage: Option<&[Usage]>) -> Value {
    let unknown = || Value::from("unknown");
    let Some(usage) = usage.filter(|_| turn.session.as_deref().is_some_and(|s| !s.is_empty())) else {
        return unknown();
    };
    let time = |at: &Option<String>| at.as_deref().and_then(|at| epoch_seconds(at).ok());
    let (Some(start), Some(end)) = (time(&turn.started_at), time(&turn.ended_at)) else { return unknown() };
    // The answers within [start, end + 1), compared as Python compares an int with a float.
    let (start, end) = (start as f64, (end + 1) as f64);
    let total =
        usage.iter().filter(|u| start <= u.when && u.when < end).fold(Tokens::default(), |sum, u| sum.plus(u.tokens));
    total.to_json()
}

/// metrics.json of a run that is done, at now, in seconds since the epoch: one object of the run's outcome, its
/// roles, its turns with their times and tokens, and its quality rounds with their fingerprints, never their
/// findings' text. docs/design.md gives every key.
pub fn run_metrics(state: &RunState, now: i64) -> Value {
    let s = state;
    let start = turn_starts(s).into_iter().min();
    let turns: Vec<Value> = s
        .turns
        .iter()
        .map(|t| {
            let harness = t.harness.as_deref();
            let tokens = match harness {
                None => Value::Null,
                Some(_) if t.tokens.is_set() => t.tokens.clone(),
                Some(_) => Value::from("unknown"),
            };
            json!({
                "step": t.step,
                "role": t.role,
                "harness": t.harness,
                "model": t.model,
                "round": t.round,
                "quality_round": t.quality_round,
                "human_paced": t.human_paced,
                "started_at": t.started_at,
                "ended_at": t.ended_at,
                "seconds": t.seconds,
                "tokens": tokens,
            })
        })
        .collect();
    let roles: Object = s
        .agents
        .iter()
        .map(|(role, agent)| {
            (role.to_string(), json!({"harness": agent.harness, "model": s.models.get(role)}))
        })
        .collect();
    let restored: usize = s
        .restored
        .iter()
        .filter_map(|r| r.get("paths").and_then(Value::as_object))
        .flat_map(|paths| paths.values())
        .map(|ps| ps.as_array().map_or(0, Vec::len))
        .sum();
    let outcome = match (&s.verdict, &s.error) {
        (Some(v), _) if !v.is_empty() => v.clone(),
        (_, Some(e)) if !e.is_empty() => "error".into(),
        _ => "unfinished".into(),
    };
    json!({
        "version": METRICS_VERSION,
        "run_id": s.run_id,
        "workflow": s.workflow,
        "outcome": outcome,
        "rounds": s.round,
        "max_rounds": s.max_rounds,
        "quality_gate": s.had_quality_gate(),
        "max_quality_rounds": if s.had_quality_gate() { Some(s.max_quality_rounds) } else { None },
        "pull_request": s.pr_url,
        "started_at": start.map(iso_utc),
        "ended_at": iso_utc(now),
        "seconds": start.map(|start| now - start),
        "roles": roles,
        "turns": turns,
        "quality_rounds": s.quality_rounds,
        "restored_locked_files": restored,
    })
}

#[cfg(test)]
mod tests;
