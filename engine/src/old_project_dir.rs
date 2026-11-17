//! Moving a project's own directory from the name an engine from before the rename to Octoflow AI gave it,
//! `.orchestrator`, to PROJECT_DIR. The move happens at most once, on the first start that finds the old name and not
//! the new one, and says so; once nobody can still have the old name, this module goes.

use crate::constants::{PROJECT_DIR, WORKTREES_DIR};
use crate::error::OrchestratorError;
use crate::host::{run_states_in, Host, DEFAULT_TIMEOUT};

/// A project's own directory under its old name, relative to the project, as PROJECT_DIR is.
const OLD_PROJECT_DIR: &str = ".orchestrator";

/// Move the project at cwd's own directory, on host, from its old name to PROJECT_DIR, when the old one holds an
/// older engine's runs, every one of them closed, and nothing is at PROJECT_DIR yet. A closed run is never resumed,
/// so nothing goes on reading or writing the old paths its state and its agents' prompts name. Each worktree under
/// it, which close keeps for a run that ended well, is registered with git again at its new path. cwd is the
/// project's absolute directory, as Host::resolve_dir gives it; dry_run is true for a command that promises to
/// change nothing, which a due move refuses instead.
///
/// Returns the line for standard error saying what moved, and naming each worktree git could not register again,
/// to be repaired by hand; None when nothing had to move: no run under the old name, something at PROJECT_DIR
/// already, or another engine moved it first.
///
/// Errors: a run under the old name is not closed, which the error names, so that the human finishes and closes it
/// with the older engine first; dry_run and a move is due; a state under the old name cannot be read; or the
/// rename fails.
pub fn move_project_dir(host: &dyn Host, cwd: &str, dry_run: bool) -> Result<Option<String>, OrchestratorError> {
    let (old, new) = (format!("{cwd}/{OLD_PROJECT_DIR}"), format!("{cwd}/{PROJECT_DIR}"));
    if !host.exists(&old)? || host.exists(&new)? {
        return Ok(None);
    }
    // Runs are the mark of an engine's directory: another tool may keep a directory by that name.
    let runs = run_states_in(host, cwd, &format!("{OLD_PROJECT_DIR}/runs"))?;
    if runs.is_empty() {
        return Ok(None);
    }
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
    let unrepaired = host.check(&["sh", "-c", repair, "_", cwd, &worktrees], None, DEFAULT_TIMEOUT)?;
    let mut said = format!("moved the project's runs from {old} to {new}");
    for tree in unrepaired.lines() {
        said.push_str(&format!("; git could not register the worktree {tree} again: run `git worktree repair {tree}` in {cwd}"));
    }
    Ok(Some(said))
}

#[cfg(test)]
mod tests;
