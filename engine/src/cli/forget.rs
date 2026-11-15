//! Deleting what is left of closed runs: the `forget` command, for one run or every closed run of a project.

use super::prune::awaits_prune;
use super::Connected;
use crate::constants::EXIT_ERROR;
use crate::error::OrchestratorError;
use crate::state::{find_run, run_dir, SavedRun};

/// Why a run is not forgotten, as it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Left {
    /// It is not closed.
    NotClosed,
    /// It is closed, but prune has yet to delete its branch, origin's and its worktree, and finds them only through
    /// its state (crate::cli::prune::awaits_prune).
    AwaitsPrune,
    /// Its state cannot be read (SavedRun::problem), so that neither is known; why.
    Unreadable(String),
}

/// What forget_all did with one run, or with a dry run would do.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    /// Its directory deleted, its metrics kept (Host::forget_run); with a dry run, to be.
    Forgotten,
    /// Left as it is, and why.
    Left(Left),
    /// Its forget failed, and why; part of its directory may be gone.
    Failed(String),
}

/// The `forget` command in the project at cwd, run from world: delete the directory of the closed run run_ref names,
/// or without one of every closed run, keeping their metrics (Host::forget_run), and print what became of each; with
/// dry_run, print what would go and change nothing. A run that is not closed, or whose branch prune has yet to delete,
/// is refused alone and left by a sweep, as is a run whose state cannot be read; a run whose directory cannot be
/// deleted does not stop a sweep. A raised Interrupt starts no further run.
///
/// Returns the exit status: EXIT_ERROR when a run of a sweep failed, else 0. Errors: the project's runs cannot be read;
/// for run_ref, no run or several match it, or it is left as decide says, or forgetting it fails.
pub fn forget_command(world: &Connected, cwd: &str, run_ref: Option<&str>, dry_run: bool) -> Result<i32, OrchestratorError> {
    let runs = world.with.host.run_states(cwd)?;
    let Some(run_ref) = run_ref else {
        return Ok(if forget_all(world, cwd, runs, dry_run) { EXIT_ERROR } else { 0 });
    };
    let run = find_run(&runs, run_ref, cwd)?;
    let run_id = &run.shown().run_id;
    match decide(run) {
        Ok(()) => forget(world, cwd, run_id, dry_run).map(|()| 0),
        Err(Left::NotClosed) => {
            Err(OrchestratorError(format!("run {run_id} is not closed: close it before forgetting it")))
        }
        Err(Left::AwaitsPrune) => Err(OrchestratorError(format!(
            "run {run_id} keeps its branch and worktree for prune, which deletes them once its pull request is \
             merged or closed: forget it after that"
        ))),
        Err(Left::Unreadable(why)) => Err(OrchestratorError(why)),
    }
}

/// Forget each closed run of runs, those of the project at cwd, in run id order, as forget_command says, printing a
/// line per run forgotten, skipped or failed, then a count of each outcome. Returns whether some run failed.
fn forget_all(world: &Connected, cwd: &str, mut runs: Vec<SavedRun>, dry_run: bool) -> bool {
    runs.sort_by(|a, b| a.shown().run_id.cmp(&b.shown().run_id));
    let mut outcomes = vec![];
    for run in &runs {
        if world.with.interrupt.is_raised() {
            break;
        }
        outcomes.push(sweep_one(world, cwd, run, dry_run));
    }
    let count = |of: fn(&Outcome) -> bool| outcomes.iter().filter(|o| of(o)).count();
    let forgotten = count(|o| *o == Outcome::Forgotten);
    let open = count(|o| *o == Outcome::Left(Left::NotClosed));
    let pruning = count(|o| *o == Outcome::Left(Left::AwaitsPrune));
    let skipped = count(|o| matches!(o, Outcome::Left(Left::Unreadable(_))));
    let failed = count(|o| matches!(o, Outcome::Failed(_)));
    let verb = if dry_run { "to forget" } else { "forgotten" };
    let pruning = if pruning > 0 { format!(", {pruning} waiting for prune") } else { String::new() };
    let failures = if failed > 0 { format!(", {failed} failed") } else { String::new() };
    world.say(&format!("{forgotten} {verb}, {open} not closed{pruning}, {skipped} skipped{failures}"));
    failed > 0
}

/// forget_all's step for one run: forget run if decide lets it, saying why when it is skipped as unreadable or its
/// forget fails, and silent when it is left for being open or awaiting prune, which only the count reports.
fn sweep_one(world: &Connected, cwd: &str, run: &SavedRun, dry_run: bool) -> Outcome {
    let run_id = &run.shown().run_id;
    let outcome = match decide(run) {
        Ok(()) => match forget(world, cwd, run_id, dry_run) {
            Ok(()) => Outcome::Forgotten,
            Err(e) => Outcome::Failed(e.0),
        },
        Err(left) => Outcome::Left(left),
    };
    match &outcome {
        Outcome::Left(Left::Unreadable(why)) => world.say(&format!("skipped: {why}")),
        Outcome::Failed(why) => world.say(&format!("could not forget run {run_id}: {why}")),
        Outcome::Forgotten | Outcome::Left(Left::NotClosed | Left::AwaitsPrune) => {}
    }
    outcome
}

/// Whether run may be forgotten, as it was read, or why not: only a closed run may, as nothing drives it and resume
/// refuses it, and of those only one whose branch prune is done with.
fn decide(run: &SavedRun) -> Result<(), Left> {
    let state = run.state().map_err(|e| Left::Unreadable(e.0))?;
    if !state.is_closed() {
        Err(Left::NotClosed)
    } else if awaits_prune(state) {
        Err(Left::AwaitsPrune)
    } else {
        Ok(())
    }
}

/// Forget the run of the project at cwd whose id is run_id (Host::forget_run), saying so, or with dry_run only say
/// what would go. Errors: Host::forget_run's.
fn forget(world: &Connected, cwd: &str, run_id: &str, dry_run: bool) -> Result<(), OrchestratorError> {
    let dir = run_dir(cwd, run_id);
    if dry_run {
        world.say(&format!("would forget run {run_id}: delete {dir}"));
        return Ok(());
    }
    world.with.host.forget_run(cwd, run_id)?;
    world.say(&format!("forgot run {run_id}: deleted {dir}"));
    Ok(())
}

#[cfg(test)]
mod tests;
