//! Moving a project's own directory from the name an engine from before the rename to Octoflow AI gave it,
//! `.orchestrator`, to PROJECT_DIR. The move happens at most once, on the first start that finds the old name and not
//! the new one, and says so; once nobody can still have the old name, this module goes.

use serde_json::Value;

use crate::constants::{PROJECT_DIR, WORKTREES_DIR};
use crate::error::OrchestratorError;
use crate::host::{run_states_in, Host, DEFAULT_TIMEOUT};

/// A project's own directory under its old name, relative to the project, as PROJECT_DIR is.
const OLD_PROJECT_DIR: &str = ".orchestrator";

/// Move the project at cwd's own directory, on host, from its old name to PROJECT_DIR, when the old one is an older
/// engine's (it has a runs directory, which forget leaves even when it deleted every run), every run in it is
/// closed, and nothing is at PROJECT_DIR yet. A closed run is never resumed, so no agent or resume goes on using the
/// old paths. Each worktree under it, which close keeps for a run that ended well until prune deletes it, is
/// registered with git again at its new path, and its run's saved worktree names that path. cwd is the project's
/// absolute directory, as Host::resolve_dir gives it; dry_run is true for a command that promises to change
/// nothing, which a due move refuses instead.
///
/// Returns the line for standard error saying what moved, and naming each worktree git could not register again or
/// whose run could not be saved with its new path, each with what to do by hand; None when nothing had to move: no
/// engine's directory under the old name, something at PROJECT_DIR already, or another engine moved it first.
///
/// Errors: a run under the old name is not closed, which the error names, so that the human finishes and closes it
/// with the older engine first; dry_run and a move is due; a state under the old name cannot be read; or the rename
/// fails. Once the rename is made, what fails after it is named in the returned line instead.
pub fn move_project_dir(host: &dyn Host, cwd: &str, dry_run: bool) -> Result<Option<String>, OrchestratorError> {
    let (old, new) = (format!("{cwd}/{OLD_PROJECT_DIR}"), format!("{cwd}/{PROJECT_DIR}"));
    if !host.exists(&old)? || host.exists(&new)? {
        return Ok(None);
    }
    // Another tool may keep a directory by that name.
    if !host.exists(&format!("{old}/runs"))? {
        return Ok(None);
    }
    let runs = run_states_in(host, cwd, &format!("{OLD_PROJECT_DIR}/runs"))?;
    let open: Vec<&str> = runs.iter().map(|r| r.shown()).filter(|s| !s.is_closed()).map(|s| s.run_id.as_str()).collect();
    if !open.is_empty() {
        return Err(OrchestratorError(format!(
            "{old} holds runs an older engine left open: {}; this engine moves it to {new} once every run there is \
             closed: finish or stop each with that engine and close it there, then start again",
            open.join(", ")
        )));
    }
    if dry_run {
        return Err(OrchestratorError(format!(
            "{old} holds this project's runs, which the first command that is not a dry run moves to {new}"
        )));
    }
    if let Err(e) = host.rename(&old, &new) {
        // Another engine's first command, serve's for one, may have moved it between the checks and here.
        return if !host.exists(&old)? && host.exists(&new)? { Ok(None) } else { Err(e) };
    }
    // Each worktree's .git file and the project's record of it name each other by absolute path; repair, given the
    // worktree's new path, rewrites both. A directory there that is no worktree is left alone, and one repair that
    // fails, printed, does not stop the others.
    let repair = "cd -- \"$1\" || exit 1; for d in \"$2\"/*/; do [ -f \"$d.git\" ] || continue; \
                  git worktree repair \"${d%/}\" >/dev/null 2>&1 || printf '%s\\n' \"${d%/}\"; done";
    let worktrees = format!("{cwd}/{WORKTREES_DIR}");
    let mut said = format!("moved the project's runs from {old} to {new}");
    match host.check(&["sh", "-c", repair, "_", cwd, &worktrees], None, DEFAULT_TIMEOUT) {
        Ok(unrepaired) => {
            for tree in unrepaired.lines() {
                said.push_str(&format!(
                    "; git could not register the worktree {tree} again: run `git worktree repair {tree}` in {cwd}"
                ));
            }
        }
        Err(e) => said.push_str(&format!("; {e}: run `git worktree repair` on each worktree in {worktrees}")),
    }
    // A run's worktree is WORKTREES_DIR/<its key> (cli::start), under whatever spelling of the project's path the run
    // was started with; prune finds a kept one only by its saved path.
    for run in runs.iter().map(|r| r.shown()) {
        let old_tail = format!("/{OLD_PROJECT_DIR}/worktrees/{}", run.key());
        if !run.worktree.as_deref().is_some_and(|w| w.ends_with(&old_tail)) {
            continue;
        }
        let state = format!("{new}/runs/{}/state.json", run.run_id);
        let tree = format!("{cwd}/{WORKTREES_DIR}/{}", run.key());
        if let Err(e) = resave_worktree(host, &state, &tree) {
            said.push_str(&format!("; {e}: set its worktree to {tree} by hand"));
        }
    }
    Ok(Some(said))
}

/// Set the worktree of the run whose state.json is at path to worktree, keeping the rest of the file, and its
/// modification time, the run's heartbeat (Host::write's keep_mtime). Not crate::cli::record_in_state, which writes
/// the state as this engine reads it: a run saved in a format it cannot read (SavedRun::problem) would lose what it
/// cannot read. Errors: the file is missing, cannot be read or written, or is not a JSON object.
fn resave_worktree(host: &dyn Host, path: &str, worktree: &str) -> Result<(), OrchestratorError> {
    let Some(text) = host.read(path)? else { return Err(OrchestratorError(format!("{path} is missing"))) };
    let mut saved: Value = serde_json::from_str(&text).map_err(|e| OrchestratorError(format!("{path}: {e}")))?;
    let Some(fields) = saved.as_object_mut() else {
        return Err(OrchestratorError(format!("{path}: not a JSON object")));
    };
    fields.insert("worktree".to_string(), Value::String(worktree.to_string()));
    host.write(path, &saved.to_string(), true)
}

#[cfg(test)]
mod tests;
