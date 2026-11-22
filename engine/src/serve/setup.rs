//! What serve gives the desktop app's setup dialog: crate::cli::doctor's checks of a project for the app's runs, whose
//! agents run over ACP without herdr; the installs of the ACP adapters (crate::acp_adapter); and the marker that the
//! dialog once closed with every Required check passed, which, as Setup::checked says, keeps it from opening by itself
//! again.

use std::io;
use std::time::Duration;

use crate::acp_adapter::Installs;
use crate::cli::doctor::{check_acp_setup, Check, Level};
use crate::constants::DEFAULT_HARNESS;
use crate::env::ConfigDir;
use crate::error::os_error;
use crate::harness::Adapter;
use crate::host::ProcessHost;
use crate::interrupt::Interrupt;
use serde_json::Value;

use super::object;

/// How a Setup checks a project: given the project's directory, which may start with ~, and the Interrupt the server
/// raises when it stops, which must stop the checks' commands; the checks, in the order check_acp_setup gives them.
/// [`project_checks`] is the real one.
pub type CheckProject = dyn Fn(&str, &Interrupt) -> Vec<Check> + Send + Sync;

/// The setup checks a RunServer answers /setup with, the installs of the ACP adapters that POST /setup/adapter makes,
/// and the marker /setup/checked reads and writes.
pub struct Setup {
    /// How a project is checked.
    check: Box<CheckProject>,
    /// The installs of each ACP adapter, in Adapter::ALL's order.
    installs: Vec<Installs>,
    /// The marker's path (ConfigDir::setup_checked_path); its parent is created when the marker is written.
    marker: String,
}

impl Setup {
    /// The real one: project_checks, each adapter's installs in the engine's environment (crate::env::child_env), and
    /// the marker in config.
    pub fn of(config: &ConfigDir) -> Setup {
        let env = crate::env::child_env();
        let installs = Adapter::ALL.iter().map(|a| Installs::in_environment(*a, env.clone())).collect();
        Setup::new(Box::new(project_checks), installs, config.setup_checked_path())
    }

    /// One that checks a project by check, installs the adapters of installs, and has its marker at path.
    pub fn new(check: Box<CheckProject>, installs: Vec<Installs>, marker: String) -> Setup {
        Setup { check, installs, marker }
    }

    /// /setup's answer for the project: {"checks": [...], "passed", "adapters": [...]}, each check as {"name",
    /// "level", "ok", "detail", "fix"}, its level as Level::as_str names it, and passed true when every Required one
    /// is ok, which is when the dialog may write the marker. adapters are the ACP adapters the dialog offers to
    /// install, each as {"agent", "installed", "megabytes"}: its harness's name (Adapter::harness), whether it is
    /// installed for the current pins, and the disk an install takes (Adapter::megabytes). A check that fails is part
    /// of the answer, never an error.
    ///
    /// passed is not doctor's exit status (crate::cli::doctor::report_checks), which a failed NeededForPr check also
    /// makes 1: a missing gh login stops a run at its pull request, which doctor is asked about, but it is no reason
    /// for the dialog to open by itself at every start.
    pub fn checks(&self, cwd: &str, stopping: &Interrupt) -> Value {
        let checks = (self.check)(cwd, stopping);
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

    /// Install adapter for the current pins, unless it is installed, as Installs::install does, then remove its old
    /// installs (Installs::remove_old); stopping is the Interrupt the server raises when it stops, which stops it.
    /// Returns why old installs could not be removed, None when they were. Errors: Installs::install's, and this Setup
    /// has no installs of adapter (Setup::new).
    pub fn install_adapter(&self, adapter: Adapter, stopping: &Interrupt) -> Result<Option<String>, String> {
        let Some(installs) = self.installs.iter().find(|i| i.adapter() == adapter) else {
            return Err(format!("this server installs no {} adapter", adapter.harness()));
        };
        installs.install(stopping).map_err(|e| e.0)?;
        Ok(installs.remove_old().err().map(|e| e.0))
    }

    /// Whether the dialog need not open by itself: the marker is there and the adapter of DEFAULT_HARNESS, if it has
    /// one, is installed for the current pins, whose absence fails a Required check, as after an upgrade raised a pin;
    /// another adapter's check is Optional. Errors: whether the marker is there cannot
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

/// How long an ACP agent has to answer `initialize` in project_checks, and then to report its login when it declares
/// it does (crate::acp::probe): long enough for a CLI's first start, which may update itself, short enough that the
/// dialog does not hang on one that never answers or never reports.
const PROBE_SECONDS: u64 = 30;

/// The checks of the project in cwd for the app's runs (check_acp_setup), each harness's agent asked `initialize` by
/// crate::acp::probe in the engine's environment, working in the home directory, the adapters as installed there.
pub fn project_checks(cwd: &str, stopping: &Interrupt) -> Vec<Check> {
    let host = ProcessHost::local(stopping);
    let env = crate::env::child_env();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let answers = |h| crate::acp::probe(h, &env, &home, Duration::from_secs(PROBE_SECONDS), stopping);
    check_acp_setup(&host, &env, Some(cwd), &answers)
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
