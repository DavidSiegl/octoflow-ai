//! Deleting what is left of closed runs: the `forget` command, for one run or every closed run of a project.

use super::prune::awaits_prune;
use super::{refuse_app_run, Connected};
use crate::constants::EXIT_ERROR;
use crate::error::OrchestratorError;
use crate::host::{Host, RunScope};
use crate::interrupt::Interrupt;
use crate::json::{object, ValueExt};
use crate::state::{find_run, run_dir, SavedRun};
use serde_json::Value;
use std::time::Duration;

/// What forgetting works with: the CLI's Connected, or serve's project (crate::serve::runs::ProjectRuns::forget_all).
pub struct Forgetter<'a> {
    /// The machine the project is on, where the run directories and the metrics archive are.
    pub host: &'a dyn Host,
    /// Once raised, forget_all starts no further run.
    pub interrupt: &'a Interrupt,
    /// Where forget prints, which takes text as Connected::stdout does, a line with its line end.
    pub stdout: &'a dyn Fn(&str),
    /// Which of the project's runs forget reads, and so can forget.
    pub scope: RunScope,
    /// True when forget may forget the desktop app's runs (refuse_app_run): serve's, and the CLI's given --acp.
    pub acp: bool,
    /// Whether a process of an id runs on this machine: crate::runs::pid_alive, but in tests. Tells a forget lock
    /// this machine's process left when it died (ForgetLock::take).
    pub pid_alive: &'a dyn Fn(i64) -> bool,
    /// This machine's name, as crate::runs::local_host gives it, which a forget lock names its holder's machine by.
    pub local_host: &'a str,
}

/// Which runs forget_all looks at, and whether it deletes.
#[derive(Clone, Copy, Debug)]
pub enum Sweep<'p> {
    /// Every run of the project, deleting none: say what forgetting each would do.
    DryRun,
    /// Every run of the project.
    All,
    /// Only the runs with these ids, in this order, as an earlier DryRun gave them, each read only at its turn: a run
    /// that is not there then (forgotten since, say), or that the Forgetter's scope leaves out, is Outcome::Gone. Each is still forgotten only if decide lets it.
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
    /// A Sweep::Listed run the project did not have at its turn, or that the Forgetter's scope left out.
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
    /// It is the desktop app's, and the Forgetter's acp is false; why, as refuse_app_run says it.
    AppRun(String),
}

