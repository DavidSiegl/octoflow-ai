//! Deleting what is left of closed runs: the `forget` command, for one run or every closed run of a project.

use super::Connected;
use crate::constants::{EXIT_ERROR, RUNS_DIR};
use crate::error::OrchestratorError;
use crate::state::{find_run, SavedRun};

/// What forget_all did with one run, or with a dry run would do.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    /// Its directory deleted, its metrics kept (Host::forget_run); with a dry run, to be.
    Forgotten,
    /// Left as it is: it is not closed.
    NotClosed,
    /// Left as it is, and why: its state cannot be read (SavedRun::problem), so that whether it is closed is not known.
    Skipped(String),
    /// Its forget failed, and why; part of its directory may be gone.
    Failed(String),
}

/// The `forget` command in the project at cwd, run from world: delete the directory of the closed run run_ref names,
/// or without one of every closed run, keeping their metrics (Host::forget_run), and print what became of each; with
/// dry_run, print what would go and change nothing. A single run that is not closed is refused; a sweep leaves such
/// runs and those whose state cannot be read, and a run whose directory cannot be deleted does not stop it. A raised
/// Interrupt starts no further run.
///
/// Returns the exit status: EXIT_ERROR when a run of a sweep failed, else 0. Errors: the project's runs cannot be read;
/// for run_ref, no run or several match it, its state cannot be read, it is not closed, or forgetting it fails.
pub fn forget_command(world: &Connected, cwd: &str, run_ref: Option<&str>, dry_run: bool) -> Result<i32, OrchestratorError> {
    let runs = world.with.host.run_states(cwd)?;
    let Some(run_ref) = run_ref else {
        return Ok(if forget_all(world, cwd, runs, dry_run) { EXIT_ERROR } else { 0 });
    };
    let run = find_run(&runs, run_ref, cwd)?;
    match decide(run) {
        Outcome::Forgotten => forget(world, cwd, &run.shown().run_id, dry_run).map(|()| 0),
        Outcome::NotClosed => Err(OrchestratorError(format!(
            "run {} is not closed: close it before forgetting it",
            run.shown().run_id
        ))),
        Outcome::Skipped(why) | Outcome::Failed(why) => Err(OrchestratorError(why)),
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
        let run_id = &run.shown().run_id;
        let outcome = match decide(run) {
            Outcome::Forgotten => match forget(world, cwd, run_id, dry_run) {
                Ok(()) => Outcome::Forgotten,
                Err(e) => Outcome::Failed(e.0),
            },
            decided => decided,
        };
        match &outcome {
            Outcome::Skipped(why) => world.say(&format!("skipped: {why}")),
            Outcome::Failed(why) => world.say(&format!("could not forget run {run_id}: {why}")),
            Outcome::Forgotten | Outcome::NotClosed => {}
        }
        outcomes.push(outcome);
    }
    let count = |of: fn(&Outcome) -> bool| outcomes.iter().filter(|o| of(o)).count();
    let forgotten = count(|o| *o == Outcome::Forgotten);
    let open = count(|o| *o == Outcome::NotClosed);
    let skipped = count(|o| matches!(o, Outcome::Skipped(_)));
    let failed = count(|o| matches!(o, Outcome::Failed(_)));
    let verb = if dry_run { "to forget" } else { "forgotten" };
    let failures = if failed > 0 { format!(", {failed} failed") } else { String::new() };
    world.say(&format!("{forgotten} {verb}, {open} not closed, {skipped} skipped{failures}"));
    failed > 0
}

/// Whether run may be forgotten (Outcome::Forgotten), as it was read: only a closed run may, as nothing drives it and
/// resume refuses it.
fn decide(run: &SavedRun) -> Outcome {
    match run.state() {
        Err(e) => Outcome::Skipped(e.0),
        Ok(state) if state.is_closed() => Outcome::Forgotten,
        Ok(_) => Outcome::NotClosed,
    }
}

/// Forget the run of the project at cwd whose id is run_id (Host::forget_run), saying so, or with dry_run only say
/// what would go. Errors: Host::forget_run's.
fn forget(world: &Connected, cwd: &str, run_id: &str, dry_run: bool) -> Result<(), OrchestratorError> {
    let dir = format!("{cwd}/{RUNS_DIR}/{run_id}");
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
