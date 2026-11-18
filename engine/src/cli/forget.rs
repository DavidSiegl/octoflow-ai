//! Deleting what is left of closed runs: the `forget` command, for one run or every closed run of a project.

use super::prune::awaits_prune;
use super::Connected;
use crate::constants::EXIT_ERROR;
use crate::error::OrchestratorError;
use crate::host::Host;
use crate::interrupt::Interrupt;
use crate::state::{find_run, run_dir, SavedRun};

/// What forgetting works with: the CLI's Connected, or serve's project (crate::serve::runs::ProjectRuns::forget_all).
pub struct Forgetter<'a> {
    /// The machine the project is on, where the run directories and the metrics archive are.
    pub host: &'a dyn Host,
    /// Once raised, forget_all starts no further run.
    pub interrupt: &'a Interrupt,
    /// Where forget prints, which takes text as Connected::stdout does, a line with its line end.
    pub stdout: &'a dyn Fn(&str),
}

/// Which runs forget_all looks at, and whether it deletes.
#[derive(Clone, Copy, Debug)]
pub enum Sweep<'p> {
    /// Every run of the project, deleting none: say what forgetting each would do.
    DryRun,
    /// Every run of the project.
    All,
    /// Only the runs with these ids, in this order, as an earlier DryRun gave them, each read only at its turn: a run
    /// that is not there then (forgotten since, say) is Outcome::Gone. Each is still forgotten only if decide lets it.
    Listed(&'p [String]),
}

/// One run forget_all looked at, and what became of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunForgotten {
    /// The run's id.
    pub run_id: String,
    /// What forget_all did with the run, or with a dry run would do.
    pub outcome: Outcome,
}

/// What forget_all did with one run, or with a dry run would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Its directory deleted, its metrics kept (Host::forget_run); with a dry run, to be.
    Forgotten,
    /// Left as it is, and why.
    Left(Left),
    /// A Sweep::Listed run the project did not have at its turn.
    Gone,
    /// Its forget failed, and why; part of its directory may be gone. For a Sweep::Listed run, also its state.json
    /// that could not be read at its turn, or is not a JSON object naming its run.
    Failed(String),
}

/// Why a closed run waits for prune, as forget and serve's POST /forget give it.
pub const AWAITS_PRUNE: &str = "prune deletes its branch and worktree once its pull request is merged or closed";

/// Why a run is not forgotten, as it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Left {
    /// It is not closed.
    NotClosed,
    /// It is closed, but prune has yet to delete its branch, origin's and its worktree, and finds them only through
    /// its state (crate::cli::prune::awaits_prune).
    AwaitsPrune,
    /// Its state cannot be read (SavedRun::problem), so that neither is known; why.
    Unreadable(String),
}

