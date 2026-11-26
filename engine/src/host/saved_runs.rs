//! The runs a project keeps under its runs directory, as Host reads, lists and deletes them on the machine: the sh
//! each of Host's run-store methods runs, and the parsing of what it prints. Host's methods are the interface and
//! document it; each delegates to the function here of the same job.

use std::collections::HashSet;

use serde_json::Value;

use super::{cat_unless_gone, checked, decimal_int, Host, RunFile, RunFileText, DEFAULT_TIMEOUT, MISSING_FILE_STATUS};
use crate::constants::{METRICS_FILE, RUNS_DIR, SUMMARY_FILE};
use crate::error::OrchestratorError;
use crate::state::{run_dir, SavedRun};

/// Where, relative to a project, forget_run keeps the metrics of the runs it deleted: one line of JSON per run, each
/// its METRICS_FILE's object, in the order they were forgotten.
const METRICS_ARCHIVE: &str = concat!(crate::project_dir!(), "/metrics-archive.jsonl");

/// sh, inside a loop over files $f: what ends that pass when the command before it failed. A file gone since the loop
/// found it is skipped, as a running run removes some of its files and prune a whole run (serve-run-files/03); one
/// still there fails the script.
const SKIP_F_IF_GONE: &str = "{ [ -f \"$f\" ] || continue; exit 1; }";

/// sh: sets m to the modification time of file $f, in seconds since the epoch; fails when stat does. stat -c is GNU,
/// stat -f is BSD.
const MTIME_OF_F: &str = "m=$(stat -c %Y -- \"$f\" 2>/dev/null || stat -f %m -- \"$f\")";

/// sh: as MTIME_OF_F, but sets m to the modification time and, after a space, the size in bytes of file $f.
const MTIME_AND_SIZE_OF_F: &str = "m=$(stat -c \"%Y %s\" -- \"$f\" 2>/dev/null || stat -f \"%m %z\" -- \"$f\")";

/// The status the scripts of run_files_inside and read_inside exit with when the file, or the run directory, leads
/// out of where it must stay (run_dir_resolved).
const OUTSIDE_STATUS: i32 = 5;

/// sh defining `below F D`, which succeeds when F, a file or directory, every link resolved, is below directory D, a
/// path with its links resolved, and leaves F's resolved path in r; it returns 1 when F leads out of D, and 2 when F
/// cannot be resolved, as when it, or a link on its way, was removed meanwhile, or its links loop. It uses only what
/// sh has on Linux and macOS alike: `cd -P` and `pwd -P` resolve a directory, and links are followed one at a time
/// with readlink without options (POSIX.1-2024, and GNU, BSD and BusyBox readlink alike, unlike readlink -f or
/// realpath), at most 40 deep, as Linux follows them. serve's view of a run (Host::run_files_inside, read_inside,
/// run_states_inside and run_state_inside) is what stays below the run directory, and the run directory below the runs
/// directory.
const BELOW: &str = "below() { p=$1 n=0; \
     while [ -L \"$p\" ]; do n=$((n + 1)); [ \"$n\" -le 40 ] || return 2; t=$(readlink -- \"$p\") || return 2; \
       case $t in /*) p=$t;; *) p=${p%/*}/$t;; esac; done; \
     if [ -d \"$p\" ]; then r=$(cd -P -- \"$p\" 2>/dev/null && pwd -P) || return 2; \
     else r=$(cd -P -- \"${p%/*}/\" 2>/dev/null && pwd -P) || return 2; r=${r%/}/${p##*/}; fi; \
     case $r in \"${2%/}\"/*) ;; *) return 1;; esac; }; ";

/// sh, run before a script given a run directory as $1: defines BELOW's `below` and sets d to the run directory with
/// every link resolved. It exits with MISSING_FILE_STATUS when $1 is not a directory, and with OUTSIDE_STATUS when it
/// is not below its parent, the runs directory, links resolved too.
fn run_dir_resolved() -> String {
    format!(
        "{BELOW}[ -d \"$1\" ] || exit {MISSING_FILE_STATUS}; \
         runs=$(cd -P -- \"${{1%/*}}/\" 2>/dev/null && pwd -P) || exit {MISSING_FILE_STATUS}; below \"$1\" \"$runs\"; \
         case $? in 0) d=$r;; 1) exit {OUTSIDE_STATUS};; *) exit {MISSING_FILE_STATUS};; esac; "
    )
}

