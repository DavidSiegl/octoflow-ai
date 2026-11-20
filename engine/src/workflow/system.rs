//! The Clocks and the Human of a real run on this machine, which the CLI gives a Workflow: the system's clocks, a
//! notification through the local herdr or, for a run over ACP, the desktop's own command (DesktopNotify), and log
//! lines on stderr.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nix::time::{clock_gettime, ClockId};

use super::{Clocks, Human};
use crate::error::OrchestratorError;
use crate::human::Notify;
use crate::interrupt::Interrupt;

/// This machine's zone, as jiff's TimeZone::try_system finds it from TZ, else /etc/localtime; UTC when neither
/// names one it can read (where TimeZone::system would give Etc/Unknown).
pub fn machine_zone() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC)
}

/// The system's clocks, whose sleep the Ctrl-C cuts short.
#[derive(Clone, Debug)]
pub struct SystemClocks {
    /// What cuts a sleep short.
    interrupt: Interrupt,
    /// When the clocks were made, as an Instant and on CLOCK_MONOTONIC in seconds: monotonic reads the system's
    /// monotonic clock from Instant, which never fails.
    made: (Instant, f64),
}

impl SystemClocks {
    /// The system's clocks, a sleep ending early once interrupt is pending (Interrupt::sleep).
    ///
    /// Errors: the system's monotonic clock cannot be read.
    pub fn new(interrupt: &Interrupt) -> Result<SystemClocks, OrchestratorError> {
        let now = clock_gettime(ClockId::CLOCK_MONOTONIC)
            .map_err(|e| OrchestratorError(format!("cannot read the monotonic clock: {e}")))?;
        let seconds = now.tv_sec() as f64 + now.tv_nsec() as f64 / 1e9;
        Ok(SystemClocks { interrupt: interrupt.clone(), made: (Instant::now(), seconds) })
    }
}

impl Clocks for SystemClocks {
    /// A time below zero or not finite sleeps not at all.
    fn sleep(&self, seconds: f64) {
        self.interrupt.sleep(Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO));
    }

    /// Seconds since a point the system chooses, such as its boot (CLOCK_MONOTONIC).
    fn monotonic(&self) -> f64 {
        self.made.1 + self.made.0.elapsed().as_secs_f64()
    }

    /// machine_zone.
    fn zone(&self) -> jiff::tz::TimeZone {
        machine_zone()
    }

    fn wall(&self) -> f64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(since) => since.as_secs_f64(),
            Err(before) => -before.duration().as_secs_f64(),
        }
    }
}

/// The human at this machine: notified where they are watching, never on the run's --machine, and told everything
/// else on stderr, each line indented by two spaces.
#[derive(Debug)]
pub struct LocalHuman {
    /// Where the human is watching on this machine.
    notifier: Box<dyn Notify>,
}

impl LocalHuman {
    /// The human, shown notifications by notifier.
    pub fn new(notifier: Box<dyn Notify>) -> LocalHuman {
        LocalHuman { notifier }
    }
}

impl Human for LocalHuman {
    /// A notification that cannot be shown costs only itself, and is logged.
    fn notify(&self, title: &str, body: &str) {
        if let Err(e) = self.notifier.notify(title, body) {
            self.log(&format!("notification failed: {e}"));
        }
    }

    /// A line stderr does not take is lost: there is nowhere else to tell the human, and a run is not stopped for it.
    fn log(&self, line: &str) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "  {line}");
    }
}

/// Notifications shown by the desktop's own command, whether the app is open or not, as a run over ACP shows them:
/// `notify-send` on Linux and other Unix desktops, `osascript` on macOS (desktop_command).
#[derive(Debug)]
pub struct DesktopNotify {
    /// The environment the command runs in, its PATH being where it is found.
    env: Vec<(OsString, OsString)>,
    /// True once a notification found the command missing: that one's error said so, and every later one is dropped
    /// without an error, so that the log says it once.
    missing_told: AtomicBool,
}

impl DesktopNotify {
    /// Notifications by this machine's command, found on this process's PATH.
    pub fn new() -> DesktopNotify {
        DesktopNotify::in_environment(std::env::vars_os())
    }

    /// As new, the command run in the environment vars alone.
    fn in_environment(vars: impl Iterator<Item = (OsString, OsString)>) -> DesktopNotify {
        DesktopNotify { env: vars.collect(), missing_told: AtomicBool::new(false) }
    }
}

impl Default for DesktopNotify {
    fn default() -> DesktopNotify {
        DesktopNotify::new()
    }
}

impl Notify for DesktopNotify {
    /// Errors: the command is missing, the first time only; it cannot be run, or fails, saying what it printed.
    fn notify(&self, title: &str, body: &str) -> Result<(), OrchestratorError> {
        let argv = desktop_command(std::env::consts::OS, title, body);
        let (program, args) = argv.split_first().expect("a desktop command has a program");
        let output = Command::new(program)
            .args(args)
            .env_clear()
            .envs(self.env.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match output {
            Err(e) if e.kind() == ErrorKind::NotFound => match self.missing_told.swap(true, Ordering::Relaxed) {
                true => Ok(()),
                false => Err(OrchestratorError(format!(
                    "{program} is not installed, so the run shows no desktop notification, this one or any later"
                ))),
            },
            Err(e) => Err(OrchestratorError(format!("cannot run {program}: {e}"))),
            Ok(out) if !out.status.success() => Err(OrchestratorError(format!(
                "{program} failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))),
            Ok(_) => Ok(()),
        }
    }
}

/// The command that shows a desktop notification with title and body on the system os names
/// (std::env::consts::OS): osascript on `macos`, the title and body passed as arguments rather than spliced into its
/// script; notify-send on any other.
fn desktop_command(os: &str, title: &str, body: &str) -> Vec<String> {
    let mut argv: Vec<String> = match os {
        "macos" => [
            "osascript",
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
        ]
        .map(String::from)
        .to_vec(),
        _ => ["notify-send", "--app-name", "octoflow", "--"].map(String::from).to_vec(),
    };
    argv.extend([title.to_string(), body.to_string()]);
    argv
}

#[cfg(test)]
mod tests;
