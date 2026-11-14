//! Cleaning up after a run nothing drives any more, as orchestrator.py's close_run: the `close` command, for one run
//! or every run of a project.

use super::{record_in_state, Connected};
use crate::constants::{EXIT_ERROR, RUNS_DIR};
use crate::error::OrchestratorError;
use crate::host::git_status;
use crate::runs::RUNNING;
use crate::state::{find_run, refs_prefix, RunState, SavedRun};

/// What close does to one run, decided just before it is done (decide), so that a dry run says what closing would do.
enum Decision<'r> {
    /// The run is closed already; its state, whose closed says how.
    AlreadyClosed(&'r RunState),
    /// The run must not be closed now, and why, naming it: its state cannot be read (SavedRun::problem), or it is still
    /// running or done but still ending (RunState::health, as a done run still ending may be deleting its refs itself,
    /// Workflow::clean_up_locks).
    Refused(OrchestratorError),
    /// The run is to be closed as the plan says.
    Close(Plan<'r>),
}

/// What closing a run would do to it, each step's object as it stood when decided.
struct Plan<'r> {
    /// The run's state as read.
    state: &'r RunState,
    /// The herdr workspace to close; None when the run never had one or herdr no longer has it.
    workspace: Option<&'r str>,
    /// What becomes of the run's worktree.
    worktree: WorktreeFate<'r>,
    /// True when the run's local refs (crate::state::refs_prefix) are to be deleted: it locked files, so it has or had
    /// refs, which a close that failed part way may have deleted already, or it has refs nonetheless (its first lock
    /// failed after making one).
    refs: bool,
}