impl<'a> Forgetter<'a> {
    /// The Forgetter of the CLI's world: its host, interrupt and standard output.
    pub fn of(world: &Connected<'a>) -> Forgetter<'a> {
        Forgetter { host: world.with.host, interrupt: world.with.interrupt, stdout: world.stdout }
    }

    /// Print line on stdout; line has no line end, which this adds.
    fn say(&self, line: &str) {
        (self.stdout)(&format!("{line}\n"));
    }
}

/// The `forget` command in the project at cwd, run from world: delete the directory of the closed run run_ref names,
/// or without one of every closed run, keeping their metrics (Host::forget_run), and print what became of each; with
/// dry_run, print what would go and change nothing. A run that is not closed, or whose branch prune has yet to delete,
/// is refused alone and left by a sweep, as is a run whose state cannot be read; a run whose directory cannot be
/// deleted does not stop a sweep. A raised Interrupt starts no further run.
///
/// Returns the exit status: EXIT_ERROR when a run of a sweep failed, else 0. Errors: the project's runs cannot be read;
/// for run_ref, no run or several match it, or it is left as decide says, or forgetting it fails.
pub fn forget_command(
    world: &Connected,
    cwd: &str,
    run_ref: Option<&str>,
    dry_run: bool,
) -> Result<i32, OrchestratorError> {
    let forgetter = Forgetter::of(world);
    let Some(run_ref) = run_ref else {
        let runs = forget_all(&forgetter, cwd, if dry_run { Sweep::DryRun } else { Sweep::All })?;
        return Ok(if runs.iter().any(|run| matches!(run.outcome, Outcome::Failed(_))) { EXIT_ERROR } else { 0 });
    };
    let runs = world.with.host.run_states(cwd)?;
    let run = find_run(&runs, run_ref, cwd)?;
    let run_id = &run.shown().run_id;
    match decide(run) {
        Ok(()) => forget(&forgetter, cwd, run_id, dry_run).map(|()| 0),
        Err(Left::NotClosed) => {
            Err(OrchestratorError(format!("run {run_id} is not closed: close it before forgetting it")))
        }
        Err(Left::AwaitsPrune) => {
            Err(OrchestratorError(format!("run {run_id} is closed, but {AWAITS_PRUNE}: forget it after that")))
        }
        Err(Left::Unreadable(why)) => Err(OrchestratorError(why)),
    }
}

/// Forget the closed runs of the project at cwd that sweep names, as forget_command says, printing a line per run
/// forgotten (or to be), skipped or failed, then a count of each outcome. The runs are every run of the project, in
/// run id order, or a Sweep::Listed's, in its order. A raised Interrupt starts no further run.
///
/// Returns each run looked at, in that order; none after an Interrupt. Errors: for Sweep::DryRun and Sweep::All, the
/// project's runs cannot be read at the start (a Sweep::Listed run that cannot be read at its turn is Outcome::Failed).
pub fn forget_all(world: &Forgetter, cwd: &str, sweep: Sweep) -> Result<Vec<RunForgotten>, OrchestratorError> {
    // Every run of the project, read once here; none for a Sweep::Listed, whose runs are read at their turns.
    let mut every = match sweep {
        Sweep::DryRun | Sweep::All => world.host.run_states(cwd)?,
        Sweep::Listed(_) => vec![],
    };
    every.sort_by(|a, b| a.shown().run_id.cmp(&b.shown().run_id));
    let ids: Vec<&str> = match sweep {
        Sweep::DryRun | Sweep::All => every.iter().map(|run| run.shown().run_id.as_str()).collect(),
        Sweep::Listed(ids) => ids.iter().map(String::as_str).collect(),
    };
    let dry_run = matches!(sweep, Sweep::DryRun);
    let mut outcomes = vec![];
    for (i, run_id) in ids.into_iter().enumerate() {
        if world.interrupt.is_raised() {
            break;
        }
        let outcome = match every.get(i) {
            Some(run) => sweep_one(world, cwd, run, dry_run),
            None => match world.host.run_state(cwd, run_id) {
                Ok(Some(run)) => sweep_one(world, cwd, &run, dry_run),
                Ok(None) => Outcome::Gone,
                Err(e) => Outcome::Failed(e.0),
            },
        };
        outcomes.push(RunForgotten { run_id: run_id.to_string(), outcome });
    }
    say_counts(world, &outcomes, dry_run);
    Ok(outcomes)
}

/// forget_all's step for one run: forget run if decide lets it, saying why when it is skipped as unreadable or its
/// forget fails, and silent when it is left for being open or awaiting prune, which only the count reports.
fn sweep_one(world: &Forgetter, cwd: &str, run: &SavedRun, dry_run: bool) -> Outcome {
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
        Outcome::Forgotten | Outcome::Gone | Outcome::Left(Left::NotClosed | Left::AwaitsPrune) => {}
    }
    outcome
}

/// forget_all's closing line: how many runs of outcomes were forgotten (with dry_run, are to be), not closed, waiting
/// for prune, skipped and failed; the last two only when some are, and a listed run that was gone counts as skipped.
fn say_counts(world: &Forgetter, outcomes: &[RunForgotten], dry_run: bool) {
    let count = |of: fn(&Outcome) -> bool| outcomes.iter().filter(|run| of(&run.outcome)).count();
    let forgotten = count(|o| *o == Outcome::Forgotten);
    let open = count(|o| *o == Outcome::Left(Left::NotClosed));
    let pruning = count(|o| *o == Outcome::Left(Left::AwaitsPrune));
    let skipped = count(|o| matches!(o, Outcome::Left(Left::Unreadable(_)) | Outcome::Gone));
    let failed = count(|o| matches!(o, Outcome::Failed(_)));
    let verb = if dry_run { "to forget" } else { "forgotten" };
    let pruning = if pruning > 0 { format!(", {pruning} waiting for prune") } else { String::new() };
    let failures = if failed > 0 { format!(", {failed} failed") } else { String::new() };
    world.say(&format!("{forgotten} {verb}, {open} not closed{pruning}, {skipped} skipped{failures}"));
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
fn forget(world: &Forgetter, cwd: &str, run_id: &str, dry_run: bool) -> Result<(), OrchestratorError> {
    let dir = run_dir(cwd, run_id);
    if dry_run {
        world.say(&format!("would forget run {run_id}: delete {dir}"));
        return Ok(());
    }
    world.host.forget_run(cwd, run_id)?;
    world.say(&format!("forgot run {run_id}: deleted {dir}"));
    Ok(())
}

#[cfg(test)]
mod tests;
