//! Cleaning up after a run nothing drives any more: the `close` command, for one run or every run of a project.

use super::{record_in_state, Connected};
use crate::constants::RUNS_DIR;
use crate::error::OrchestratorError;
use crate::herdr::Workspaces;
use crate::host::{git_status, Host};
use crate::interrupt::Interrupt;
use crate::runs::RUNNING;
use crate::state::{find_run, refs_prefix, RunState, SavedRun};

/// What close talks to: the project's host, its runs' herdr workspaces, this machine's processes, and where it says
/// what it does: the CLI's from its Connected (Closer::of), or any other caller's from what it reaches a project
/// with.
#[derive(Clone, Copy)]
pub struct Closer<'a> {
    /// The machine the project is on: its runs' state, worktrees, branches and refs.
    pub host: &'a dyn Host,
    /// Where each run's herdr workspace is told and closed, in the server the run saved.
    pub workspaces: &'a dyn Workspaces,
    /// Once raised, close_all starts no further run.
    pub interrupt: &'a Interrupt,
    /// Whether a process of a pid exists on this machine, for telling a run still running (RunState::health).
    pub pid_alive: &'a dyn Fn(i64) -> bool,
    /// This machine's name, which an owner record of a run here names (crate::runs::local_host).
    pub local_host: &'a str,
    /// Takes each line close prints, without its line end.
    pub say: &'a dyn Fn(&str),
}

/// One run close_all looked at, and what became of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunClosed {
    /// The run's id.
    pub run: String,
    pub outcome: Outcome,
}

/// What became of one run of close_all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Closed, or with a dry run to be closed.
    Closed {
        /// What closing recorded as the run's closed (Plan::summary), or with a dry run what it would do
        /// (Plan::preview).
        done: String,
        /// What went, or would go, with the run's discarded worktree: its uncommitted changes, one line of git status
        /// each; empty when the worktree is kept, clean or gone, or the run has none.
        discarded: Vec<String>,
    },
    /// Closed before, by close or since the sweep began: what close recorded then.
    AlreadyClosed(String),
    /// Not closed, and why: Decision::Refused's reason, or the run is gone.
    Skipped(String),
    /// Its close failed, with this error; what it did before the failure stays done.
    Failed(String),
}

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

