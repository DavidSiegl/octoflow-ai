//! Deleting the branches of runs whose pull request is done, as orchestrator.py's prune_branches: the `prune`
//! command, and the sweep a run that opens a pull request makes first (sweep_branches). A branch is deleted only when
//! its pull request's head commit holds all of it, so every deleted branch can be restored from its pull request
//! (GitHub keeps refs/pull/<n>/head; docs/design.md). The same holds for the --worktree a run left behind, which
//! would keep its branch checked out: it is removed only when clean and held by that head.

use super::{record_in_state, Connected};
use crate::constants::{BRANCH_PREFIX, CLOSED_BRANCH_GRACE, RUNS_DIR, WORKTREES_DIR};
use crate::error::OrchestratorError;
use crate::host::{git_status, GhError, Host, PullRequest};
use crate::runs::RUNNING;
use crate::state::{RunState, SavedRun};
use crate::workflow::{origin_unreachable, reaching_origin, Collaborators};

/// A run's branch_cleanup once both sides of its branch are gone.
pub const BRANCH_DELETED: &str = "deleted";
/// How a run's branch_cleanup, and a side's outcome, starts when a side was kept; the reason follows.
pub const BRANCH_KEPT: &str = "kept: ";

/// What prune_branches did with one run's branch, locally and on origin, and what it recorded for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrunedRun {
    /// The run's id, as its state.json saves it.
    pub run_id: String,
    /// The run's branch, as its state.json saves it: one the orchestrator made (BRANCH_PREFIX), since prune takes
    /// no other.
    pub branch: String,
    /// What became of the local branch: BRANCH_DELETED, "would be deleted", "gone", or BRANCH_KEPT and why; empty
    /// when it was not decided.
    pub local: String,
    /// What became of origin's branch, as local words it.
    pub remote: String,
    /// What became of the run's worktree: BRANCH_DELETED, "would be deleted", or BRANCH_KEPT and why; empty when the
    /// run has none to decide (see prune_worktree).
    pub worktree: String,
    /// Why nothing was decided: the run's state cannot be read (SavedRun::problem), the pull request is not done
    /// yet, or pruning the run failed. Nothing is recorded then.
    pub skipped: Option<String>,
    /// The run's branch_cleanup: written, or what would be in a dry run; None when skipped.
    pub cleanup: Option<String>,
}

impl PrunedRun {
    /// The run as prune prints it.
    pub fn line(&self) -> String {
        let what = match &self.skipped {
            Some(why) => format!("skipped: {why}"),
            None if self.worktree.is_empty() => format!("local {}, remote {}", self.local, self.remote),
            None => format!("worktree {}, local {}, remote {}", self.worktree, self.local, self.remote),
        };
        format!("{}  {}  {what}", self.run_id, self.branch)
    }
}

/// Delete the branches of the runs of the project at cwd, on world's host, whose pull request is merged, or closed
/// for CLOSED_BRANCH_GRACE by world's wall clock, and record what became of each in its state.json
/// (branch_cleanup); with dry_run, decide each and change nothing.
///
/// A local branch goes when it is at or behind its pull request's head commit, origin's when it is at it, deleted
/// under a lease; a branch checked out in a worktree stays, unless that is the run's own --worktree under
/// WORKTREES_DIR, which is removed first (git worktree remove, without force, so with its ignored files) when it
/// has no uncommitted changes and is at or behind the head too. Worktrees whose directory is gone are forgotten
/// first (Host::prune_worktrees), unless dry_run. Only runs with no branch_cleanup yet are candidates, and with
/// revisit the runs whose branch was kept too; a run without a pull request, one whose branch the orchestrator did
/// not make, and one an orchestrator drives now (RunState::health, a done run still ending included), are not. A run
/// whose state cannot be read (SavedRun::problem) is skipped with that reason. Returns each candidate and each such
/// run, in run_states' order, but those after gh was found unusable or origin unreachable, which ends the sweep. A
/// failure on one run is logged and skips that run.
///
/// Errors: the runs or the checked-out branches cannot be read, or the gone worktrees not be forgotten, or a run's
/// failure while world's interrupt is raised.
pub fn prune_branches(
    world: &Connected,
    cwd: &str,
    dry_run: bool,
    revisit: bool,
) -> Result<Vec<PrunedRun>, OrchestratorError> {
    let host = world.with.host;
    let runs = host.run_states(cwd)?;
    let candidates: Vec<&SavedRun> = runs.iter().filter(|run| is_prune_candidate(run, world, revisit)).collect();
    if candidates.is_empty() {
        return Ok(vec![]);
    }
    if !dry_run {
        host.prune_worktrees(cwd)?;
    }
    let mut checked_out = host.checked_out_branches(cwd)?;
    let mut pruned = vec![];
    for candidate in candidates {
        let shown = candidate.shown();
        let branch = shown.branch.clone().unwrap_or_default();
        let mut run = PrunedRun { run_id: shown.run_id.clone(), branch, ..PrunedRun::default() };
        let state = match candidate.state() {
            Ok(state) => state,
            Err(e) => {
                run.skipped = Some(e.0);
                pruned.push(run);
                continue;
            }
        };
        match prune_run(world, cwd, state, &mut run, &mut checked_out, dry_run) {
            Ok(()) => {}
            Err(e) if world.with.interrupt.is_raised() => return Err(e.into()),
            Err(GhError::Unusable(e)) => {
                world.with.human.log(&format!("stopped pruning run branches: {e}"));
                break;
            }
            Err(GhError::Failed(e)) => {
                world.with.human.log(&format!("could not prune the branch of run {}: {e}", run.run_id));
                run.skipped = Some(format!("failed: {e}"));
            }
        }
        pruned.push(run);
    }
    Ok(pruned)
}

