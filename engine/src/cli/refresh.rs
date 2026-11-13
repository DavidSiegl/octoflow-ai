//! `refresh`: what became of the pull request a finished run opened over conflicts with its base, and the clean-up
//! once it merges cleanly, is merged or is closed, as orchestrator.py's refresh_command, ConflictCheck and
//! refresh_conflict do it. drain refreshes the runs of its specs so too (crate::cli::drain).

use super::{record_in_state, Connected};
use crate::constants::{EXIT_WAITING, RUNS_DIR, SUCCEEDED};
use crate::error::OrchestratorError;
use crate::pull_request::without_conflict_warning;
use crate::state::{find_run, RunState, SavedRun};

/// What gh says now of the pull request a run opened over conflicts with its base (has_conflict), and so what
/// refresh_conflict does about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictCheck {
    /// The run's id, as its state saves it.
    pub run_id: String,
    /// The pull request's URL.
    pub url: String,
    /// OPEN, CLOSED or MERGED.
    pub pr_state: String,
    /// MERGEABLE, CONFLICTING, or UNKNOWN while GitHub has not computed it; it decides nothing for a pull request
    /// that is not OPEN.
    pub mergeable: String,
    /// The pull request's body with the run's conflict warning removed; None when it is not OPEN and MERGEABLE, or
    /// has no such warning left to remove.
    pub body: Option<String>,
    /// Whether clearing the conflict takes the pull request out of draft: it is OPEN, MERGEABLE and a draft, and the
    /// run's verdict is one of SUCCEEDED.
    pub ready: bool,
}

impl ConflictCheck {
    /// Whether the run's conflicts are forgotten: its pull request merges cleanly, or is merged or closed.
    pub fn clears(&self) -> bool {
        self.pr_state != "OPEN" || self.mergeable == "MERGEABLE"
    }

    /// What refresh prints for the run.
    pub fn line(&self) -> String {
        let (run_id, url) = (&self.run_id, &self.url);
        if self.pr_state != "OPEN" {
            return format!("{run_id}: {}, conflict forgotten", self.pr_state.to_lowercase());
        }
        match self.mergeable.as_str() {
            "MERGEABLE" => {
                let ready = if self.ready { ", ready for review" } else { "" };
                format!("{run_id}: conflict cleared{ready}  {url}")
            }
            "CONFLICTING" => format!("{run_id}: still conflicts  {url}"),
            _ => format!("{run_id}: mergeability not known yet, try again  {url}"),
        }
    }
}

/// Whether the run of state s is done, with a pull request it opened over conflicts it still records.
pub fn has_conflict(s: &RunState) -> bool {
    s.done() && s.pr_url.as_deref().is_some_and(|url| !url.is_empty()) && !s.conflicts.is_empty()
}

/// Ask gh about the pull request of the run whose state is state, which has_conflict, and, unless dry_run, act on what
/// it says: for one that merges cleanly now, remove the conflict warning from its body, every other edit to the body
/// kept, take it out of draft if the run succeeded, and record no conflicts in the run's state; for one merged or
/// closed, only record no conflicts. In every case that clears (a clean open one, a merged one, a closed one),
/// for_later_drain also records the pull request as conflict_cleared (RunState::conflict_cleared). One that still
/// conflicts, or whose mergeability GitHub has not computed yet, is left as it is. Returns what gh said, and so what
/// was done.
///
/// for_later_drain is true for refresh run by hand, whose clearing a later drain is to take up, and false for drain's
/// own refresh, which takes it up in the same pass. cwd is the project, where gh runs and the run's state is, on
/// world's host. Errors, naming the run: gh fails, or its state.json changed since state was read (record_in_state).
pub fn refresh_conflict(
    world: &Connected,
    cwd: &str,
    state: &RunState,
    dry_run: bool,
    for_later_drain: bool,
) -> Result<ConflictCheck, OrchestratorError> {
    let run_id = &state.run_id;
    let refreshed = || -> Result<ConflictCheck, OrchestratorError> {
        let host = world.with.host;
        let url = state.pr_url.clone().unwrap_or_default();
        let pr = host.pull_request_merge(cwd, &url)?;
        let clean = pr.state == "OPEN" && pr.mergeable == "MERGEABLE";
        let body = clean.then(|| without_conflict_warning(&pr.body, state.base_branch.as_deref(), &state.conflicts));
        let succeeded = state.verdict.as_deref().is_some_and(|v| SUCCEEDED.contains(&v));
        let check = ConflictCheck {
            run_id: run_id.clone(),
            url: url.clone(),
            body: body.filter(|b| *b != pr.body),
            ready: clean && pr.is_draft && succeeded,
            pr_state: pr.state,
            mergeable: pr.mergeable,
        };
        if dry_run || !check.clears() {
            return Ok(check);
        }
        if let Some(body) = &check.body {
            host.edit_pr_body(cwd, &url, body)?;
        }
        if check.ready {
            host.mark_pr_ready(cwd, &url)?;
        }
        let path = format!("{cwd}/{RUNS_DIR}/{run_id}/state.json");
        let cleared = for_later_drain.then(|| url.clone());
        let record = |s: &mut RunState| {
            s.conflicts.clear();
            if let Some(url) = cleared {
                s.conflict_cleared = Some(url);
            }
        };
        record_in_state(host, &path, state, record, "its pull request was refreshed")?;
        Ok(check)
    };
    refreshed().map_err(|e| OrchestratorError(format!("run {run_id}: {e}")))
}

/// Refresh the run run_ref names (find_run), or without one every run in cwd that has_conflict, in run id order,
/// printing a line each (ConflictCheck::line); there, a run that has_conflict as far as its state reads, but whose
/// state cannot be read, gets a line saying why it is skipped (SavedRun::problem). Returns the exit status: 0 when each
/// conflict was forgotten, or there was none to refresh, and EXIT_WAITING when any pull request still conflicts or is
/// not known to merge yet, a skipped run's among them. Errors: run_ref names no run, one whose state cannot be read, or
/// one that is not done; and as refresh_conflict.
pub fn refresh_command(world: &Connected, cwd: &str, run_ref: Option<&str>) -> Result<i32, OrchestratorError> {
    let all = world.with.host.run_states(cwd)?;
    let runs: Vec<&SavedRun> = match run_ref {
        Some(run_ref) => {
            let run = find_run(&all, run_ref, cwd)?;
            let state = run.state()?;
            if !state.done() {
                return Err(OrchestratorError(format!(
                    "run {} is not done; refresh only looks at a finished run's pull request",
                    state.run_id
                )));
            }
            if !has_conflict(state) {
                world.say(&format!("{}: no conflict to clear", state.run_id));
                return Ok(0);
            }
            vec![run]
        }
        None => {
            let mut runs: Vec<&SavedRun> = all.iter().filter(|run| has_conflict(run.shown())).collect();
            runs.sort_by(|a, b| a.shown().run_id.cmp(&b.shown().run_id));
            runs
        }
    };
    let mut waiting = false;
    for run in runs {
        let state = match run.state() {
            Ok(state) => state,
            Err(e) => {
                world.say(&format!("{}: skipped: {e}", run.shown().run_id));
                // Its conflict is not cleared, and nothing tells when it will be.
                waiting = true;
                continue;
            }
        };
        let check = refresh_conflict(world, cwd, state, false, true)?;
        world.say(&check.line());
        waiting = waiting || !check.clears();
    }
    Ok(if waiting { EXIT_WAITING } else { 0 })
}

#[cfg(test)]
mod tests;
