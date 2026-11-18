//! Cleaning up after a run nothing drives any more: the `close` command, for one run or every run of a project.

use super::{record_in_state, Connected};
use crate::constants::{EXIT_ERROR, RUNS_DIR};
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
    /// Where close prints, which takes text as Connected::stdout does, a line with its line end.
    pub stdout: &'a dyn Fn(&str),
}

/// One run close_all looked at, and what became of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunClosed {
    /// The run's id.
    pub run_id: String,
    /// What close_all did with the run, or with a dry run would do.
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
    /// Not closed, and why: Decision::Refused's reason, the run is gone, or it changed since the preview of a
    /// Sweep::Previewed.
    Skipped(String),
    /// Its close failed; what it did before the failure stays done.
    Failed {
        /// Why it failed.
        error: String,
        /// The uncommitted changes that went with the run's worktree before the failure, one line of git status each;
        /// empty when the worktree is still there, or the run had none to lose.
        discarded: Vec<String>,
    },
}

/// Which runs close_all closes, and whether it closes them at all.
#[derive(Clone, Copy)]
pub enum Sweep<'p> {
    /// Close none: say what closing each run would do.
    DryRun,
    /// Close every run as it stands when its turn comes.
    Close,
    /// Close only the runs that this earlier DryRun of the project gave as Outcome::Closed, each only while closing it
    /// would still do just what the dry run said, discarding the same changes, so that what goes is what was shown; a
    /// run that changed since is skipped, saying what closing it would now do. No other run is looked at. The changes
    /// are compared line by line of git status (crate::host::GIT_STATUS_ARGS), so a file listed may have changed
    /// further, and ignored files, which git status does not list, go with the worktree unshown.
    Previewed(&'p [RunClosed]),
}

/// What a dry run said closing a run would do and discard: its Plan::preview and Plan::discarded.
type Preview<'p> = (&'p str, &'p [String]);

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