/// Whether prune may touch the branch of run, as far as its state reads: one the orchestrator made and opened a pull
/// request for, and that no orchestrator drives now, as world tells (RunState::health). A run without a pull request
/// may hold the Builder's uncommitted work. One with a branch_cleanup is done with, or with revisit, only one whose
/// branch was kept.
fn is_prune_candidate(run: &SavedRun, world: &Connected, revisit: bool) -> bool {
    let s = run.shown();
    let branch = s.branch.as_deref().is_some_and(|b| b.starts_with(BRANCH_PREFIX));
    if !(branch && s.pr_url.as_deref().is_some_and(|url| !url.is_empty())) {
        return false;
    }
    if let Some(cleanup) = s.branch_cleanup.as_deref().filter(|c| !c.is_empty()) {
        if !(revisit && cleanup.starts_with(BRANCH_KEPT)) {
            return false;
        }
    }
    !run.health(world.local_host, world.with.pid_alive).is_some_and(|(h, _)| h == RUNNING)
}

/// Decide the run's worktree and each side of its branch, as state says, delete what may go, unless dry_run, and
/// record the result in its state.json, on world's host, filling run in; checked_out maps each branch checked out to
/// its worktree, and loses the branch of a worktree removed. Errors: GhError::Unusable when gh is, or when prune_remote fails as origin_unreachable says (its read of
/// origin's branch after every try, or a deletion there, which is tried once), either of which no other run can get
/// past; else GhError::Failed with what failed.
fn prune_run(
    world: &Connected,
    cwd: &str,
    state: &RunState,
    run: &mut PrunedRun,
    checked_out: &mut crate::map::Map<String>,
    dry_run: bool,
) -> Result<(), GhError> {
    let failed = GhError::Failed;
    let host = world.with.host;
    // Before gh pr view, as Python reads wall() for prune_run's call.
    let now = world.with.clocks.wall();
    // is_prune_candidate takes only runs with both.
    let (Some(url), Some(branch)) = (state.pr_url.as_deref(), state.branch.as_deref()) else {
        return Err(failed(OrchestratorError::new("the run has no branch or no pull request")));
    };
    let number = url.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let pr = host.pull_request(cwd, url)?;
    if let Some(why) = pull_request_pending(&pr, number, now).map_err(failed)? {
        run.skipped = Some(why);
        return Ok(());
    }
    let head = &pr.head_ref_oid;
    // Each side is decided on its own, so keeping one does not keep the other.
    // An origin unreachable for this run's branch is so for the runs after it too.
    let unusable = |e| if origin_unreachable(&e) { GhError::Unusable(e) } else { GhError::Failed(e) };
    let (remote, remote_kept) = prune_remote(&world.with, cwd, branch, head, number, dry_run).map_err(unusable)?;
    // Before the local branch, which a worktree that goes no longer pins.
    let path = state.worktree.as_deref().filter(|w| is_run_worktree(cwd, w));
    let (worktree, worktree_kept) = match path {
        Some(path) => prune_worktree(host, cwd, path, branch, head, number, dry_run).map_err(failed)?,
        None => (String::new(), None),
    };
    // A worktree that went frees the branch it held, not one checked out elsewhere (a detached worktree).
    let went = !worktree.is_empty() && worktree_kept.is_none();
    if went && path.is_some_and(|path| checked_out.get(branch).is_some_and(|at| at == path)) {
        checked_out.remove(branch);
    }
    let (local, local_kept) = prune_local(host, cwd, branch, head, number, checked_out, dry_run).map_err(failed)?;
    (run.remote, run.local, run.worktree) = (remote, local, worktree);
    let kept: Vec<String> = [worktree_kept, local_kept, remote_kept].into_iter().flatten().collect();
    let cleanup = if kept.is_empty() { BRANCH_DELETED.to_string() } else { format!("{BRANCH_KEPT}{}", kept.join("; ")) };
    run.cleanup = Some(cleanup.clone());
    if !dry_run {
        let path = format!("{cwd}/{RUNS_DIR}/{}/state.json", state.run_id);
        record_in_state(host, &path, state, |s| s.branch_cleanup = Some(cleanup), "its branch was pruned").map_err(failed)?;
    }
    Ok(())
}