/// What close does to a run's worktree.
enum WorktreeFate<'r> {
    /// Nothing: the run has none (an in-place run, whose checkout is the human's), or a finished run's went with its
    /// pull request.
    None,
    /// A finished run's worktree, at this path, holds a --no-pr run's change, so it stays.
    Kept(&'r str),
    /// A failed or stale run's worktree, at path, goes, and so may its branch.
    Discarded {
        path: &'r str,
        /// What goes with the worktree, one line of git status each, none for a clean one; None when the worktree is
        /// already gone.
        changes: Option<Vec<String>>,
        /// The run's branch and what becomes of it; None when the run has none or it is already gone. Looked at even
        /// with the worktree gone, as a close that failed at the branch must find it again.
        branch: Option<(&'r str, BranchFate)>,
    },
}

/// What close does to a discarded worktree's branch.
enum BranchFate {
    /// It is deleted: it holds no commit beyond the run's base, and the Builder never commits, so it has nothing to lose.
    Deleted,
    /// It stays, and why: it has commits beyond the base, or the run has no base to tell.
    Kept(String),
}

/// Clean up after the run of the project at cwd that run_ref names, by its id or key, on world's herdr and host,
/// record what was done as the run's closed in its state.json, so that resume refuses it, and return that. Prints
/// each step on world's standard output. With dry_run, change nothing: print what would be discarded with the
/// worktree and the line closing would record, and return that line.
///
/// Every run loses its workspace and its local refs (crate::state::refs_prefix), which hold the files its steps locked
/// and which nothing else deletes once it is closed. A failed or stale --worktree run also loses its worktree, changes
/// and all (listed first), and its branch if that holds no commit beyond the run's base. A finished run's worktree is a
/// --no-pr run's change, and an in-place run's checkout and branch are the human's to sort out, so both stay. Each step
/// skips what is already gone, so a close that failed part way can be run again; closed is recorded only once every
/// step has succeeded. A run already closed is left as it is, and its closed returned.
///
/// Errors: no run or several match run_ref; the run is Decision::Refused; herdr or git cannot tell what is there; a
/// step fails; or the state.json changed since it was read.
pub fn close_run(world: &Connected, cwd: &str, run_ref: &str, dry_run: bool) -> Result<String, OrchestratorError> {
    let runs = world.with.host.run_states(cwd)?;
    match decide(world, cwd, find_run(&runs, run_ref, cwd)?)? {
        Decision::AlreadyClosed(state) => {
            let closed = state.closed.clone().unwrap_or_default();
            world.say(&format!("run {} is already closed: {closed}", state.run_id));
            Ok(closed)
        }
        Decision::Refused(e) => Err(e),
        Decision::Close(plan) => plan.close(world, cwd, dry_run),
    }
}

/// Close every run of the project at cwd as close_run does, in run id order, or with dry_run say what closing each
/// would do, then print a count of the runs closed (or to close), skipped and already closed, and failed if any. A
/// Decision::Refused run is skipped with a line saying why; one already closed is only counted. Each run is read again
/// just before it is closed, so one that started running since the sweep began is skipped. A run whose close fails
/// gets a line with the error, and the rest are still closed. A raised Interrupt starts no further run; the run being
/// closed then finishes or fails as close_run would, and a second sweep finishes it.
///
/// Returns the exit status: 0, or EXIT_ERROR when any run's close failed. Errors: the runs cannot be read at the start.
pub fn close_all(world: &Connected, cwd: &str, dry_run: bool) -> Result<i32, OrchestratorError> {
    let host = world.with.host;
    let closed_as_read = |run: &SavedRun| run.state().is_ok_and(is_closed);
    let mut runs: Vec<(String, bool)> =
        host.run_states(cwd)?.iter().map(|run| (run.shown().run_id.clone(), closed_as_read(run))).collect();
    runs.sort();
    let (mut closed, mut skipped, mut already, mut failed) = (0, 0, 0, 0);
    for (id, was_closed) in runs {
        if world.with.interrupt.is_raised() {
            break;
        }
        // A closed run stays closed: resume refuses it.
        if was_closed {
            already += 1;
            continue;
        }
        match sweep_run(world, cwd, &id, dry_run) {
            Ok(Swept::Closed) => closed += 1,
            Ok(Swept::AlreadyClosed) => already += 1,
            Ok(Swept::Skipped(why)) => {
                world.say(&format!("skipped: {why}"));
                skipped += 1;
            }
            Err(e) => {
                world.say(&format!("could not close run {id}: {e}"));
                failed += 1;
            }
        }
    }
    let verb = if dry_run { "to close" } else { "closed" };
    let failures = if failed > 0 { format!(", {failed} failed") } else { String::new() };
    world.say(&format!("{closed} {verb}, {skipped} skipped, {already} already closed{failures}"));
    Ok(if failed > 0 { EXIT_ERROR } else { 0 })
}

/// Read the run of the project at cwd whose id is run_id again, and close it as close_all says, or with dry_run say
/// what closing it would do. Errors: its state.json is gone, cannot be read, or its close fails.
fn sweep_run(world: &Connected, cwd: &str, run_id: &str, dry_run: bool) -> Result<Swept, OrchestratorError> {
    let gone = || OrchestratorError(format!("its state.json is gone from {cwd}/{RUNS_DIR}"));
    let run = world.with.host.run_state(cwd, run_id)?.ok_or_else(gone)?;
    Ok(match decide(world, cwd, &run)? {
        Decision::AlreadyClosed(_) => Swept::AlreadyClosed,
        Decision::Refused(why) => Swept::Skipped(why),
        Decision::Close(plan) => plan.close(world, cwd, dry_run).map(|_| Swept::Closed)?,
    })
}

/// What became of one run of close_all that did not fail.
enum Swept {
    /// Closed, or with a dry run said what closing would do.
    Closed,
    /// Closed since the sweep read the runs.
    AlreadyClosed,
    /// Not closed, and why (Decision::Refused).
    Skipped(OrchestratorError),
}

/// Whether state records that the run was closed.
fn is_closed(state: &RunState) -> bool {
    state.closed.as_deref().is_some_and(|c| !c.is_empty())
}

/// What closing run, of the project at cwd, would do now, as world's herdr and host find it; changes nothing.
/// Errors: herdr or git cannot tell what is there.
fn decide<'r>(world: &Connected, cwd: &str, run: &'r SavedRun) -> Result<Decision<'r>, OrchestratorError> {
    let host = world.with.host;
    let state = match run.state() {
        Ok(state) => state,
        Err(e) => return Ok(Decision::Refused(e)),
    };
    if is_closed(state) {
        return Ok(Decision::AlreadyClosed(state));
    }
    let health = run.health(world.local_host, world.with.pid_alive);
    if let Some((RUNNING, why)) = &health {
        return Ok(Decision::Refused(OrchestratorError(format!(
            "run {} is still running ({why}); stop it before closing it",
            state.run_id
        ))));
    }
    // Stale and not done: its orchestrator died mid-run. A done run stale while ending finished its change.
    let failed = state.error.as_deref().is_some_and(|e| !e.is_empty()) || (health.is_some() && !state.done());
    let workspace = match Some(state.workspace_id.as_str()).filter(|w| !w.is_empty()) {
        Some(w) if world.with.herdr.workspace_exists(w)? => Some(w),
        _ => None,
    };
    let worktree = match state.worktree.as_deref().filter(|w| !w.is_empty()) {
        Some(path) if failed => {
            let changes = match host.is_worktree(cwd, path)? {
                true => Some(git_status(host, path)?.lines().map(String::from).collect()),
                false => None,
            };
            WorktreeFate::Discarded { path, changes, branch: branch_fate(world, cwd, state)? }
        }
        Some(path) if host.is_worktree(cwd, path)? => WorktreeFate::Kept(path),
        _ => WorktreeFate::None,
    };
    // A run that locked nothing, in a cwd that is no git repository with a commit, such as a --no-pr run's, has no
    // refs, and no git command runs for them.
    let refs = !state.locked.is_empty() || {
        let prefix = refs_prefix(&state.run_id);
        let unreadable = |e: OrchestratorError| OrchestratorError(format!("could not read the refs under {prefix}: {e}"));
        host.git_head(cwd)?.is_some() && !host.refs(cwd, &prefix).map_err(unreadable)?.is_empty()
    };
    Ok(Decision::Close(Plan { state, workspace, worktree, refs }))
}