/// The `close` command in the project at cwd, run from world: close_run of the run run_ref names, or without one
/// close_all, each with dry_run changing nothing. Returns the exit status: EXIT_ERROR when a run of close_all failed,
/// else 0. Errors: as close_run's, or close_all's.
pub fn close_command(
    world: &Connected,
    cwd: &str,
    run_ref: Option<&str>,
    dry_run: bool,
) -> Result<i32, OrchestratorError> {
    let closer = Closer::of(world);
    match run_ref {
        Some(run_ref) => close_run(&closer, cwd, run_ref, dry_run).map(|_| 0),
        None => {
            let runs = close_all(&closer, cwd, if dry_run { Sweep::DryRun } else { Sweep::Close })?;
            Ok(if runs.iter().any(|run| matches!(run.outcome, Outcome::Failed { .. })) { EXIT_ERROR } else { 0 })
        }
    }
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

/// Close the runs of the project at cwd that sweep names as close_run does, or with Sweep::DryRun say what closing
/// each would do, then say a count of the runs closed (or to close), skipped and already closed, and failed if any.
/// The runs are every run of the project, in run id order, or a Sweep::Previewed's, in the preview's order. A
/// Decision::Refused run is skipped with a line saying why; one already closed is only counted. Each run is read again
/// just before it is closed, so one that started running, or was removed, since the sweep began is skipped. A run
/// whose close fails gets a line with the error, and the rest are still closed. A raised Interrupt starts no further
/// run; the run being closed then finishes or fails as close_run would, and a second sweep finishes it.
///
/// Returns each run looked at, in that order; none after an Interrupt. Errors: for Sweep::DryRun and Sweep::Close,
/// the project's runs cannot be read at the start. A Sweep::Previewed reads each run only at its turn: one whose
/// state.json cannot be read, or is not a JSON object naming its run, is Outcome::Failed; one whose format or fields
/// do not read (SavedRun::problem) is skipped as Decision::Refused.
pub fn close_all(world: &Closer, cwd: &str, sweep: Sweep) -> Result<Vec<RunClosed>, OrchestratorError> {
    // Each run to look at: its id, what its state.json records as its closed when first read, and for a
    // Sweep::Previewed what the preview said closing it would do and discard.
    let runs: Vec<(String, Option<String>, Option<Preview>)> = match sweep {
        Sweep::Previewed(preview) => preview
            .iter()
            .filter_map(|run| match &run.outcome {
                Outcome::Closed { done, discarded } => {
                    Some((run.run_id.clone(), None, Some((done.as_str(), &discarded[..]))))
                }
                _ => None,
            })
            .collect(),
        Sweep::DryRun | Sweep::Close => {
            let mut runs: Vec<_> = world
                .host
                .run_states(cwd)?
                .iter()
                .map(|run| (run.shown().run_id.clone(), closed_as_read(run), None))
                .collect();
            runs.sort_by(|a, b| a.0.cmp(&b.0));
            runs
        }
    };
    let dry_run = matches!(sweep, Sweep::DryRun);
    let mut looked_at = vec![];
    for (run_id, recorded, previewed) in runs {
        if world.interrupt.is_raised() {
            break;
        }
        // A closed run stays closed: resume refuses it.
        let outcome = match recorded {
            Some(recorded) => Outcome::AlreadyClosed(recorded),
            None => sweep_run(world, cwd, &run_id, dry_run, previewed)
                .unwrap_or_else(|e| Outcome::Failed { error: e.to_string(), discarded: vec![] }),
        };
        match &outcome {
            Outcome::Skipped(why) => world.say(&format!("skipped: {why}")),
            Outcome::Failed { error, .. } => world.say(&format!("could not close run {run_id}: {error}")),
            Outcome::Closed { .. } | Outcome::AlreadyClosed(_) => {}
        }
        looked_at.push(RunClosed { run_id, outcome });
    }
    let count = |of: fn(&Outcome) -> bool| looked_at.iter().filter(|r| of(&r.outcome)).count();
    let closed = count(|o| matches!(o, Outcome::Closed { .. }));
    let skipped = count(|o| matches!(o, Outcome::Skipped(_)));
    let already = count(|o| matches!(o, Outcome::AlreadyClosed(_)));
    let failed = count(|o| matches!(o, Outcome::Failed { .. }));
    let verb = if dry_run { "to close" } else { "closed" };
    let failures = if failed > 0 { format!(", {failed} failed") } else { String::new() };
    world.say(&format!("{closed} {verb}, {skipped} skipped, {already} already closed{failures}"));
    Ok(looked_at)
}

/// What run's state.json, as close_all first read it, records as its closed; None when it records none or cannot be
/// read.
fn closed_as_read(run: &SavedRun) -> Option<String> {
    run.state().ok().filter(|state| state.is_closed()).and_then(|state| state.closed.clone())
}

/// Read the run of the project at cwd whose id is run_id again, and close it as close_all says, or with dry_run say
/// what closing it would do; a run whose state.json is gone is skipped, as nothing is left to close. With previewed,
/// a run whose plan is no longer what the dry run said is skipped. A close that
/// fails is Outcome::Failed. Errors: its state.json cannot be read, or herdr or git cannot tell what is there.
fn sweep_run(
    world: &Closer,
    cwd: &str,
    run_id: &str,
    dry_run: bool,
    previewed: Option<Preview>,
) -> Result<Outcome, OrchestratorError> {
    let Some(run) = world.host.run_state(cwd, run_id)? else {
        return Ok(Outcome::Skipped(format!(
            "run {run_id} is gone: its state.json was removed from {cwd}/{RUNS_DIR} since the sweep began"
        )));
    };
    Ok(match decide(world, cwd, &run)? {
        Decision::AlreadyClosed(state) => Outcome::AlreadyClosed(state.closed.clone().unwrap_or_default()),
        Decision::Refused(why) => Outcome::Skipped(why.to_string()),
        Decision::Close(plan) => {
            if previewed.is_some_and(|(done, discarded)| plan.preview() != done || plan.discarded() != discarded) {
                return Ok(Outcome::Skipped(format!(
                    "run {run_id} changed since the preview: closing it would now {}",
                    plan.preview()
                )));
            }
            match plan.close(world, cwd, dry_run) {
                Ok(done) => Outcome::Closed { done, discarded: plan.discarded().to_vec() },
                Err(e) => Outcome::Failed { error: e.to_string(), discarded: plan.discarded_by_now(world.host, cwd) },
            }
        }
    })
}

/// What closing run, of the project at cwd, would do now, as world's host and the run's herdr server find it; changes
/// nothing. Errors: herdr or git cannot tell what is there.
fn decide<'r>(world: &Closer, cwd: &str, run: &'r SavedRun) -> Result<Decision<'r>, OrchestratorError> {
    let host = world.host;
    let state = match run.state() {
        Ok(state) => state,
        Err(e) => return Ok(Decision::Refused(e)),
    };
    if state.is_closed() {
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
        let unreadable =
            |e: OrchestratorError| OrchestratorError(format!("could not read the refs under {prefix}: {e}"));
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
    /// What close talks to when run from world, printing on its standard output.
    pub fn of(world: &Connected<'a>) -> Closer<'a> {
        Closer {
            host: world.with.host,
            workspaces: world.with.herdr,
            interrupt: world.with.interrupt,
            pid_alive: world.with.pid_alive,
            local_host: world.local_host,
            stdout: world.stdout,
        }
    }

    /// Print line on stdout; line has no line end, which this adds.
    fn say(&self, line: &str) {
        (self.stdout)(&format!("{line}\n"));
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
    /// worktree /proj/.octoflow-ai/worktrees/a1b2c3; delete branch orchestrator/a1b2c3; delete the refs under
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

    /// What went with a discarded worktree by now, as discarded says, for a close that failed part way: empty while
    /// the worktree is still one.
    fn discarded_by_now(&self, host: &dyn Host, cwd: &str) -> Vec<String> {
        let WorktreeFate::Discarded { path, changes: Some(changes), .. } = &self.worktree else { return vec![] };
        // When git cannot tell, the changes are given as gone: a warning of a loss that did not happen costs less
        // than silence about one that did.
        match host.is_worktree(cwd, path) {
            Ok(true) => vec![],
            Ok(false) | Err(_) => changes.clone(),
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