/// Why the branch of pull request pr, number number, stays for now; None once it is merged, or closed for
/// CLOSED_BRANCH_GRACE before now, in seconds since the epoch. Its closing time is an RFC 3339 time, as gh gives it.
/// Errors: it is closed at what is not such a time.
pub fn pull_request_pending(pr: &PullRequest, number: &str, now: f64) -> Result<Option<String>, OrchestratorError> {
    if pr.state == "MERGED" {
        return Ok(None);
    }
    if pr.state != "CLOSED" {
        return Ok(Some(format!("PR #{number} is {}", pr.state.to_lowercase())));
    }
    let closed = pr.closed_at.as_deref().map(crate::summary::epoch_time);
    let Some(Ok(closed)) = closed else {
        let at = pr.closed_at.as_deref().map_or("None".to_string(), |at| format!("{at:?}"));
        return Err(OrchestratorError(format!("PR #{number} is closed at {at}, which is not a time")));
    };
    if now - closed > CLOSED_BRANCH_GRACE as f64 {
        return Ok(None);
    }
    Ok(Some(format!("PR #{number} was closed less than {} days ago", CLOSED_BRANCH_GRACE / 86400)))
}

/// What became of origin's branch, and why it was kept, if it was: gone already (its tracking ref then deleted),
/// kept with commits beyond the pull request's head, or deleted at head; on with's host, origin's branch read through
/// a short outage of origin (reaching_origin).
fn prune_remote(
    with: &Collaborators,
    cwd: &str,
    branch: &str,
    head: &str,
    number: &str,
    dry_run: bool,
) -> Result<(String, Option<String>), OrchestratorError> {
    let host = with.host;
    let tip = reaching_origin(with.clocks, with.human, with.interrupt, || host.remote_branch_commit(cwd, branch))?;
    let Some(tip) = tip else {
        if !dry_run {
            host.delete_tracking_ref(cwd, branch)?;
        }
        return Ok(("gone".to_string(), None));
    };
    if tip != head {
        let why = format!("remote has commits beyond PR #{number}");
        return Ok((format!("{BRANCH_KEPT}{why}"), Some(why)));
    }
    if !dry_run {
        host.delete_remote_branch_at(cwd, branch, head)?;
    }
    Ok((deletion(dry_run), None))
}

/// Whether path is directly under cwd's WORKTREES_DIR: where a --worktree run puts its worktree, and the only place
/// prune removes one from.
fn is_run_worktree(cwd: &str, path: &str) -> bool {
    let name = path.strip_prefix(&format!("{cwd}/{WORKTREES_DIR}/"));
    name.is_some_and(|n| !n.is_empty() && n != ".." && !n.contains('/'))
}

/// What became of the run's worktree at path, and why it was kept, if it was: not there any more (empty), kept with
/// uncommitted changes, with another branch than the run's branch checked out, with commits the pull request's head
/// lacks or that cannot be told without that head, or that git refuses to remove (locked, say), or removed, without
/// force. Errors: git cannot be run, or fails in any other step.
fn prune_worktree(
    host: &dyn Host,
    cwd: &str,
    path: &str,
    branch: &str,
    head: &str,
    number: &str,
    dry_run: bool,
) -> Result<(String, Option<String>), OrchestratorError> {
    if !host.is_worktree(cwd, path)? {
        return Ok((String::new(), None));
    }
    let kept = |why: String| Ok((format!("{BRANCH_KEPT}{why}"), Some(why)));
    if !git_status(host, path)?.is_empty() {
        return kept(format!("worktree {path} has uncommitted changes"));
    }
    // Someone reusing the leftover worktree for other work is not done with it.
    let held = host.checked_out_branch(path)?;
    if let Some(other) = held.as_deref().filter(|held| *held != branch) {
        return kept(format!("worktree {path} now holds branch {other}"));
    }
    if let Some(why) = head_missing(host, cwd, head, number)? {
        return kept(format!("worktree {path}: {why}"));
    }
    if !host.is_ancestor(path, "HEAD", head)? {
        return kept(format!("worktree {path} has commits PR #{number}'s head lacks"));
    }
    if !dry_run {
        // Git's refusal (a locked worktree, a submodule) is an answer, not a failure of the run.
        if let Some(why) = host.try_remove_worktree(cwd, path)? {
            return kept(format!("worktree {path} could not be removed: {why}"));
        }
    }
    Ok((deletion(dry_run), None))
}