/// Host::run_state, on host.
pub(super) fn state<H: Host + ?Sized>(
    host: &H,
    cwd: &str,
    run_id: &str,
) -> Result<Option<SavedRun>, OrchestratorError> {
    let script = format!("now=$(date +%s) || exit 1; {}", run_states_loop(RUNS_DIR, "\"$2\"", "true"));
    let out = run_states_output(host, &["sh", "-c", &script, "_", cwd, run_id])?;
    Ok(parsed_run_states(cwd, RUNS_DIR, &out)?.into_iter().next())
}

/// Host::run_states_inside, on host.
pub(super) fn states_inside<H: Host + ?Sized>(host: &H, cwd: &str) -> Result<Vec<SavedRun>, OrchestratorError> {
    let script = run_states_inside_script("*");
    parsed_inside_run_states(cwd, &run_states_output(host, &["sh", "-c", &script, "_", cwd])?)
}

/// Host::run_state_inside, on host.
pub(super) fn state_inside<H: Host + ?Sized>(
    host: &H,
    cwd: &str,
    run_id: &str,
) -> Result<Option<SavedRun>, OrchestratorError> {
    let script = run_states_inside_script("\"$2\"");
    let out = run_states_output(host, &["sh", "-c", &script, "_", cwd, run_id])?;
    Ok(parsed_inside_run_states(cwd, &out)?.into_iter().next())
}