/// The branch in cwd of the failed run of state and what close does to it; None when it has none or it is gone.
fn branch_fate<'r>(
    world: &Connected,
    cwd: &str,
    state: &'r RunState,
) -> Result<Option<(&'r str, BranchFate)>, OrchestratorError> {
    let host = world.with.host;
    let Some(branch) = state.branch.as_deref().filter(|b| !b.is_empty()) else { return Ok(None) };
    if host.branch_commit(cwd, branch)?.is_none() {
        return Ok(None);
    }
    let fate = match state.base.as_deref() {
        None => BranchFate::Kept("the run has no base to compare it with".to_string()),
        Some(base) => match host.commits_beyond(cwd, base, branch)? {
            0 => BranchFate::Deleted,
            n => BranchFate::Kept(format!("{n} commit{} beyond base", if n == 1 { "" } else { "s" })),
        },
    };
    Ok(Some((branch, fate)))
}

impl Plan<'_> {
    /// What closing records as the run's closed, the steps in order, such as "workspace closed; worktree deleted;
    /// branch deleted; refs deleted".
    fn summary(&self) -> String {
        let workspace = if self.workspace.is_some() { "workspace closed" } else { "workspace already gone" };
        let mut done = vec![workspace.to_string()];
        match &self.worktree {
            WorktreeFate::None => {}
            WorktreeFate::Kept(path) => done.push(format!("worktree kept: {path}")),
            WorktreeFate::Discarded { changes, branch, .. } => {
                done.push(if changes.is_some() { "worktree deleted" } else { "worktree already gone" }.to_string());
                match branch {
                    Some((_, BranchFate::Deleted)) => done.push("branch deleted".to_string()),
                    Some((_, BranchFate::Kept(why))) => done.push(format!("branch kept: {why}")),
                    None => {}
                }
            }
        }
        if self.refs {
            done.push("refs deleted".to_string());
        }
        done.join("; ")
    }

    /// Carry out the plan on the run of the project at cwd, or with dry_run only say what it would do, as close_run
    /// says; return its summary. Errors: as close_run's for a step or the record.
    fn close(&self, world: &Connected, cwd: &str, dry_run: bool) -> Result<String, OrchestratorError> {
        let (host, run_id) = (world.with.host, &self.state.run_id);
        let closed = self.summary();
        if dry_run {
            self.say_changes(world, "would discard");
            world.say(&format!("would close run {run_id}: {closed}"));
            return Ok(closed);
        }
        if let Some(workspace) = self.workspace {
            world.with.herdr.close_workspace(workspace)?;
            world.say(&format!("closed workspace {workspace}"));
        }
        if let WorktreeFate::Discarded { path, changes, branch } = &self.worktree {
            if changes.is_some() {
                self.say_changes(world, "discarding");
                host.remove_worktree(cwd, path, true)?;
                world.say(&format!("deleted worktree {path}"));
            }
            match branch {
                Some((branch, BranchFate::Deleted)) => {
                    host.delete_branch(cwd, branch)?;
                    world.say(&format!("deleted branch {branch}"));
                }
                Some((branch, BranchFate::Kept(why))) => world.say(&format!("kept branch {branch}: {why}")),
                None => {}
            }
        }
        if self.refs {
            let prefix = refs_prefix(run_id);
            let deleted = host
                .delete_refs(cwd, &prefix)
                .map_err(|e| OrchestratorError(format!("could not delete the refs under {prefix}: {e}")))?;
            if deleted {
                world.say(&format!("deleted the refs under {prefix}"));
            }
        }
        let path = format!("{cwd}/{RUNS_DIR}/{run_id}/state.json");
        record_in_state(host, &path, self.state, |s| s.closed = Some(closed.clone()), "the run was closed")?;
        world.say(&format!("closed run {run_id}: {closed}"));
        Ok(closed)
    }

    /// Print what goes with a discarded worktree, if anything does, under a line "<doing> in <worktree>:".
    fn say_changes(&self, world: &Connected, doing: &str) {
        if let WorktreeFate::Discarded { path, changes: Some(changes), .. } = &self.worktree {
            if !changes.is_empty() {
                world.say(&format!("{doing} in {path}:"));
                for line in changes {
                    world.say(&format!("    {line}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