impl<'a> Forgetter<'a> {
    /// The Forgetter of the CLI's world: its host, interrupt and standard output.
    pub fn of(world: &Connected<'a>) -> Forgetter<'a> {
        Forgetter {
            host: world.with.host,
            interrupt: world.with.interrupt,
            stdout: world.stdout,
            scope: RunScope::Every,
            acp: world.acp,
            pid_alive: world.with.pid_alive,
            local_host: world.local_host,
        }
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
/// deleted does not stop a sweep. A raised Interrupt starts no further run. A forget that deletes first waits for any
/// other of the project to end (FORGET_LOCK).
///
/// Returns the exit status: EXIT_ERROR when a run of a sweep failed, else 0. Errors: the project's runs cannot be read;
/// for run_ref, no run or several match it, or it is left as decide says, or forgetting it fails; the forget lock
/// cannot be taken (ForgetLock::take).
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
    let _lock = if dry_run { None } else { Some(ForgetLock::take(&forgetter, cwd)?) };
    let runs = world.with.host.run_states(cwd)?;
    let run = find_run(&runs, run_ref, cwd)?;
    let run_id = &run.shown().run_id;
    match decide(run, forgetter.acp) {
        Ok(()) => forget(&forgetter, cwd, run_id, dry_run).map(|()| 0),
        Err(Left::NotClosed) => {
            Err(OrchestratorError(format!("run {run_id} is not closed: close it before forgetting it")))
        }
        Err(Left::AwaitsPrune) => {
            Err(OrchestratorError(format!("run {run_id} is closed, but {AWAITS_PRUNE}: forget it after that")))
        }
        Err(Left::Unreadable(why) | Left::AppRun(why)) => Err(OrchestratorError(why)),
    }
}

/// Forget the closed runs of the project at cwd that sweep names, as forget_command says, printing a line per run
/// forgotten (or to be), skipped or failed, then a count of each outcome. The runs are every run of the project
/// world's scope reads (RunScope), in run id order, or a Sweep::Listed's, in its order. A raised Interrupt starts no further run.
/// A sweep but a dry run holds the project's forget lock throughout, first waiting for any other forget of the project
/// to end (FORGET_LOCK).
///
/// Returns each run looked at, in that order; none after an Interrupt. Errors: for Sweep::DryRun and Sweep::All, the
/// project's runs cannot be read at the start (a Sweep::Listed run that cannot be read at its turn is Outcome::Failed);
/// the forget lock cannot be taken (ForgetLock::take).
pub fn forget_all(world: &Forgetter, cwd: &str, sweep: Sweep) -> Result<Vec<RunForgotten>, OrchestratorError> {
    let dry_run = matches!(sweep, Sweep::DryRun);
    let _lock = if dry_run { None } else { Some(ForgetLock::take(world, cwd)?) };
    // Every run of the project, read once here; none for a Sweep::Listed, whose runs are read at their turns.
    let mut every = match sweep {
        Sweep::DryRun | Sweep::All => world.scope.states(world.host, cwd)?,
        Sweep::Listed(_) => vec![],
    };
    every.sort_by(|a, b| a.shown().run_id.cmp(&b.shown().run_id));
    let ids: Vec<&str> = match sweep {
        Sweep::DryRun | Sweep::All => every.iter().map(|run| run.shown().run_id.as_str()).collect(),
        Sweep::Listed(ids) => ids.iter().map(String::as_str).collect(),
    };
    let mut outcomes = vec![];
    for (i, run_id) in ids.into_iter().enumerate() {
        if world.interrupt.is_raised() {
            break;
        }
        let outcome = match every.get(i) {
            Some(run) => sweep_one(world, cwd, run, dry_run),
            None => match world.scope.state(world.host, cwd, run_id) {
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
    let outcome = match decide(run, world.acp) {
        Ok(()) => match forget(world, cwd, run_id, dry_run) {
            Ok(()) => Outcome::Forgotten,
            Err(e) => Outcome::Failed(e.0),
        },
        Err(left) => Outcome::Left(left),
    };
    match &outcome {
        Outcome::Left(Left::Unreadable(why) | Left::AppRun(why)) => world.say(&format!("skipped: {why}")),
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
    let skipped = count(|o| matches!(o, Outcome::Left(Left::Unreadable(_) | Left::AppRun(_)) | Outcome::Gone));
    let failed = count(|o| matches!(o, Outcome::Failed(_)));
    let verb = if dry_run { "to forget" } else { "forgotten" };
    let pruning = if pruning > 0 { format!(", {pruning} waiting for prune") } else { String::new() };
    let failures = if failed > 0 { format!(", {failed} failed") } else { String::new() };
    world.say(&format!("{forgotten} {verb}, {open} not closed{pruning}, {skipped} skipped{failures}"));
}

/// Whether run may be forgotten, as it was read, or why not: only a closed run may, as nothing drives it and resume
/// refuses it, and of those only one whose branch prune is done with; and a desktop app's run only with acp
/// (refuse_app_run).
fn decide(run: &SavedRun, acp: bool) -> Result<(), Left> {
    let state = run.state().map_err(|e| Left::Unreadable(e.0))?;
    refuse_app_run(state, "forget", acp).map_err(|e| Left::AppRun(e.0))?;
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
    world.host.forget_run(cwd, run_id, world.scope)?;
    world.say(&format!("forgot run {run_id}: deleted {dir}"));
    Ok(())
}

/// The project's forget lock, relative to the project. Every forget that deletes holds it (ForgetLock), from reading
/// the runs until the last is forgotten, so that forgets of one project by several processes (two apps' serve,
/// `octoflow forget` beside one, on any machine reaching the project) run one after the other: a run they all read is
/// forgotten by the first, found gone by the rest, and its metrics archived once. Its text names the holder: JSON with
/// its machine ("host"), its process ("pid") and a "nonce" of its own.
const FORGET_LOCK: &str = concat!(crate::project_dir!(), "/forget.lock");

/// How old, in seconds by its mtime on the project's host, a forget lock another machine's process holds must be to be
/// taken for one it left when it died: that process cannot be asked, as this machine's can. A sweep forgets a run in a
/// few commands, so a live one holds the lock far shorter.
const FOREIGN_LOCK_STALE: i64 = 600;

/// How long a forget waits before it tries again for a forget lock another holds.
const LOCK_RETRY: Duration = Duration::from_secs(1);

/// The project's forget lock (FORGET_LOCK), held: released when dropped.
struct ForgetLock<'w, 'a> {
    /// Who holds it.
    world: &'w Forgetter<'a>,
    /// Its absolute path.
    path: String,
    /// Its text while this holds it.
    text: String,
}

impl<'w, 'a> ForgetLock<'w, 'a> {
    /// Take the forget lock of the project at cwd for world, waiting while another live process holds it, which world
    /// is told of once. A lock left by a process that died is taken over: one of this machine whose pid is gone, or
    /// one of another machine FOREIGN_LOCK_STALE old. Errors: the interrupt is raised before it is taken; the lock
    /// cannot be read, made, moved or removed; the system's random bytes cannot be read.
    fn take(world: &'w Forgetter<'a>, cwd: &str) -> Result<ForgetLock<'w, 'a>, OrchestratorError> {
        let path = format!("{cwd}/{FORGET_LOCK}");
        let bytes: [u8; 8] = crate::env::random_bytes()
            .map_err(|e| OrchestratorError(format!("cannot read /dev/urandom for a forget lock: {e}")))?;
        let nonce: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let holder = object([
            ("host", Value::from(world.local_host)),
            ("pid", Value::from(std::process::id())),
            ("nonce", Value::from(nonce.as_str())),
        ]);
        let text = format!("{}\n", Value::Object(holder));
        let mut told = false;
        loop {
            if world.interrupt.is_raised() {
                return Err(OrchestratorError(format!("interrupted while waiting for the forget lock {path}")));
            }
            if world.host.create_new(&path, &text)? {
                return Ok(ForgetLock { world, path, text });
            }
            // Released since, when it is gone: try again at once.
            let Some(held) = world.host.read(&path)? else { continue };
            let dead = match left_by_the_dead(world, &path, &held) {
                Ok(dead) => dead,
                Err(_) if !world.host.exists(&path)? => continue,
                Err(e) => return Err(e),
            };
            if dead {
                take_over(world, &path, &held, &nonce)?;
                continue;
            }
            if !told {
                world.say(&format!("waiting for another forget of this project to end: it holds {path}"));
                told = true;
            }
            world.interrupt.sleep(LOCK_RETRY);
        }
    }
}

impl Drop for ForgetLock<'_, '_> {
    /// Remove the lock if it is still this one's (one taken over as left by the dead is its taker's), even past the
    /// Ctrl-C; a failure is told, as the lock then holds back this machine's forgets for as long as this process lives.
    fn drop(&mut self) {
        let world = self.world;
        let released = world.interrupt.finally(|| match world.host.read(&self.path)? {
            Some(text) if text == self.text => world.host.remove(&self.path),
            _ => Ok(()),
        });
        if let Err(e) = released {
            world.say(&format!("could not remove the forget lock {}: {e}; delete it if no forget runs", self.path));
        }
    }
}

/// Whether the forget lock at path, whose text is held, was left by a process that died (ForgetLock::take says which).
/// A text naming no holder, as a hand-made lock may, counts as another machine's. Errors: the lock's age cannot be read,
/// as when it is gone since.
fn left_by_the_dead(world: &Forgetter, path: &str, held: &str) -> Result<bool, OrchestratorError> {
    let holder = serde_json::from_str::<Value>(held).ok();
    let field = |key: &str| holder.as_ref().and_then(|h| h.get(key)).cloned();
    if let (Some(host), Some(pid)) = (field("host"), field("pid").and_then(|p| p.as_i64())) {
        if host.text() == world.local_host {
            return Ok(!(world.pid_alive)(pid));
        }
    }
    Ok(world.host.now()? - world.host.mtime(path)? >= FOREIGN_LOCK_STALE)
}

/// Remove the forget lock at path, read as held and found left by the dead, unless it changed since. It is moved
/// aside first, to a name nonce makes this process's own, and only removed there if it is still held: so of two
/// processes that found it so at once, the second moves the first's new lock, sees that, and puts it back. A third
/// that took the lock in that instant would hold it with the first; that needs three forgets of one project and a
/// dead process's lock at once, and is accepted. Errors: the lock cannot be moved (other than for being gone), read,
/// put back or removed.
fn take_over(world: &Forgetter, path: &str, held: &str, nonce: &str) -> Result<(), OrchestratorError> {
    let aside = format!("{path}.{nonce}");
    if let Err(e) = world.host.rename(path, &aside) {
        // Gone already: another took it over, or its holder released it after all.
        return if world.host.exists(path)? { Err(e) } else { Ok(()) };
    }
    if let Some(moved) = world.host.read(&aside)?.filter(|moved| moved != held) {
        world.host.create_new(path, &moved)?;
    }
    world.host.remove(&aside)
}

#[cfg(test)]
mod tests;