/// Host::run_metrics, on host.
pub(super) fn metrics<H: Host + ?Sized>(host: &H, cwd: &str) -> Result<Vec<Value>, OrchestratorError> {
    // Each file ends at a NUL, which no JSON text holds, so one a human pretty-printed reads as well.
    let script = format!(
        "for f in \"$1\"/{RUNS_DIR}/*/{METRICS_FILE}; do [ -f \"$f\" ] || continue; cat -- \"$f\" && printf '\\000'; \
         done; true"
    );
    let out = host.check(&["sh", "-c", &script, "_", cwd], None, DEFAULT_TIMEOUT)?;
    let live = out
        .split('\0')
        .filter(|text| !text.trim().is_empty())
        .map(|text| {
            serde_json::from_str::<Value>(text)
                .map_err(|e| OrchestratorError(format!("corrupt {METRICS_FILE} under {cwd}/{RUNS_DIR}: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    with_archived_metrics(host, cwd, live)
}

/// Host::remove_run_dir, on host.
pub(super) fn remove_dir<H: Host + ?Sized>(host: &H, cwd: &str, run_id: &str) -> Result<(), OrchestratorError> {
    crate::paths::plain_name(run_id)?;
    let dir = run_dir(cwd, run_id);
    host.check(&["rm", "-rf", "--", &dir], None, DEFAULT_TIMEOUT)
        .map(drop)
        .map_err(|e| OrchestratorError(format!("could not delete {dir}: {e}")))
}

/// Host::forget_run, on host.
pub(super) fn forget<H: Host + ?Sized>(
    host: &H,
    cwd: &str,
    run_id: &str,
    scope: RunScope,
) -> Result<(), OrchestratorError> {
    crate::paths::plain_name(run_id)?;
    let dir = run_dir(cwd, run_id);
    let metrics_path = format!("{dir}/{METRICS_FILE}");
    let text = match scope {
        RunScope::Every => host.read(&metrics_path)?.map(String::into_bytes),
        RunScope::Inside => match host.read_inside(&dir, METRICS_FILE)? {
            RunFileText::Text(bytes) => Some(bytes),
            RunFileText::Missing | RunFileText::Outside => None,
        },
    };
    if let Some(text) = text {
        let metrics = serde_json::from_slice::<Value>(&text)
            .map_err(|e| OrchestratorError(format!("corrupt {metrics_path}: {e}")))?;
        let archive_path = format!("{cwd}/{METRICS_ARCHIVE}");
        let archived = archived_metrics(&host.read(&archive_path)?.unwrap_or_default(), &archive_path)?;
        if !archived.iter().any(|m| m.get("run_id") == metrics.get("run_id")) {
            // One line per run: a metrics.json a human pretty-printed is written back compact.
            host.append(&archive_path, &format!("{metrics}\n"))?;
        }
    }
    host.remove_run_dir(cwd, run_id)
}

/// Host::run_files, on host.
pub(super) fn files<H: Host + ?Sized>(host: &H, run_dir: &str) -> Result<Vec<RunFile>, OrchestratorError> {
    let out = host.check(&["sh", "-c", &run_files_loop("true"), "_", run_dir], None, DEFAULT_TIMEOUT)?;
    parsed_run_files(run_dir, &out)
}

/// Host::run_files_inside, on host.
pub(super) fn files_inside<H: Host + ?Sized>(host: &H, run_dir: &str) -> Result<Vec<RunFile>, OrchestratorError> {
    let script = format!("{}{}", run_dir_resolved(), run_files_loop("below \"$f\" \"$d\""));
    let argv = ["sh", "-c", &script, "_", run_dir];
    let proc = host.run(&argv, None, DEFAULT_TIMEOUT)?;
    if proc.status == MISSING_FILE_STATUS || proc.status == OUTSIDE_STATUS {
        return Ok(vec![]);
    }
    parsed_run_files(run_dir, &checked(host, &argv, proc)?)
}

/// Host::read_inside, on host.
pub(super) fn read_inside<H: Host + ?Sized>(
    host: &H,
    run_dir: &str,
    name: &str,
) -> Result<RunFileText, OrchestratorError> {
    // What is read is the path below resolved, so the links it walked are not followed again; one put in its
    // place after that is: a race left open, here as in run_states_inside and run_files_inside, which check a
    // path a moment before it is read, since an agent that can race the check can as well copy the file into its
    // run directory, which serve shows as any other; the check is against a link left there.
    let script = format!(
        "{}f=$1/$2; [ -f \"$f\" ] || exit {MISSING_FILE_STATUS}; below \"$f\" \"$d\"; \
         case $? in 0) ;; 1) exit {OUTSIDE_STATUS};; *) exit {MISSING_FILE_STATUS};; esac; {}",
        run_dir_resolved(),
        cat_unless_gone("$r")
    );
    let proc = host.run_verbatim(&["sh", "-c", &script, "_", run_dir, name], None, DEFAULT_TIMEOUT)?;
    match proc.status {
        0 => Ok(RunFileText::Text(proc.stdout)),
        MISSING_FILE_STATUS => Ok(RunFileText::Missing),
        OUTSIDE_STATUS => Ok(RunFileText::Outside),
        _ => Err(OrchestratorError(format!("reading {run_dir}/{name}: {}", proc.stderr.trim()))),
    }
}

/// Host::run_states for the runs directory runs_dir, relative to the project at cwd, in place of RUNS_DIR: how
/// crate::old_project_dir reads the runs a project kept under its old name. Errors: as run_states'.
pub(crate) fn run_states_in<H: Host + ?Sized>(
    host: &H,
    cwd: &str,
    runs_dir: &str,
) -> Result<Vec<SavedRun>, OrchestratorError> {
    let script = format!("now=$(date +%s) || exit 1; {}", run_states_loop(runs_dir, "*", "true"));
    parsed_run_states(cwd, runs_dir, &run_states_output(host, &["sh", "-c", &script, "_", cwd])?)
}

/// What the run_states_loop script argv prints, verbatim, so that a run whose directory name or state.json is not
/// UTF-8 is that run's problem, not the listing's. Errors: argv fails (checked).
fn run_states_output<H: Host + ?Sized>(host: &H, argv: &[&str]) -> Result<Vec<u8>, OrchestratorError> {
    checked(host, argv, host.run_verbatim(argv, None, DEFAULT_TIMEOUT)?)
}

/// Which of a project's runs a command reads: Every run with a state.json, as the CLI does, or only those Inside the
/// runs directory, as serve shows them to the desktop app (Host::run_states_inside), so that serve never acts on a run
/// the app does not show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunScope {
    Every,
    Inside,
}

impl RunScope {
    /// The runs of the project at cwd this scope reads, as Host::run_states gives them. Errors: as run_states'.
    pub fn states(self, host: &dyn Host, cwd: &str) -> Result<Vec<SavedRun>, OrchestratorError> {
        match self {
            RunScope::Every => host.run_states(cwd),
            RunScope::Inside => host.run_states_inside(cwd),
        }
    }

    /// The run of the project at cwd whose id is run_id, as Host::run_state gives it; None also for a run this scope
    /// leaves out. Errors: as run_states'.
    pub fn state(self, host: &dyn Host, cwd: &str, run_id: &str) -> Result<Option<SavedRun>, OrchestratorError> {
        match self {
            RunScope::Every => host.run_state(cwd, run_id),
            RunScope::Inside => host.run_state_inside(cwd, run_id),
        }
    }
}

/// sh printing what run_states_loop prints for the runs of the project $1 whose directory names match the sh word
/// runs, but only of those Host::run_states_inside keeps.
fn run_states_inside_script(runs: &str) -> String {
    // Without a runs directory there are no runs; f becomes the resolved path, which is what is stat'ed and read. A
    // run directory counts only under its own name, not under a link's: an unreadable state.json names no run, so its
    // run is the one its directory is named for (SavedRun::read), and a link to it would list it twice.
    format!(
        "{BELOW}now=$(date +%s) || exit 1; runs=$(cd -P -- \"$1/{RUNS_DIR}\" 2>/dev/null && pwd -P) || exit 0; {}",
        run_states_loop(
            RUNS_DIR,
            runs,
            "below \"${f%/*}\" \"$runs\" && [ \"${r##*/}\" = \"$d\" ] && below \"$f\" \"$r\" && f=$r"
        )
    )
}

/// sh printing, for each state.json $f of a run in the directory runs_dir of the project $1, relative to it, whose
/// directory name matches the sh word runs, such as `*`, and for which the sh condition keep succeeds, its age in
/// seconds by $now, a blank, 1 when the directory $f is in lacks SUMMARY_FILE or METRICS_FILE and 0 when it has both
/// (SavedRun::lacks_ending_file), a blank, the run directory's name as $f names it, a slash, which no name holds, and
/// its text, ended by a NUL, which no JSON text holds: what parsed_run_states reads. A state.json a human
/// pretty-printed reads as well as the engine's one line.
fn run_states_loop(runs_dir: &str, runs: &str, keep: &str) -> String {
    // d is taken before keep, which may point f at the file a link resolves to.
    format!(
        "for f in \"$1\"/{runs_dir}/{runs}/state.json; do [ -f \"$f\" ] || continue; d=${{f%/*}}; d=${{d##*/}}; \
         {keep} || continue; {MTIME_OF_F} || {SKIP_F_IF_GONE}; s=$(cat -- \"$f\") || {SKIP_F_IF_GONE}; \
         l=0; [ -e \"${{f%/*}}/{SUMMARY_FILE}\" ] && [ -e \"${{f%/*}}/{METRICS_FILE}\" ] || l=1; \
         printf \"%s %s %s/%s\\000\" \"$((now - m))\" \"$l\" \"$d\" \"$s\"; done; true"
    )
}

/// The runs run_states_loop printed as out, under the project at cwd's runs_dir, as run_states gives them; a run
/// directory's name that is not UTF-8 has each such byte replaced, as it only names the run. Errors: an entry not of
/// run_states_loop's shape, its age not a number or no run directory's name in it.
pub(super) fn parsed_run_states(cwd: &str, runs_dir: &str, out: &[u8]) -> Result<Vec<SavedRun>, OrchestratorError> {
    Ok(parsed_named_run_states(cwd, runs_dir, out)?.into_iter().map(|(_, run)| run).collect())
}

/// As parsed_run_states, under the project at cwd's runs directory, but only the runs whose state.json names the run
/// its directory is named for, as Host::run_states_inside keeps them. An unreadable run whose state.json names no run
/// is named for its directory (SavedRun::read), so it is kept.
fn parsed_inside_run_states(cwd: &str, out: &[u8]) -> Result<Vec<SavedRun>, OrchestratorError> {
    let runs = parsed_named_run_states(cwd, RUNS_DIR, out)?;
    Ok(runs.into_iter().filter(|(dir, run)| run.shown().run_id == *dir).map(|(_, run)| run).collect())
}

/// The runs parsed_run_states reads from out, each with the name of the directory it was read from.
fn parsed_named_run_states(
    cwd: &str,
    runs_dir: &str,
    out: &[u8],
) -> Result<Vec<(String, SavedRun)>, OrchestratorError> {
    let mut runs = vec![];
    for entry in out.split(|&b| b == 0) {
        if entry.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let corrupt = |e: String| OrchestratorError(format!("corrupt run state under {cwd}/{runs_dir}: {e}"));
        let (age, rest) = split_at_byte(entry, b' ').unwrap_or((entry, b""));
        let age = String::from_utf8_lossy(age);
        let not_int = || corrupt(format!("an age that is not a whole number: {age:?}"));
        let age: i64 = decimal_int(&age).ok_or_else(not_int)?;
        let (lacks, named) = split_at_byte(rest, b' ').unwrap_or((rest, b""));
        let lacks = match lacks {
            b"0" => false,
            b"1" => true,
            _ => return Err(corrupt(format!("no 0 or 1 after the age in {:?}", String::from_utf8_lossy(rest)))),
        };
        let no_dir = || corrupt(format!("no run directory in {:?}", String::from_utf8_lossy(named)));
        let (dir, text) = split_at_byte(named, b'/').ok_or_else(no_dir)?;
        let dir = String::from_utf8_lossy(dir).into_owned();
        let run = SavedRun::read(age.max(0).unsigned_abs(), &format!("{cwd}/{runs_dir}/{dir}"), text)
            .lacking_ending_file(lacks);
        runs.push((dir, run));
    }
    Ok(runs)
}

/// bytes before and after the first at in them; None when at is not there.
fn split_at_byte(bytes: &[u8], at: u8) -> Option<(&[u8], &[u8])> {
    let i = bytes.iter().position(|&b| b == at)?;
    Some((&bytes[..i], &bytes[i + 1..]))
}

/// sh printing, for each file $f in the directory $1 that run_files lists and for which the sh condition keep
/// succeeds, "<mtime> <size> <path>" ended by a NUL, which no path holds: what parsed_run_files reads.
fn run_files_loop(keep: &str) -> String {
    format!(
        "for f in \"$1\"/*; do [ -f \"$f\" ] || continue; \
         case \"${{f##*/}}\" in state.json|*.tmp.*) continue;; esac; \
         {keep} || continue; {MTIME_AND_SIZE_OF_F} || {SKIP_F_IF_GONE}; printf \"%s %s\\0\" \"$m\" \"$f\"; done; true"
    )
}

/// live, the metrics of the runs of the project at cwd that still have a directory, with those of the runs forget_run
/// deleted, as host's run_metrics gives them: ordered by run id, the order of the runs' directories, so that forgetting
/// a run does not move its line. A run archived whose directory is still there, as after a forget_run that failed to
/// delete it, is given once, from its directory, and so is a run the archive holds twice. Errors: the archive cannot
/// be read, or holds a line that is not JSON.
pub(super) fn with_archived_metrics(
    host: &(impl Host + ?Sized),
    cwd: &str,
    live: Vec<Value>,
) -> Result<Vec<Value>, OrchestratorError> {
    let path = format!("{cwd}/{METRICS_ARCHIVE}");
    let run_id = |m: &Value| m.get("run_id").and_then(Value::as_str).unwrap_or_default().to_string();
    let live_ids: HashSet<String> = live.iter().map(run_id).collect();
    let mut all = archived_metrics(&host.read(&path)?.unwrap_or_default(), &path)?;
    // Two forgets of one run at once can both find its line missing and both append it (Host::forget_run).
    let mut seen = HashSet::new();
    all.retain(|m| !live_ids.contains(&run_id(m)) && seen.insert(run_id(m)));
    all.extend(live);
    all.sort_by_key(run_id);
    Ok(all)
}

/// The records of text, a METRICS_ARCHIVE read from path, one per line. Errors: a line that is not JSON.
fn archived_metrics(text: &str, path: &str) -> Result<Vec<Value>, OrchestratorError> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).map_err(|e| OrchestratorError(format!("corrupt {path}: {e}"))))
        .collect()
}

/// The files run_files_loop printed as out, listing run_dir, oldest first, those of one time by name. Errors: an
/// entry whose time or size is not one.
fn parsed_run_files(run_dir: &str, out: &str) -> Result<Vec<RunFile>, OrchestratorError> {
    let mut files = vec![];
    for entry in out.split('\0').filter(|e| !e.is_empty()) {
        let (mtime, rest) = entry.split_once(' ').unwrap_or((entry, ""));
        let mtime = decimal_int(mtime)
            .ok_or_else(|| OrchestratorError(format!("listing {run_dir}: stat printed {mtime:?}, not a time")))?;
        let (size, path) = rest.split_once(' ').unwrap_or((rest, ""));
        let size = size
            .parse::<u64>()
            .map_err(|_| OrchestratorError(format!("listing {run_dir}: stat printed {size:?}, not a size")))?;
        files.push((mtime, path.to_string(), size));
    }
    files.sort();
    Ok(files.into_iter().map(|(mtime, path, size)| RunFile { path, mtime, size }).collect())
}
