//! Moving a project's own directory from the name an engine from before the rename to Octoflow AI gave it,
//! `.orchestrator`, to PROJECT_DIR. The move happens at most once, on the first start that finds the old name and not
//! the new one, and says so; once nobody can still have the old name, this module goes.

use serde_json::Value;

use crate::constants::{PROJECT_DIR, WORKTREES_DIR};
use crate::error::OrchestratorError;
use crate::host::{run_states_in, Host, DEFAULT_TIMEOUT};
use crate::runs::RUNNING;

/// A project's own directory under its old name, relative to the project, as PROJECT_DIR is.
const OLD_PROJECT_DIR: &str = ".orchestrator";

/// Move the project at cwd's own directory, on host, from its old name to PROJECT_DIR, when the old one exists and
/// nothing is at PROJECT_DIR yet, so that its runs, their worktrees and state go on under the new name: each
/// worktree under it stays registered with git at its new path, and a run's saved worktree (RunState::worktree)
/// names that path. cwd is the project's absolute directory, as Host::resolve_dir gives it; local_host and pid_alive
/// tell whether a run is live, as SavedRun::health takes them. Returns the line saying what moved, for standard
/// error, or None when nothing had to.
///
/// Errors: a run under the old name is live (SavedRun::health's RUNNING), driven by an engine from before the rename,
/// which would go on writing there: nothing moves, and the error names the run, so that the human lets it end first;
/// a state under the old name cannot be read; or a step of the move fails, which the error names. A move that fails
/// after the rename, or is interrupted there, leaves git's records of the worktrees or the saved worktree paths
/// pointing at the old place, which no later start repairs: `git worktree repair` does the one, editing state.json
/// the other.
pub fn move_project_dir(
    host: &dyn Host,
    cwd: &str,
    local_host: &str,
    pid_alive: &dyn Fn(i64) -> bool,
) -> Result<Option<String>, OrchestratorError> {
    let (old, new) = (format!("{cwd}/{OLD_PROJECT_DIR}"), format!("{cwd}/{PROJECT_DIR}"));
    if !host.exists(&old)? || host.exists(&new)? {
        return Ok(None);
    }
    let runs = run_states_in(host, cwd, &format!("{OLD_PROJECT_DIR}/runs"))?;
    for run in &runs {
        if let Some((RUNNING, why)) = run.health(local_host, pid_alive) {
            return Err(OrchestratorError(format!(
                "{old} holds run {}, which an older engine drives ({why}); this one moves the directory to {new} \
                 once no run there is live: let that run end, or stop it, then start again",
                run.shown().run_id
            )));
        }
    }
    host.rename(&old, &new)?;
    // Each worktree's .git file and the project's record of it name each other by absolute path; repair, given the
    // worktree's new path, rewrites both. A directory there that is no worktree is left alone.
    let repair = "cd -- \"$1\" || exit 1; for d in \"$2\"/*/; do [ -f \"$d.git\" ] || continue; \
                  git worktree repair \"${d%/}\" || exit 1; done";
    let worktrees = format!("{cwd}/{WORKTREES_DIR}");
    host.check(&["sh", "-c", repair, "_", cwd, &worktrees], None, DEFAULT_TIMEOUT)?;
    for run in &runs {
        let moved = run.shown().worktree.as_deref().and_then(|w| w.strip_prefix(&format!("{old}/")));
        if let Some(rest) = moved {
            resave_worktree(host, &format!("{new}/runs/{}/state.json", run.shown().run_id), &format!("{new}/{rest}"))?;
        }
    }
    Ok(Some(format!("moved the project's runs from {old} to {new}")))
}

/// Set the worktree of the run whose state.json is at path to worktree, keeping the rest of the file and its
/// modification time, which is the run's heartbeat (Host::write's keep_mtime). Not crate::cli::record_in_state,
/// which writes the state as this engine reads it: a run saved in a format it cannot read (SavedRun::problem) would
/// lose what it cannot read. Nothing when the file is gone. Errors: it cannot be read or written, or is not a JSON
/// object.
fn resave_worktree(host: &dyn Host, path: &str, worktree: &str) -> Result<(), OrchestratorError> {
    let Some(text) = host.read(path)? else { return Ok(()) };
    let mut saved: Value = serde_json::from_str(&text).map_err(|e| OrchestratorError(format!("{path}: {e}")))?;
    let Some(fields) = saved.as_object_mut() else {
        return Err(OrchestratorError(format!("{path}: not a JSON object")));
    };
    fields.insert("worktree".to_string(), Value::String(worktree.to_string()));
    host.write(path, &saved.to_string(), true)
}

#[cfg(test)]
mod tests;
