//! Deleting what is left of closed runs: the `forget` command, for one run or every closed run of a project.

use super::prune::awaits_prune;
use super::{refuse_app_run, Connected};
use crate::constants::{EXIT_ERROR, HEARTBEAT_SECONDS};
use crate::error::OrchestratorError;
use crate::host::{Host, RunScope};
use crate::interrupt::Interrupt;
use crate::json::object;
use crate::state::{find_run, run_dir, SavedRun};
use serde_json::Value;

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
    /// A run the project did not have at its turn, or that the Forgetter's scope left out: of a Sweep::Listed, or of
    /// a Sweep::All one another forget forgot since the sweep listed it.
    Gone,
    /// Its forget failed, and why; part of its directory may be gone. For a sweep but a dry run, also its state.json
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
/// deleted does not stop a sweep. A raised Interrupt starts no further run. A forget that deletes is refused while
/// another of the project runs (FORGET_LOCK).
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
    let _lock = if dry_run { None } else { ForgetLock::take(&forgetter, cwd)? };
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
/// A sweep but a dry run holds the project's forget lock throughout, and is refused while another forget of the
/// project holds it (FORGET_LOCK); it reads each run again at its turn, so a run another forget forgot meanwhile (one
/// that took this sweep's lock over while it was silent, and released it since) is Outcome::Gone, not forgotten twice.
///
/// Returns each run looked at, in that order; none after an Interrupt. Errors: for Sweep::DryRun and Sweep::All, the
/// project's runs cannot be read at the start (a run that cannot be read at its turn is Outcome::Failed);
/// the forget lock cannot be taken (ForgetLock::take), or another forget holds it mid-sweep, having taken it over,
/// which stops the sweep (ForgetLock::renew).
pub fn forget_all(world: &Forgetter, cwd: &str, sweep: Sweep) -> Result<Vec<RunForgotten>, OrchestratorError> {
    let dry_run = matches!(sweep, Sweep::DryRun);
    let lock = if dry_run { None } else { ForgetLock::take(world, cwd)? };
    // Every run of the project, read once here; none for a Sweep::Listed. Only a dry run acts on these states.
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
        if let Some(lock) = &lock {
            lock.renew()?;
        }
        let outcome = match every.get(i) {
            Some(run) if dry_run => sweep_one(world, cwd, run, dry_run),
            _ => match world.scope.state(world.host, cwd, run_id) {
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
/// for prune, skipped and failed; the last two only when some are, and a run that was gone counts as skipped.
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
/// `octoflow forget` beside one, on any machine reaching the project) never run at once: while one holds it, another
/// is refused. Its text names the holder: JSON with its machine ("host"), its process ("pid") and a "nonce" of its
/// own. Its mtime is the holder's heartbeat, renewed before each run it forgets (ForgetLock::renew).
const FORGET_LOCK: &str = concat!(crate::project_dir!(), "/forget.lock");

/// How long, in seconds by the project's host's clock, a forget lock's heartbeat must have been silent for the lock to
/// be taken for one its holder left when it died. The holder renews it before each run, so this bounds the forgetting
/// of one run (an append to the metrics archive and the delete of its directory), not a whole sweep.
const LOCK_SILENCE: i64 = 600;

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
    /// Take the forget lock of the project at cwd for world, taking over one its holder left when it died
    /// (left_by_the_dead). None, and no lock, when the project has no directory of its own (crate::project_dir!), and
    /// so no run to forget. Errors: another holds it, who is named; something other than a file is at its path; the
    /// lock cannot be read, made, moved or removed; the system's random bytes cannot be read.
    fn take(world: &'w Forgetter<'a>, cwd: &str) -> Result<Option<ForgetLock<'w, 'a>>, OrchestratorError> {
        if !world.host.exists(&format!("{cwd}/{}", crate::project_dir!()))? {
            return Ok(None);
        }
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
        // Each pass ends in the lock taken or refused, or finds it released or taken over since, and tries again.
        loop {
            if world.host.create_new(&path, &text)? {
                return Ok(Some(ForgetLock { world, path, text }));
            }
            let Some(held) = world.host.read(&path)? else {
                if world.host.exists(&path)? {
                    return Err(OrchestratorError(format!(
                        "{path} is not a file: no forget of this project can run until it is removed"
                    )));
                }
                continue;
            };
            match left_by_the_dead(world, &path, &held) {
                Ok(true) => take_over(world, &path, &held, &nonce)?,
                Ok(false) => {
                    let who = holder_of(&held).map_or("a holder its text does not name".to_string(), |(host, pid)| {
                        format!("process {pid} on {host}")
                    });
                    return Err(OrchestratorError(format!(
                        "another forget of this project is running: {who} holds {path}; forget again once it ends"
                    )));
                }
                Err(_) if !world.host.exists(&path)? => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Renew the lock's heartbeat, before forgetting the next run. A lock gone from its path is made again: another's
    /// take_over may have moved it aside for an instant, to put it back, and finding it there then refuses that
    /// other; or another forget took it for dead, swept and released it, and this sweep goes on, finding the runs it
    /// forgot gone (forget_all). A text naming no holder is one another's create_new is still writing, and is read
    /// once more. Errors: another holder's lock is at its path, as when another forget took this one for dead and
    /// still holds it, and the sweep must stop; the lock cannot be read, made or touched.
    fn renew(&self) -> Result<(), OrchestratorError> {
        let world = self.world;
        let mut held = match world.host.read(&self.path)? {
            None if world.host.create_new(&self.path, &self.text)? => return Ok(()),
            None => world.host.read(&self.path)?,
            held => held,
        };
        if held.as_deref().is_some_and(|text| holder_of(text).is_none()) {
            held = world.host.read(&self.path)?;
        }
        if held.as_deref() != Some(self.text.as_str()) {
            return Err(OrchestratorError(format!(
                "the forget lock {} was taken over by another forget, which took this one for dead: stopped",
                self.path
            )));
        }
        world.host.touch(&self.path)
    }
}

impl Drop for ForgetLock<'_, '_> {
    /// Remove the lock if it is still this one's (one taken over is its taker's), even past the Ctrl-C. A failure is
    /// told, as the lock then refuses the project's forgets until its heartbeat has been silent LOCK_SILENCE.
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

/// The machine and process a forget lock's text held names, if it names both.
fn holder_of(held: &str) -> Option<(String, i64)> {
    let holder = serde_json::from_str::<Value>(held).ok()?;
    Some((holder.get("host")?.as_str()?.to_string(), holder.get("pid")?.as_i64()?))
}

/// Whether the forget lock at path, whose text is held, was left by a process that died: one naming a process of this
/// machine that is gone, its heartbeat late by HEARTBEAT_SECONDS too, or any whose heartbeat has been silent
/// LOCK_SILENCE. A lock naming no holder is judged by its heartbeat alone. The late heartbeat is asked of a dead pid
/// as RunState::health asks it, for the same reason: another machine of this one's name may hold the lock. Errors: the
/// lock's age cannot be read, as when it is gone since.
fn left_by_the_dead(world: &Forgetter, path: &str, held: &str) -> Result<bool, OrchestratorError> {
    let age = world.host.now()? - world.host.mtime(path)?;
    let gone = |(host, pid): (String, i64)| host == world.local_host && !(world.pid_alive)(pid);
    Ok(age >= LOCK_SILENCE || age > HEARTBEAT_SECONDS as i64 && holder_of(held).is_some_and(gone))
}

/// Remove the forget lock at path, read as held and found left by the dead, unless it changed since. It is moved aside
/// first, to a name nonce makes this process's own, and removed there only if it is still held: so of two processes
/// that found it so at once, the second moves the first's new lock, sees that, and puts it back. Should the first have
/// released it in between, the lock put back has nobody to renew it and is taken over once its heartbeat is late, as
/// left_by_the_dead says; should the first renew it in between, it makes it again, and the put-back fails, as it does
/// should a third have made one, whose holder the first then finds at its next ForgetLock::renew and stops. Errors: the
/// lock cannot be moved (other than for being gone), read, put back or removed.
fn take_over(world: &Forgetter, path: &str, held: &str, nonce: &str) -> Result<(), OrchestratorError> {
    let aside = format!("{path}.{nonce}");
    if let Err(e) = world.host.rename(path, &aside) {
        // Gone already: another took it over, or its holder released it after all.
        return if world.host.exists(path)? { Err(e) } else { Ok(()) };
    }
    if let Some(moved) = world.host.read(&aside)?.filter(|moved| moved != held) {
        // False when a third made the lock in between: see above.
        let _put_back = world.host.create_new(path, &moved)?;
    }
    world.host.remove(&aside)
}

#[cfg(test)]
mod tests;
