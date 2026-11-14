//! Whether a run is driven by a live orchestrator, as its state.json tells (RunState::health): what `list` shows of
//! each run, what keeps two runs from sharing one checkout (Workflow's check_alone), and what keeps the commands that
//! act on a run off one that is driven.

use crate::constants::{HEARTBEAT_SECONDS, STALE_SECONDS};
use crate::json::ValueExt;
use crate::state::{RunState, SavedRun};
use serde_json::Value;
use crate::summary::format_age;

/// A run an orchestrator still drives, as far as its heartbeat tells.
pub const RUNNING: &str = "running";
/// An unfinished run nothing drives any more: its orchestrator died, or its machine was lost.
pub const STALE: &str = "stale";

impl RunState {
    /// (RUNNING or STALE, why) for a run an orchestrator drives, or drove until it died: one neither done nor failed,
    /// or one saved as done whose orchestrator has not released it yet, which may still be ending it
    /// (Workflow::end_done_run). None for any other run, which nothing drives: what list shows, and what every command
    /// that acts on a run asks before touching it.
    ///
    /// age is the seconds since the state was written, which a live owner does every HEARTBEAT_SECONDS. The owner's
    /// pid can be checked only on its own host, local_host, by pid_alive. Host names can collide, so a dead pid counts
    /// only once a beat is late too; a live pid proves nothing, since pids are reused, so the age still applies to it.
    pub fn health(&self, age: u64, local_host: &str, pid_alive: &dyn Fn(i64) -> bool) -> Option<(&'static str, String)> {
        let owner = self.owner.as_ref().filter(|o| !o.is_empty());
        let driven = if self.done() { owner.is_some() } else { self.error.as_deref().is_none_or(str::is_empty) };
        if !driven {
            return None;
        }
        let field = |key: &str| owner.and_then(|o| o.get(key)).map_or("unknown".to_string(), Value::text);
        let pid = owner.and_then(|o| o.get("pid")).and_then(Value::as_i64);
        let local = owner.and_then(|o| o.get("host")).and_then(Value::as_str) == Some(local_host);
        if local && age > HEARTBEAT_SECONDS && pid.is_some_and(|pid| !pid_alive(pid)) {
            return Some((STALE, format!("orchestrator pid {} exited", field("pid"))));
        }
        let beat = format_age(age as i64);
        if age > STALE_SECONDS {
            return Some((STALE, format!("no heartbeat for {beat}")));
        }
        if owner.is_some() {
            return Some((RUNNING, format!("pid {} on {}, beat {beat} ago", field("pid"), field("host"))));
        }
        Some((RUNNING, format!("beat {beat} ago")))
    }
}

impl SavedRun {
    /// The health of the run as far as its state reads (SavedRun::shown), its age being the state.json's: as
    /// RunState::health.
    pub fn health(&self, local_host: &str, pid_alive: &dyn Fn(i64) -> bool) -> Option<(&'static str, String)> {
        self.shown().health(self.age(), local_host, pid_alive)
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

/// This machine's name, as socket.gethostname gives it: what an owner record's host holds, and what RunState::health
/// compares it with. "localhost" when the system cannot say.
pub fn local_host() -> String {
    nix::unistd::gethostname().map_or_else(|_| "localhost".to_string(), |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests;
