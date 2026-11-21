//! What serve gives the desktop app's setup dialog: crate::cli::doctor's checks of a project, for the backend the app's
//! runs use (herdr's, or over ACP without herdr); for runs over ACP, the installs of the ACP adapters
//! (crate::acp_adapter); and the marker that the dialog once closed with every Required check passed, which, as
//! Setup::checked says, keeps it from opening by itself again.

use std::io;
use std::time::Duration;

use crate::acp_adapter::{Adapter, Installs};
use crate::cli::doctor::{check_acp_setup, check_setup, Check, Level};
use crate::constants::DEFAULT_HARNESS;
use crate::env::ConfigDir;
use crate::error::os_error;
use crate::herdr::ProcessHerdr;
use crate::host::ProcessHost;
use crate::interrupt::Interrupt;
use crate::state::Backend;
use serde_json::Value;

use super::object;

/// How a Setup checks a project: given the saved herdr machine (empty for this one), the project's directory, which
/// may start with ~, and the Interrupt the server raises when it stops, which must stop the checks' commands; the
/// checks, in the order check_setup or check_acp_setup gives them. [`project_checks`] and [`acp_project_checks`] are
/// the real ones, Setup::of picking one by backend.
pub type CheckProject = dyn Fn(&str, &str, &Interrupt) -> Vec<Check> + Send + Sync;

/// The setup checks a RunServer answers /setup with, the installs of the ACP adapters that POST /setup/adapter makes,
/// and the marker /setup/checked reads and writes.
pub struct Setup {
    /// How a project is checked.
    check: Box<CheckProject>,
    /// The installs of each ACP adapter, in Adapter::ALL's order, for app runs over ACP; empty for app runs in herdr,
    /// whose agents are the harnesses' CLIs.
    installs: Vec<Installs>,
    /// The marker's path (ConfigDir::setup_checked_path, or acp_setup_checked_path); its parent is created when the
    /// marker is written.
    marker: String,
}

impl Setup {
    /// The real one, for app runs whose agents run in backend: project_checks, or acp_project_checks and each
    /// adapter's installs in the engine's environment (crate::env::child_env); and the marker in config.
    pub fn of(config: &ConfigDir, backend: Backend) -> Setup {
        match backend {
            Backend::Herdr => Setup::new(Box::new(project_checks), vec![], config.setup_checked_path()),
            Backend::Acp => {
                let env = crate::env::child_env();
                let installs = Adapter::ALL.iter().map(|a| Installs::in_environment(*a, env.clone())).collect();
                Setup::new(Box::new(acp_project_checks), installs, config.acp_setup_checked_path())
            }
        }
    }

    /// One that checks a project by check, installs the adapters of installs (none for none to install), and has its
    /// marker at path.
    pub fn new(check: Box<CheckProject>, installs: Vec<Installs>, marker: String) -> Setup {
        Setup { check, installs, marker }
    }

    /// /setup's answer for the project: {"checks": [...], "passed", "adapters": [...]}, each check as {"name",
    /// "level", "ok", "detail", "fix"}, its level as Level::as_str names it, and passed true when every Required one
    /// is ok, which is when the dialog may write the marker. adapters are the ACP adapters the dialog offers to
    /// install, each as {"agent", "installed", "megabytes"}: its harness's name (Adapter::harness), whether it is
    /// installed for the current pins, and the disk an install takes (Adapter::megabytes); empty for app runs in
    /// herdr. A check that fails is part of the answer, never an error.
    ///
    /// passed is not doctor's exit status (crate::cli::doctor::report_checks), which a failed NeededForPr check also
    /// makes 1: a missing gh login stops a run at its pull request, which doctor is asked about, but it is no reason
    /// for the dialog to open by itself at every start.
    pub fn checks(&self, machine: &str, cwd: &str, stopping: &Interrupt) -> Value {
        let checks = (self.check)(machine, cwd, stopping);
        let passed = checks.iter().all(|c| c.ok || c.level != Level::Required);
        let checks = Value::Array(checks.iter().map(check_json).collect());
        let adapters = self.installs.iter().map(|i| {
            object([
                ("agent", Value::from(i.adapter().harness())),
                ("installed", Value::from(i.installed().is_some())),
                ("megabytes", Value::from(i.adapter().megabytes())),
            ])
        });
        object([("checks", checks), ("passed", Value::from(passed)), ("adapters", Value::Array(adapters.collect()))])
    }

    /// Install adapter for the current pins, unless it is installed, as Installs::install does; stopping is the
    /// Interrupt the server raises when it stops, which stops it. Returns false when there is none to install, for
    /// app runs in herdr. Errors: Installs::install's.
    pub fn install_adapter(&self, adapter: Adapter, stopping: &Interrupt) -> Result<bool, String> {
        match self.installs.iter().find(|i| i.adapter() == adapter) {
            Some(installs) => installs.install(stopping).map(|_| true).map_err(|e| e.0),
            None => Ok(false),
        }
    }

    /// Whether the dialog need not open by itself: the marker is there and, for app runs over ACP, the adapter of
    /// DEFAULT_HARNESS, if it has one, is installed for the current pins, whose absence fails a Required check, as
    /// after an upgrade raised a pin; another adapter's check is Optional. Errors: whether the marker is there cannot
    /// be told, as when the directory cannot be searched.
    pub fn checked(&self) -> Result<bool, String> {
        let marked = std::path::Path::new(&self.marker).try_exists().map_err(|e| self.failed("look for", &e))?;
        let required = self.installs.iter().filter(|i| i.adapter().harness() == DEFAULT_HARNESS);
        Ok(marked && required.into_iter().all(|i| i.installed().is_some()))
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

/// How long an ACP agent has to answer `initialize` in acp_project_checks: long enough for a CLI's first start, which
/// may update itself, short enough that the dialog does not hang on one that never answers.
const PROBE_SECONDS: u64 = 30;

/// The checks of the project in cwd for app runs over ACP (check_acp_setup), each harness's agent asked `initialize`
/// by crate::acp::probe in the engine's environment, working in the home directory, the adapters as installed there.
/// machine is not used: such runs run on this machine.
pub fn acp_project_checks(_machine: &str, cwd: &str, stopping: &Interrupt) -> Vec<Check> {
    let host = ProcessHost::local(stopping);
    let env = crate::env::child_env();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let answers = |h| crate::acp::probe(h, &env, &home, Duration::from_secs(PROBE_SECONDS), stopping);
    check_acp_setup(&host, &env, Some(cwd), &answers)
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