/// Why the commits of cwd's repository cannot be told from pull request number's head commit: this clone lacks it.
fn head_missing(host: &dyn Host, cwd: &str, head: &str, number: &str) -> Result<Option<String>, OrchestratorError> {
    if host.has_commit(cwd, head)? {
        return Ok(None);
    }
    let short: String = head.chars().take(12).collect();
    Ok(Some(format!("PR #{number}'s head {short} is not in this clone; fetch it to decide")))
}

/// What became of the local branch, and why it was kept, if it was: kept while checked out (checked_out maps each
/// branch checked out to its worktree), gone already, kept with commits the pull request's head lacks or that cannot
/// be told without that head, or deleted.
fn prune_local(
    host: &dyn Host,
    cwd: &str,
    branch: &str,
    head: &str,
    number: &str,
    checked_out: &crate::map::Map<String>,
    dry_run: bool,
) -> Result<(String, Option<String>), OrchestratorError> {
    let kept = |why: String| Ok((format!("{BRANCH_KEPT}{why}"), Some(why)));
    if let Some(worktree) = checked_out.get(branch) {
        return kept(format!("checked out in {worktree}"));
    }
    let Some(tip) = host.branch_commit(cwd, branch)? else { return Ok(("gone".to_string(), None)) };
    if tip != head {
        if let Some(why) = head_missing(host, cwd, head, number)? {
            return kept(why);
        }
        if !host.is_ancestor(cwd, &tip, head)? {
            let n = host.commits_beyond(cwd, head, branch)?;
            return kept(format!("local has {n} commit{} beyond PR #{number}", if n == 1 { "" } else { "s" }));
        }
    }
    if !dry_run {
        host.delete_branch(cwd, branch)?;
    }
    Ok((deletion(dry_run), None))
}

/// What a side that may go became: deleted, or in a dry run, would be.
fn deletion(dry_run: bool) -> String {
    if dry_run { "would be deleted" } else { BRANCH_DELETED }.to_string()
}

/// Print each of pruned's lines on world's standard output, or that there was none.
pub fn print_pruned(world: &Connected, pruned: &[PrunedRun]) {
    if pruned.is_empty() {
        world.say("No run branches to prune.");
    }
    for run in pruned {
        world.say(&run.line());
    }
}

/// The one line a run logs about the branches and worktrees of earlier runs it pruned; None when it did nothing.
pub fn sweep_summary(pruned: &[PrunedRun]) -> Option<String> {
    let deleted = pruned
        .iter()
        .filter(|r| r.cleanup.as_deref() == Some(BRANCH_DELETED) && (r.local == BRANCH_DELETED || r.remote == BRANCH_DELETED))
        .count();
    let removed = pruned.iter().filter(|r| r.worktree == BRANCH_DELETED).count();
    let kept = pruned.iter().filter(|r| r.cleanup.as_deref().is_some_and(|c| c.starts_with(BRANCH_KEPT))).count();
    let mut parts = vec![];
    let mut gone = vec![];
    if deleted > 0 {
        gone.push(branches(deleted));
    }
    if removed > 0 {
        gone.push(if removed == 1 { "1 worktree".to_string() } else { format!("{removed} worktrees") });
    }
    if !gone.is_empty() {
        parts.push(format!("deleted {} of earlier runs", gone.join(" and ")));
    }
    if kept > 0 {
        // Names "branches" even after "deleted ... worktree", where a bare count would read as a kept worktree.
        parts.push(if gone.is_empty() {
            format!("kept {} of earlier runs, see prune", branches(kept))
        } else {
            format!("kept {}, see prune", branches(kept))
        });
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// n branches, in words.
fn branches(n: usize) -> String {
    if n == 1 { "1 branch".to_string() } else { format!("{n} branches") }
}

/// Prune the branches of the project's earlier runs, as a new run in cwd starts, logging what was done to world's
/// human; nothing that goes wrong stops the run, but a Ctrl-C. Errors: only one that failed while world's interrupt
/// is raised.
pub fn sweep_branches(world: &Connected, cwd: &str) -> Result<(), OrchestratorError> {
    match prune_branches(world, cwd, false, false) {
        Err(e) if world.with.interrupt.is_raised() => Err(e),
        Err(e) => {
            world.with.human.log(&format!("could not prune the branches of earlier runs: {e}"));
            Ok(())
        }
        Ok(pruned) => {
            if let Some(line) = sweep_summary(&pruned) {
                world.with.human.log(&line);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
