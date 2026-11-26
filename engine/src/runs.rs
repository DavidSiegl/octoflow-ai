//! Whether a run is driven by a live orchestrator, as its state.json tells (RunState::health, judged from ThisMachine):
//! what `list` shows of each run, what keeps two runs from sharing one checkout (Workflow's check_alone), and what
//! keeps the commands that act on a run off one that is driven.

use crate::constants::{HEARTBEAT_SECONDS, STALE_SECONDS};
use crate::json::ValueExt;
use crate::state::{RunState, SavedRun};
use crate::summary::format_age;
use serde_json::Value;

/// A run an orchestrator still drives, as far as its heartbeat tells: Health::Running's name.
pub const RUNNING: &str = "running";
/// An unfinished run nothing drives any more: Health::Stale's name.
pub const STALE: &str = "stale";

/// The health of a run that an orchestrator drives, or drove until it died (RunState::health), each with why, such as
/// "pid 4242 on laptop, beat 40s ago" or "no heartbeat for 12m". A run nothing drives has no health.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    /// Its orchestrator still drives it, as far as its heartbeat tells.
    Running(String),
    /// Its orchestrator died, or its machine was lost, before it was released.
    Stale(String),
}

impl Health {
    /// RUNNING or STALE: the word `list` and the run record show.
    pub fn name(&self) -> &'static str {
        match self {
            Health::Running(_) => RUNNING,
            Health::Stale(_) => STALE,
        }
    }

    /// Why the run has this health, in words for a human.
    pub fn why(&self) -> &str {
        match self {
            Health::Running(why) | Health::Stale(why) => why,
        }
    }
}

/// The machine a run's health is judged from: its name, which an owner record of an orchestrator on it holds
/// (local_host), and whether a process of an id exists on it (pid_alive, which a test replaces).
#[derive(Clone, Copy)]
pub struct ThisMachine<'a> {
    /// The name an owner record of a process on this machine holds as its host.
    name: &'a str,
    /// Whether a process of an id exists on this machine.
    pid_alive: &'a dyn Fn(i64) -> bool,
}

impl<'a> ThisMachine<'a> {
    /// The machine named name, whose processes pid_alive tells.
    pub fn new(name: &'a str, pid_alive: &'a dyn Fn(i64) -> bool) -> ThisMachine<'a> {
        ThisMachine { name, pid_alive }
    }

    /// This machine's name, as an owner record of a process on it names its host.
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// Whether the process pid of the machine named host is known to have exited: only one of this machine's can be,
    /// as pid_alive tells. Host names can collide and pids are reused, so false proves nothing, and true only that no
    /// process of that id runs here.
    pub fn has_exited(&self, host: &str, pid: i64) -> bool {
        host == self.name && !(self.pid_alive)(pid)
    }
}

impl RunState {
    /// The health of a run an orchestrator drives, or drove until it died: one neither done nor failed,
    /// or one saved as done whose orchestrator has not released it yet, which may still be ending it
    /// (Workflow::end_done_run). None for any other run, which nothing drives: what list shows, and what every command
    /// that acts on a run asks before touching it.
    ///
    /// age is the seconds since the state was written, which a live owner does every HEARTBEAT_SECONDS. The owner's
    /// pid can be checked only on its own host, by machine (ThisMachine::has_exited). Host names can collide, so an
    /// exited pid counts only once a beat is late too; a live pid proves nothing, since pids are reused, so the age
    /// still applies to it.
    pub fn health(&self, age: u64, machine: ThisMachine) -> Option<Health> {
        let owner = self.owner.as_ref().filter(|o| !o.is_empty());
        let driven = if self.done() { owner.is_some() } else { self.error.as_deref().is_none_or(str::is_empty) };
        if !driven {
            return None;
        }
        let field = |key: &str| owner.and_then(|o| o.get(key)).map_or("unknown".to_string(), Value::text);
        let pid = owner.and_then(|o| o.get("pid")).and_then(Value::as_i64);
        let host = owner.and_then(|o| o.get("host")).and_then(Value::as_str);
        let exited = host.zip(pid).is_some_and(|(host, pid)| machine.has_exited(host, pid));
        if age > HEARTBEAT_SECONDS && exited {
            return Some(Health::Stale(format!("orchestrator pid {} exited", field("pid"))));
        }
        let beat = format_age(age as i64);
        if age > STALE_SECONDS {
            return Some(Health::Stale(format!("no heartbeat for {beat}")));
        }
        if owner.is_some() {
            return Some(Health::Running(format!("pid {} on {}, beat {beat} ago", field("pid"), field("host"))));
        }
        Some(Health::Running(format!("beat {beat} ago")))
    }
}

impl SavedRun {
    /// The health of the run as far as its state reads (SavedRun::shown), its age being the state.json's: as
    /// RunState::health. None for a state.json that is not JSON naming a run, which nothing drives, whatever fields
    /// of it read; one of a format this engine does not read may be driven by another release, so its fields count.
    pub fn health(&self, machine: ThisMachine) -> Option<Health> {
        if !self.names_run {
            return None;
        }
        self.shown().health(self.age(), machine)
    }
}

/// Whether a process of that id exists on this machine, under this user or another, as kill(pid, 0) tells; false for
/// an id beyond a pid's range. An id of 0 or below is not a process's: it names a process group (0 this
/// one's, -1 every process but this one and init, -N group N), which counts as alive when kill finds a process in it.
pub fn pid_alive(pid: i64) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    let Ok(pid) = i32::try_from(pid) else { return false };
    match kill(Pid::from_raw(pid), None) {
        Ok(()) => true,
        // It exists, under another user.
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// This machine's name, as socket.gethostname gives it: what an owner record's host holds, and ThisMachine's name.
/// "localhost" when the system cannot say.
pub fn local_host() -> String {
    nix::unistd::gethostname().map_or_else(|_| "localhost".to_string(), |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests;
