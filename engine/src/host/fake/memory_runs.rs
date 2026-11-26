//! A project's saved runs played from files held in memory, as the fakes keep them: one copy, for FakeHost and serve's
//! ServeHost alike, of what the real Host's run-store methods (host/saved_runs.rs) give; FakeHost's deletion of a run,
//! which ServeHost does not play, stays its own. Files in memory are never links, so nothing leads out of a run
//! directory, and serve's view leaves out only a run whose state.json names another run.

use std::collections::BTreeMap;

use crate::constants::{METRICS_FILE, RUNS_DIR, SUMMARY_FILE};
use crate::error::OrchestratorError;
use crate::host::{Host, RunFile, RunFileText, RunScope};
use crate::state::SavedRun;
use serde_json::Value;

/// The runs of the project at cwd that files, (path, text) pairs, hold a state.json for, as Host::run_states gives them
/// (scope Every) or Host::run_states_inside does (scope Inside), in the order of their directories' names. only, when
/// given, is the one run directory's name to read, as Host::run_state and run_state_inside read one. age gives the
/// seconds since a run's state.json was written, by its directory's name.
pub(crate) fn run_states<'a>(
    files: impl IntoIterator<Item = (&'a String, &'a String)>,
    age: impl Fn(&str) -> u64,
    cwd: &str,
    scope: RunScope,
    only: Option<&str>,
) -> Vec<SavedRun> {
    let files: BTreeMap<&str, &str> = files.into_iter().map(|(path, text)| (path.as_str(), text.as_str())).collect();
    let runs = format!("{cwd}/{RUNS_DIR}/");
    let mut states = vec![];
    for (path, text) in &files {
        let Some((dir, "state.json")) = path.strip_prefix(&runs).and_then(|rest| rest.split_once('/')) else {
            continue;
        };
        if only.is_some_and(|only| only != dir) {
            continue;
        }
        let run_dir = format!("{runs}{dir}");
        let lacks = [SUMMARY_FILE, METRICS_FILE].iter().any(|f| !files.contains_key(format!("{run_dir}/{f}").as_str()));
        let run = SavedRun::read(age(dir), &run_dir, text.as_bytes()).lacking_ending_file(lacks);
        if scope == RunScope::Every || run.shown().run_id == dir {
            states.push((dir, run));
        }
    }
    // By path, "runs/a-1/state.json" comes before "runs/a/state.json"; the real Host's glob gives a before a-1.
    states.sort_by_key(|(dir, _)| *dir);
    states.into_iter().map(|(_, run)| run).collect()
}

/// The metrics.json of each run of the project at cwd among files, (path, text) pairs, as Host::run_metrics reads
/// those of the runs still there, before it adds the archive's. Errors: one that is not JSON.
pub(crate) fn live_metrics<'a>(
    files: impl IntoIterator<Item = (&'a String, &'a String)>,
    cwd: &str,
) -> Result<Vec<Value>, OrchestratorError> {
    let runs = format!("{cwd}/{RUNS_DIR}/");
    files
        .into_iter()
        .filter(|(path, _)| {
            path.strip_prefix(&runs).and_then(|rest| rest.split_once('/')).is_some_and(|(_, name)| name == METRICS_FILE)
        })
        .map(|(_, text)| {
            serde_json::from_str::<Value>(text)
                .map_err(|e| OrchestratorError(format!("corrupt {METRICS_FILE} under {cwd}/{RUNS_DIR}: {e}")))
        })
        .collect()
}

/// The files directly in run_dir among files, (path, text) pairs, as Host::run_files lists them: state.json and
/// write's temporary files left out, oldest first by mtime, a file's mtime by its path, those of one time by name.
pub(crate) fn run_files<'a>(
    files: impl IntoIterator<Item = (&'a String, &'a String)>,
    mtime: impl Fn(&str) -> i64,
    run_dir: &str,
) -> Vec<RunFile> {
    let mut listed: Vec<RunFile> = files
        .into_iter()
        .filter_map(|(path, text)| {
            let name = path.strip_prefix(run_dir)?.strip_prefix('/')?;
            let shown = !name.contains('/') && name != "state.json" && !name.contains(".tmp.");
            shown.then(|| RunFile { path: path.clone(), mtime: mtime(path), size: text.len() as u64 })
        })
        .collect();
    listed.sort_by(|a, b| (a.mtime, &a.path).cmp(&(b.mtime, &b.path)));
    listed
}

/// Host::read_inside of the file name in run_dir, as host's read gives it. Errors: host.read's.
pub(crate) fn read_inside(
    host: &(impl Host + ?Sized),
    run_dir: &str,
    name: &str,
) -> Result<RunFileText, OrchestratorError> {
    Ok(host
        .read(&format!("{run_dir}/{name}"))?
        .map_or(RunFileText::Missing, |text| RunFileText::Text(text.into_bytes())))
}