/// Clean up after the run of the project at cwd that run_ref names, by its id or key, on world's host and in its
/// workspace's herdr server, record what was done as the run's closed in its state.json, so that resume refuses it,
/// and return that. Says each step through world. With dry_run, change nothing: say what would be discarded with the
/// worktree and a line naming each thing closing would close, delete or keep (Plan::preview), and return that line.
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
pub fn close_run(world: &Closer, cwd: &str, run_ref: &str, dry_run: bool) -> Result<String, OrchestratorError> {
    let runs = world.host.run_states(cwd)?;
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
/// would do, then say a count of the runs closed (or to close), skipped and already closed, and failed if any. A
/// Decision::Refused run is skipped with a line saying why; one already closed is only counted. Each run is read again
/// just before it is closed, so one that started running, or was removed, since the sweep began is skipped. A run
/// whose close fails gets a line with the error, and the rest are still closed. A raised Interrupt starts no further
/// run; the run being closed then finishes or fails as close_run would, and a second sweep finishes it.
///
/// Returns each run looked at, in that order; none after an Interrupt. Errors: the runs cannot be read at the start.
pub fn close_all(world: &Closer, cwd: &str, dry_run: bool) -> Result<Vec<RunClosed>, OrchestratorError> {
    let mut runs: Vec<(String, Option<String>)> =
        world.host.run_states(cwd)?.iter().map(|run| (run.shown().run_id.clone(), closed_as_read(run))).collect();
    runs.sort();
    let mut looked_at = vec![];
    for (run, closed) in runs {
        if world.interrupt.is_raised() {
            break;
        }
        // A closed run stays closed: resume refuses it.
        let outcome = match closed {
            Some(closed) => Outcome::AlreadyClosed(closed),
            None => sweep_run(world, cwd, &run, dry_run).unwrap_or_else(|e| Outcome::Failed(e.to_string())),
        };
        match &outcome {
            Outcome::Skipped(why) => world.say(&format!("skipped: {why}")),
            Outcome::Failed(e) => world.say(&format!("could not close run {run}: {e}")),
            Outcome::Closed { .. } | Outcome::AlreadyClosed(_) => {}
        }
        looked_at.push(RunClosed { run, outcome });
    }
    let count = |of: fn(&Outcome) -> bool| looked_at.iter().filter(|r| of(&r.outcome)).count();
    let closed = count(|o| matches!(o, Outcome::Closed { .. }));
    let skipped = count(|o| matches!(o, Outcome::Skipped(_)));
    let already = count(|o| matches!(o, Outcome::AlreadyClosed(_)));
    let failed = count(|o| matches!(o, Outcome::Failed(_)));
    let verb = if dry_run { "to close" } else { "closed" };
    let failures = if failed > 0 { format!(", {failed} failed") } else { String::new() };
    world.say(&format!("{closed} {verb}, {skipped} skipped, {already} already closed{failures}"));
    Ok(looked_at)
}

/// What run's state.json, as close_all first read it, records as its closed; None when it records none or cannot be
/// read.
fn closed_as_read(run: &SavedRun) -> Option<String> {
    run.state().ok().filter(|state| is_closed(state)).and_then(|state| state.closed.clone())
}

/// Read the run of the project at cwd whose id is run_id again, and close it as close_all says, or with dry_run say
/// what closing it would do; a run whose state.json is gone is skipped, as nothing is left to close. Errors: its
/// state.json cannot be read, or its close fails.
fn sweep_run(world: &Closer, cwd: &str, run_id: &str, dry_run: bool) -> Result<Outcome, OrchestratorError> {
    let Some(run) = world.host.run_state(cwd, run_id)? else {
        return Ok(Outcome::Skipped(format!(
            "run {run_id} is gone: its state.json was removed from {cwd}/{RUNS_DIR} since the sweep began"
        )));
    };
    Ok(match decide(world, cwd, &run)? {
        Decision::AlreadyClosed(state) => Outcome::AlreadyClosed(state.closed.clone().unwrap_or_default()),
        Decision::Refused(why) => Outcome::Skipped(why.to_string()),
        Decision::Close(plan) => {
            let done = plan.close(world, cwd, dry_run)?;
            Outcome::Closed { done, discarded: plan.discarded().to_vec() }
        }
    })
}

/// Whether state records that the run was closed.
fn is_closed(state: &RunState) -> bool {
    state.closed.as_deref().is_some_and(|c| !c.is_empty())
}

/// What closing run, of the project at cwd, would do now, as world's host and the run's herdr server find it; changes
/// nothing. Errors: herdr or git cannot tell what is there.
fn decide<'r>(world: &Closer, cwd: &str, run: &'r SavedRun) -> Result<Decision<'r>, OrchestratorError> {
    let host = world.host;
    let state = match run.state() {
        Ok(state) => state,
        Err(e) => return Ok(Decision::Refused(e)),
    };
    if is_closed(state) {
        return Ok(Decision::AlreadyClosed(state));
    }
    let health = run.health(world.local_host, world.pid_alive);
    if let Some((RUNNING, why)) = &health {
        return Ok(Decision::Refused(OrchestratorError(format!(
            "run {} is still running ({why}); stop it before closing it",
            state.run_id
        ))));
    }
    // Stale and not done: its orchestrator died mid-run. A done run stale while ending finished its change.
    let failed = state.error.as_deref().is_some_and(|e| !e.is_empty()) || (health.is_some() && !state.done());
    let workspace = match Some(state.workspace_id.as_str()).filter(|w| !w.is_empty()) {
        Some(w) if world.workspaces.workspace_exists(w, state.herdr_server.as_ref())? => Some(w),
        _ => None,
    };
    let worktree = match state.worktree.as_deref().filter(|w| !w.is_empty()) {
        Some(path) if failed => {
            let changes = match host.is_worktree(cwd, path)? {
                true => Some(git_status(host, path)?.lines().map(String::from).collect()),
                false => None,
            };
            WorktreeFate::Discarded { path, changes, branch: branch_fate(host, cwd, state)? }
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
    host: &dyn Host,
    cwd: &str,
    state: &'r RunState,
) -> Result<Option<(&'r str, BranchFate)>, OrchestratorError> {
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

impl<'a> Closer<'a> {
    /// What close talks to when run from world, saying each line through say.
    pub fn of(world: &Connected<'a>, say: &'a dyn Fn(&str)) -> Closer<'a> {
        Closer {
            host: world.with.host,
            workspaces: world.with.herdr,
            interrupt: world.with.interrupt,
            pid_alive: world.with.pid_alive,
            local_host: world.local_host,
            say,
        }
    }

    /// Say line, without its line end.
    fn say(&self, line: &str) {
        (self.say)(line);
    }
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

    /// What closing would do, for a dry run: each step with what it acts on, such as "close workspace w1; delete
    /// worktree /proj/.orchestrator/worktrees/a1b2c3; delete branch orchestrator/a1b2c3; delete the refs under
    /// refs/orchestrator/a1b2c3/". summary is what closing records instead, once it is done.
    fn preview(&self) -> String {
        let mut steps = vec![];
        if let Some(workspace) = self.workspace {
            steps.push(format!("close workspace {workspace}"));
        }
        match &self.worktree {
            WorktreeFate::None => {}
            WorktreeFate::Kept(path) => steps.push(format!("keep worktree {path}")),
            WorktreeFate::Discarded { path, changes, branch } => {
                if changes.is_some() {
                    steps.push(format!("delete worktree {path}"));
                }
                match branch {
                    Some((branch, BranchFate::Deleted)) => steps.push(format!("delete branch {branch}")),
                    Some((branch, BranchFate::Kept(why))) => steps.push(format!("keep branch {branch}: {why}")),
                    None => {}
                }
            }
        }
        if self.refs {
            steps.push(format!("delete the refs under {}", refs_prefix(&self.state.run_id)));
        }
        if steps.is_empty() {
            steps.push("record it as closed; nothing else is left".to_string());
        }
        steps.join("; ")
    }

    /// Carry out the plan on the run of the project at cwd, or with dry_run only say what it would do, as close_run
    /// says; return its summary, or with dry_run its preview. Errors: as close_run's for a step or the record.
    fn close(&self, world: &Closer, cwd: &str, dry_run: bool) -> Result<String, OrchestratorError> {
        let (host, run_id) = (world.host, &self.state.run_id);
        if dry_run {
            let preview = self.preview();
            self.say_changes(world, "would discard");
            world.say(&format!("would close run {run_id}: {preview}"));
            return Ok(preview);
        }
        let closed = self.summary();
        if let Some(workspace) = self.workspace {
            world.workspaces.close_workspace(workspace, self.state.herdr_server.as_ref())?;
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

    /// What goes with a discarded worktree: its uncommitted changes, one line of git status each; empty when the
    /// worktree is kept, clean or already gone, or the run has none.
    fn discarded(&self) -> &[String] {
        match &self.worktree {
            WorktreeFate::Discarded { changes: Some(changes), .. } => changes,
            _ => &[],
        }
    }

    /// Say what goes with a discarded worktree, if anything does, under a line "<doing> in <worktree>:".
    fn say_changes(&self, world: &Closer, doing: &str) {
        if let (WorktreeFate::Discarded { path, .. }, [_, ..]) = (&self.worktree, self.discarded()) {
            world.say(&format!("{doing} in {path}:"));
            for line in self.discarded() {
                world.say(&format!("    {line}"));
            }
        }
    }
}

#[cfg(test)]
mod tests;
