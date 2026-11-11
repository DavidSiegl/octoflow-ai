//! Cleaning up after a run nothing drives any more, as orchestrator.py's close_run: the `close` command.

use super::{record_in_state, Connected};
use crate::constants::RUNS_DIR;
use crate::error::OrchestratorError;
use crate::herdr::Herdr;
use crate::host::git_status;
use crate::runs::RUNNING;
use crate::state::{find_run, refs_prefix, RunState};

/// Clean up after the run of the project at cwd that run_ref names, by its id or key, on world's herdr and host,
/// record what was done as the run's closed in its state.json, so that resume refuses it, and return that. Prints
/// each step on world's standard output.
///
/// Every run loses its workspace and its local refs (crate::state::refs_prefix), which hold the files its steps locked
/// and which nothing else deletes once it is closed. A failed or stale --worktree run also loses its worktree, changes
/// and all (listed first), and its branch if that holds no commit beyond the run's base: the Builder never commits, so
/// such a branch has nothing to lose. A finished run's worktree is a --no-pr run's change, and an in-place run's
/// checkout and branch are the human's to sort out, so both stay. Each step skips what is already gone, so a close that
/// failed part way can be run again; closed is recorded only once every step has succeeded. A run already closed is
/// left as it is, and its closed returned.
///
/// Errors: no run or several match run_ref; the run's state cannot be read (SavedRun::problem, which says to close it
/// with the release that saved it); the run is still running, or done but still ending (RunState::health, as a done
/// run still ending may be deleting its refs itself, Workflow::clean_up_locks); a step fails; or the state.json
/// changed since it was read.
pub fn close_run(world: &Connected, cwd: &str, run_ref: &str) -> Result<String, OrchestratorError> {
    let host = world.with.host;
    let runs = host.run_states(cwd)?;
    let run = find_run(&runs, run_ref, cwd)?;
    let state = run.state()?;
    if let Some(closed) = state.closed.as_deref().filter(|c| !c.is_empty()) {
        world.say(&format!("run {} is already closed: {closed}", state.run_id));
        return Ok(closed.to_string());
    }
    let health = run.health(world.local_host, world.with.pid_alive);
    if let Some((RUNNING, why)) = &health {
        return Err(OrchestratorError(format!(
            "run {} is still running ({why}); stop it before closing it",
            state.run_id
        )));
    }
    // Stale and not done: its orchestrator died mid-run. A done run stale while ending finished its change.
    let failed = state.error.as_deref().is_some_and(|e| !e.is_empty()) || (health.is_some() && !state.done());
    let mut done = vec![close_run_workspace(world, &state.workspace_id)?];
    match state.worktree.as_deref().filter(|w| !w.is_empty()) {
        Some(_) if failed => done.extend(discard_run_worktree(world, cwd, state)?),
        Some(worktree) if host.is_worktree(cwd, worktree)? => done.push(format!("worktree kept: {worktree}")),
        _ => {}
    }
    done.extend(delete_run_refs(world, cwd, state)?);
    let closed = done.join("; ");
    let path = format!("{cwd}/{RUNS_DIR}/{}/state.json", state.run_id);
    record_in_state(host, &path, state, |s| s.closed = Some(closed.clone()), "the run was closed")?;
    world.say(&format!("closed run {}: {closed}", state.run_id));
    Ok(closed)
}

/// Close the herdr workspace, unless it is already gone or the run never had one, and say which.
fn close_run_workspace(world: &Connected, workspace: &str) -> Result<String, OrchestratorError> {
    let herdr: &dyn Herdr = world.with.herdr;
    if !workspace.is_empty() && herdr.workspace_exists(workspace)? {
        herdr.close_workspace(workspace)?;
        world.say(&format!("closed workspace {workspace}"));
        return Ok("workspace closed".to_string());
    }
    Ok("workspace already gone".to_string())
}

/// Delete the local refs of the run of state from cwd's repository, which its worktree shares, and say so if there
/// were any. "refs deleted" when the run locked files, the refs a close that failed part way deleted counting, or
/// when it had refs nonetheless (its first lock failed after making one); None for a run with none. A run that locked
/// nothing in a cwd that is no git repository with a commit, such as a --no-pr run's, has none, and no git command
/// runs. Errors: a ref could not be deleted, or the run locked files and git cannot read cwd's repository, the error
/// naming the run's refs prefix.
fn delete_run_refs(world: &Connected, cwd: &str, state: &RunState) -> Result<Option<String>, OrchestratorError> {
    let host = world.with.host;
    if state.locked.is_empty() && host.git_head(cwd)?.is_none() {
        return Ok(None);
    }
    let prefix = refs_prefix(&state.run_id);
    let deleted = host
        .delete_refs(cwd, &prefix)
        .map_err(|e| OrchestratorError(format!("could not delete the refs under {prefix}: {e}")))?;
    if deleted {
        world.say(&format!("deleted the refs under {prefix}"));
    }
    Ok((deleted || !state.locked.is_empty()).then(|| "refs deleted".to_string()))
}

/// Force-remove the worktree of cwd that the failed run of state built in, after listing the changes that go with
/// it, then its branch (discard_run_branch); say what became of each.
fn discard_run_worktree(world: &Connected, cwd: &str, state: &RunState) -> Result<Vec<String>, OrchestratorError> {
    let host = world.with.host;
    let worktree = state.worktree.as_deref().unwrap_or_default();
    let mut done = vec![];
    if host.is_worktree(cwd, worktree)? {
        let status = git_status(host, worktree)?;
        let changes: Vec<&str> = status.lines().collect();
        if !changes.is_empty() {
            world.say(&format!("discarding in {worktree}:"));
            for line in changes {
                world.say(&format!("    {line}"));
            }
        }
        host.remove_worktree(cwd, worktree, true)?;
        world.say(&format!("deleted worktree {worktree}"));
        done.push("worktree deleted".to_string());
    } else {
        done.push("worktree already gone".to_string());
    }
    // Even with the worktree gone: a close that failed at the branch must find it again.
    if let Some(branch) = state.branch.as_deref().filter(|b| !b.is_empty()) {
        if host.branch_commit(cwd, branch)?.is_some() {
            done.push(discard_run_branch(world, cwd, state, branch)?);
        }
    }
    Ok(done)
}

/// Delete the run of state's branch of cwd unless it has commits beyond the run's base, or the run has no base to
/// tell; say which.
fn discard_run_branch(world: &Connected, cwd: &str, state: &RunState, branch: &str) -> Result<String, OrchestratorError> {
    let host = world.with.host;
    let why = match state.base.as_deref() {
        None => "the run has no base to compare it with".to_string(),
        Some(base) => match host.commits_beyond(cwd, base, branch)? {
            0 => {
                host.delete_branch(cwd, branch)?;
                world.say(&format!("deleted branch {branch}"));
                return Ok("branch deleted".to_string());
            }
            n => format!("{n} commit{} beyond base", if n == 1 { "" } else { "s" }),
        },
    };
    world.say(&format!("kept branch {branch}: {why}"));
    Ok(format!("branch kept: {why}"))
}

#[cfg(test)]
mod tests;
