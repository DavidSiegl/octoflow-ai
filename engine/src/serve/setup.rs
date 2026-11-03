//! What serve gives the desktop app's setup dialog: crate::cli::doctor's checks of a project, and the marker that the
//! dialog once closed with every Required check passed, which keeps it from opening by itself again.

use std::io;

use crate::cli::doctor::{check_setup, Check, Level};
use crate::env::ConfigDir;
use crate::error::os_error;
use crate::herdr::ProcessHerdr;
use crate::host::ProcessHost;
use crate::interrupt::Interrupt;
use serde_json::Value;

use super::object;

/// How a Setup checks a project: given the saved herdr machine (empty for this one), the project's directory, which
/// may start with ~, and the Interrupt the server raises when it stops, which must stop the checks' commands; the
/// checks, in check_setup's order. [`project_checks`] is the real one.
pub type CheckProject = dyn Fn(&str, &str, &Interrupt) -> Vec<Check> + Send + Sync;

/// The setup checks a RunServer answers /setup with, and the marker /setup/checked reads and writes.
pub struct Setup {
    /// How a project is checked.
    check: Box<CheckProject>,
    /// The marker's path (ConfigDir::setup_checked_path); its parent is created when the marker is written.
    marker: String,
}

impl Setup {
    /// The real one: project_checks, and the marker in config.
    pub fn of(config: &ConfigDir) -> Setup {
        Setup::new(Box::new(project_checks), config.setup_checked_path())
    }

    /// One that checks a project by check, its marker at path.
    pub fn new(check: Box<CheckProject>, marker: String) -> Setup {
        Setup { check, marker }
    }

    /// /setup's answer for the project: {"checks": [...], "passed"}, each check as {"name", "level", "ok", "detail",
    /// "fix"}, its level as Level::as_str names it, and passed true when every Required one is ok, which is when the
    /// dialog may write the marker. A check that fails is part of the answer, never an error.
    ///
    /// passed is not doctor's exit status (crate::cli::doctor::report_checks), which a failed NeededForPr check also
    /// makes 1: a missing gh login stops a run at its pull request, which doctor is asked about, but it is no reason
    /// for the dialog to open by itself at every start.
    pub fn checks(&self, machine: &str, cwd: &str, stopping: &Interrupt) -> Value {
        let checks = (self.check)(machine, cwd, stopping);
        let passed = checks.iter().all(|c| c.ok || c.level != Level::Required);
        let checks = Value::Array(checks.iter().map(check_json).collect());
        object([("checks", checks), ("passed", Value::from(passed))])
    }

    /// Whether the marker is there. Errors: whether it is cannot be told, as when the directory cannot be searched.
    pub fn checked(&self) -> Result<bool, String> {
        std::path::Path::new(&self.marker).try_exists().map_err(|e| self.failed("look for", &e))
    }

    /// Write the marker, an empty file, creating its directory when missing; one already there stays. It does not
    /// check that the dialog's checks passed: that is the caller's to know (serve's post_checked says why). Errors:
    /// the directory or the file cannot be made.
    pub fn mark(&self) -> Result<(), String> {
        let path = std::path::Path::new(&self.marker);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| self.failed("create the directory of", &e))?;
        }
        std::fs::write(path, b"").map_err(|e| self.failed("write", &e))
    }

    /// Why doing to the marker failed with e.
    fn failed(&self, doing: &str, e: &io::Error) -> String {
        format!("cannot {doing} {}: {}", self.marker, os_error(e, None))
    }
}

/// The checks of the project in cwd on the saved herdr machine, here with machine empty, as `doctor --machine
/// --cwd` makes them, but from serve, which starts no run: reaching herdr from a herdr pane is Optional, since the
/// user starts runs from a terminal of their own, and the app may have been started from none.
pub fn project_checks(machine: &str, cwd: &str, stopping: &Interrupt) -> Vec<Check> {
    let host = ProcessHost::local(stopping);
    let herdr = ProcessHerdr::new(Some(machine), stopping);
    checks_through(&host, &herdr, cwd, crate::cli::in_herdr_pane())
}

/// project_checks through host and herdr, from a herdr pane or not as in_herdr_pane says.
pub(crate) fn checks_through(host: &ProcessHost, herdr: &ProcessHerdr, cwd: &str, in_herdr_pane: bool) -> Vec<Check> {
    check_setup(host, herdr, Some(cwd), in_herdr_pane, Level::Optional)
}

fn check_json(c: &Check) -> Value {
    object([
        ("name", Value::from(c.name.as_str())),
        ("level", Value::from(c.level.as_str())),
        ("ok", Value::from(c.ok)),
        ("detail", Value::from(c.detail.as_str())),
        ("fix", Value::from(c.fix.as_str())),
    ])
}
