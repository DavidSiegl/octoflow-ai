//! `--quality-gate local`: the engine runs octoflow-gate itself on the project's host, in place of a Jenkins job
//! (docs/quality-gate.md, "The local gate").
//!
//! Each judgement is a job of its own on the host, started detached from the engine (start) and asked after until it
//! ends (Judging::verdict), so the engine keeps its heartbeat while the project's tests run, and the job outlives an
//! SSH connection that drops. The job judges a checkout of the snapshot, as a Jenkins build does, so the Builder's
//! untracked files are judged and what the gate and the tests write never reaches the run's worktree.
//!
//! The release is the one the run's base pins in ci/gate.pin, else BUILT_IN_PIN. Its tarball is downloaded once per
//! pin into the host's cache, `${XDG_CACHE_HOME:-~/.cache}/octoflow/gate/`, and checked against the pin's sha256
//! each time it is unpacked, fresh, into the job's directory. The gate judges by the base's gate.toml and
//! pyproject.toml, never the change's, so a change cannot loosen how it is itself judged, as a Jenkins quality build
//! judges by main's. Every step runs as a command on the host, so a project on another machine is judged there, as
//! its git steps are.

use crate::error::OrchestratorError;
use crate::host::{Host, DEFAULT_TIMEOUT};
use crate::quality::last_lines;

/// The release a base without ci/gate.pin is judged by, in ci/gate.pin's form: the octoflow-gate release this engine
/// was tested against, whose quality.json schema crate::quality::read_report reads. Raise it with the repository's own
/// ci/gate.pin.
pub const BUILT_IN_PIN: &str = "0.5.0 4ebf96264d5ae20cc2dd0516122cd50ebf11dac9f1944c14d899aece882773f8";

/// Where a pin's release is downloaded from: the GitHub release `v<version>` of octoflow-gate, with {v} the version.
const RELEASE_URL: &str = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v{v}/octoflow-gate-{v}.tar.gz";

/// The pin file, relative to the top level of the judged repository.
const PIN_PATH: &str = "ci/gate.pin";

/// The oldest python3 the gate runs on: gate.py imports tomllib.
const PYTHON_MIN: (u32, u32) = (3, 11);

/// What the preflight needs besides python3: the job's fetch on PATH (curl, tar, and sha256sum or macOS's shasum),
/// a ps that names a process's command line as STOP asks it to, which BusyBox's does not, and a cache it can write.
const PREFLIGHT: &str = r#"for c in curl tar; do command -v "$c" >/dev/null || { echo "$c is not on PATH" >&2; exit 1; }; done
ps -ww -o stat=,args= -p $$ >/dev/null 2>&1 || { echo "ps does not take -ww -o stat=,args= -p, which stopping the gate needs" >&2; exit 1; }
command -v sha256sum >/dev/null || command -v shasum >/dev/null || { echo "neither sha256sum nor shasum is on PATH" >&2; exit 1; }
d=${XDG_CACHE_HOME:-$HOME/.cache}/octoflow/gate
mkdir -p -- "$d" && [ -w "$d" ] || { echo "cannot write the cache $d" >&2; exit 1; }"#;

/// Starts argv[2:] detached, in a session and so a process group of its own, its output appended to the file
/// argv[1], and prints its pid, which is the group's id. python3 rather than sh, whose job control needs a terminal,
/// or setsid, which macOS lacks.
const LAUNCH: &str = "import subprocess, sys
with open(sys.argv[1], 'ab') as log:
    print(subprocess.Popen(sys.argv[2:], stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True).pid)";

/// Makes the round's directory $1 if need be, and in it job-<k>, a start's job directory, with k one more than the
/// last one's, and prints k. mkdir makes each name once, so two starts never share one, even at once.
const NEXT_JOB: &str = r#"mkdir -p -- "$1" || exit 1
k=1
while ! mkdir -- "$1/job-$k" 2>/dev/null; do
    [ -d "$1/job-$k" ] || { echo "cannot make $1/job-$k" >&2; exit 1; }
    k=$((k + 1))
done
echo "$k""#;

/// The job, run by sh as $0 job_name(<its job directory>), with its job directory, the release's version and sha256,
/// its URL and the base as $1..$5. The job directory holds judge/ and tree/, the checkout, when it starts. It ends by
/// writing `exit`, renamed into place: the gate's exit status, or `setup` when it failed before the gate ran, having
/// said why on standard error, which goes to job.log. The gate's own standard error goes to gate.err.
const JOB: &str = r#"set -u
out=$1 v=$2 sum=$3 url=$4 base=$5
ended() { printf '%s\n' "$1" > "$out/exit.tmp" && mv -f -- "$out/exit.tmp" "$out/exit"; }
setup_failed() { echo "$1" >&2; ended setup; exit 1; }
sha_of() { if command -v sha256sum >/dev/null; then sha256sum < "$1"; else shasum -a 256 < "$1"; fi | cut -d' ' -f1; }
cache=${XDG_CACHE_HOME:-$HOME/.cache}/octoflow/gate
tarball=$cache/octoflow-gate-$v-$sum.tar.gz
# A tarball cached for this pin that no longer matches it, being cut short or edited, is downloaded again.
if [ ! -f "$tarball" ] || [ "$(sha_of "$tarball")" != "$sum" ]; then
    curl -fsSL --connect-timeout 30 -o "$out/download.tar.gz" -- "$url" || setup_failed "cannot download $url"
    got=$(sha_of "$out/download.tar.gz")
    [ "$got" = "$sum" ] || setup_failed "$url has sha256 $got, not the pinned $sum"
    # Copied beside its final name first, so the name never holds part of it.
    mkdir -p -- "$cache" && cp -- "$out/download.tar.gz" "$tarball.$$" && mv -f -- "$tarball.$$" "$tarball" ||
        setup_failed "cannot keep the release in $cache"
fi
mkdir -- "$out/release" && tar -xzf "$tarball" -C "$out/release" || setup_failed "cannot unpack $tarball"
[ "$(sha_of "$tarball")" = "$sum" ] || setup_failed "$tarball changed while it was unpacked"
set -- python3 "$out/release/gate.py" --base "$base" --config "$out/judge/pyproject.toml" --out "$out/quality.md"
# The release's Rust helper is built for this platform alone; elsewhere the gate reports a Rust change as not read.
[ "$(uname -sm)" = "Linux x86_64" ] && set -- "$@" --rust-syntax "$out/release/gate-rust-linux-x86_64"
cd -- "$out/tree" || setup_failed "cannot enter $out/tree"
"$@" 2> "$out/gate.err"
ended "$?""#;

/// Stops the process group $1, the job named $2 (job_name), as a Ctrl-C would, SIGINT first so the gate kills its
/// tests' groups itself, then SIGKILL to what is left after 5 s. $3 is how the caller knows the group (Known): a
/// leader whose command line does not hold $2 as an argument of its own is never the job's, and a group whose leader
/// has died is stopped only when it is `tracked`. A leader that died but was never reaped, as under a PID 1 that does
/// not reap orphans (docker run without --init), is still listed, in state Z and named `[sh] <defunct>`, so it counts
/// as died. A group is named `-"$g"` after the signal, without `--`, which dash's kill refuses.
const STOP: &str = r#"g=$1 name=$2 known=$3
leader=$(ps -ww -o stat=,args= -p "$g" 2>/dev/null)
case $leader in
'' | Z*) [ "$known" = tracked ] || exit 0 ;;
*) case " $leader " in *" $name "*) ;; *) exit 0 ;; esac ;;
esac
kill -INT -"$g" 2>/dev/null || exit 0
for _ in 1 2 3 4 5; do kill -0 -"$g" 2>/dev/null || exit 0; sleep 1; done
kill -KILL -"$g" 2>/dev/null; exit 0"#;

/// What a run of the local gate came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalVerdict {
    /// The gate judged the change.
    Judged {
        /// The version of the release that judged it.
        release: String,
        /// Its quality.md.
        quality: String,
        /// Its quality.json.
        json: String,
    },
    /// The gate could not judge the change and wrote nothing, as for a gate.toml it refuses: the end of what it said
    /// on standard error.
    CouldNotJudge(String),
}

/// A judgement by the local gate, running on the host as a job of its own (start).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judging {
    /// The repository whose worktree the snapshot's checkout is.
    cwd: String,
    /// The job's directory, which holds its files and no other start's (start).
    dir: String,
    /// The version of the release that judges.
    release: String,
    /// The id of the job's process group, which is also its pid.
    group: String,
}

/// How a stop knows the process group it is given to be the job's (STOP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Known {
    /// The engine started the job and has asked after it since, so the group is the job's even once its leader has
    /// died: no pid is given out again while a group of that id lasts.
    Tracked,
    /// A job directory an earlier start left names it, as after a crash or a takeover: only a leader that is still the
    /// job shows that the system has not given its pid to another session since.
    Left,
}

/// Fails, before a run's interview, if host could not run the local gate: python3 does not start from PATH or is
/// older than 3.11; curl, tar, or both sha256sum and shasum, are not on PATH, or ps does not show a process's state and
/// command line; or the gate cache cannot be made or written to.
pub fn preflight(host: &dyn Host) -> Result<(), OrchestratorError> {
    let (major, minor) = PYTHON_MIN;
    let probe = format!("import sys; sys.exit(sys.version_info < ({major}, {minor}))");
    let python = host.run(&["python3", "-c", &probe], None, DEFAULT_TIMEOUT);
    if let Some(problem) = match &python {
        Err(e) => Some(e.to_string()),
        Ok(proc) if proc.status == 1 => Some("it is older".to_string()),
        Ok(proc) if proc.status != 0 => Some(format!("it exits {}", proc.status)),
        Ok(_) => None,
    } {
        return Err(OrchestratorError(format!(
            "the local quality gate runs octoflow-gate with python3 {major}.{minor} or later from PATH, and {problem}"
        )));
    }
    let tools = host.run(&["sh", "-c", PREFLIGHT], None, DEFAULT_TIMEOUT)?;
    if tools.status != 0 {
        return Err(OrchestratorError(format!(
            "the local quality gate fetches octoflow-gate into a cache of its own: {}",
            tools.stderr.trim()
        )));
    }
    Ok(())
}

/// Start the gate judging snapshot, a commit of cwd's repository whose only parent is base, and return the job, which
/// runs on, detached from the engine, until it ends or is stopped. Each start for out_dir, a quality round's directory,
/// judges in a job directory of its own, out_dir/job-<k>, k counting the round's starts, so that what one start does to
/// its files, as an engine since taken over still may, never touches another's. The jobs earlier starts left there that
/// still run are stopped first (Known::Left), and what only a running job needs is removed. The job directory holds
/// judge/ (base's pyproject.toml, an empty one for a base without, and base's gate.toml when it has one), job.log,
/// gate.err and exit, and, once the gate has judged, quality.md and quality.json; while the job runs, also group (its
/// process group), tree/ (a checkout of snapshot, a worktree of cwd's repository) and release/ (the release, unpacked),
/// which go when it ends or is stopped. The release and the settings are base's (the module's comment). The gate runs
/// the project's tests itself, with gate.toml's [[test]] commands, as the user running the engine, with no sandbox.
///
/// Errors: base's ci/gate.pin has no `<version> <sha256>` line; git, python3 or a file fails, as git does when a
/// job's tree/ was deleted while still registered as a worktree, which only `git worktree remove` or `prune` clears.
pub fn start(host: &dyn Host, cwd: &str, snapshot: &str, base: &str, out_dir: &str) -> Result<Judging, OrchestratorError> {
    let text = |path: &str| -> Result<Option<String>, OrchestratorError> {
        Ok(host.file_at(cwd, base, path)?.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    };
    let (release, sha256) = match text(PIN_PATH)? {
        Some(pin) => parse_pin(&pin).map_err(|why| OrchestratorError(format!("the run's base's {PIN_PATH}: {why}")))?,
        None => parse_pin(BUILT_IN_PIN)?,
    };
    let made = host.check(&["sh", "-c", NEXT_JOB, "_", out_dir], None, DEFAULT_TIMEOUT)?;
    let k: u32 = made
        .trim()
        .parse()
        .map_err(|_| OrchestratorError(format!("making a job directory in {out_dir} printed {made:?}")))?;
    for earlier in 1..k {
        let dir = format!("{out_dir}/job-{earlier}");
        if let (Some(group), None) = (host.read(&format!("{dir}/group"))?, host.read(&format!("{dir}/exit"))?) {
            stop_job(host, group.trim(), &dir, Known::Left)?;
        }
        clean_up(host, cwd, &dir)?;
    }
    let dir = format!("{out_dir}/job-{k}");
    let judge = format!("{dir}/judge");
    host.write(&format!("{judge}/pyproject.toml"), &text("pyproject.toml")?.unwrap_or_default(), false)?;
    if let Some(settings) = text("gate.toml")? {
        host.write(&format!("{judge}/gate.toml"), &settings, false)?;
    }
    host.add_worktree(cwd, &format!("{dir}/tree"), snapshot)?;
    let url = RELEASE_URL.replace("{v}", &release);
    let log = format!("{dir}/job.log");
    let name = job_name(&dir);
    let argv = ["python3", "-c", LAUNCH, &log, "sh", "-c", JOB, &name, &dir, &release, &sha256, &url, base];
    let group = host.check(&argv, None, DEFAULT_TIMEOUT)?.trim().to_string();
    host.write(&format!("{dir}/group"), &group, false)?;
    Ok(Judging { cwd: cwd.to_string(), dir, release, group })
}

impl Judging {
    /// What the gate came to; None while it runs. Once it has ended, what only a running job needs is removed.
    ///
    /// Errors: the job could not fetch or unpack the release, as for a download that fails or does not match the pin's
    /// sha256; it ended without saying how, as when killed; the gate exits other than 0, 1 or 2, or 2 without an error
    /// of its own, as python3 does for a gate.py it cannot open and argparse for arguments the release does not take,
    /// or exits 0 or 1 without both reports; git or a file fails.
    pub fn verdict(&self, host: &dyn Host) -> Result<Option<LocalVerdict>, OrchestratorError> {
        let out = &self.dir;
        let ended = match host.read(&format!("{out}/exit"))? {
            Some(ended) => ended,
            None if self.running(host)? => return Ok(None),
            // The job may have ended between the two reads.
            None => host.read(&format!("{out}/exit"))?.unwrap_or_default(),
        };
        clean_up(host, &self.cwd, out)?;
        let release = &self.release;
        let log = |name: &str| -> Result<String, OrchestratorError> {
            Ok(last_lines(&host.read(&format!("{out}/{name}"))?.unwrap_or_default()))
        };
        match ended.trim() {
            "0" | "1" => {
                let read = |name: &str| {
                    host.read(&format!("{out}/{name}"))?.ok_or_else(|| {
                        OrchestratorError(format!("octoflow-gate {release} judged but wrote no {out}/{name}"))
                    })
                };
                let (quality, json) = (read("quality.md")?, read("quality.json")?);
                Ok(Some(LocalVerdict::Judged { quality, json, release: release.clone() }))
            }
            // gate.py says `error: <why>` when it cannot judge the change. argparse's usage error and python3's own
            // say otherwise: the release or the host is at fault, which nothing the Builder changes mends.
            "2" if could_not_judge(&log("gate.err")?) => Ok(Some(LocalVerdict::CouldNotJudge(log("gate.err")?))),
            "setup" => Err(OrchestratorError(format!("fetching octoflow-gate {release}: {}", log("job.log")?))),
            "" => Err(OrchestratorError(format!(
                "octoflow-gate {release} stopped before it judged, its job gone without a status: {}",
                log("job.log")?
            ))),
            status => Err(OrchestratorError(format!("octoflow-gate {release} exited {status}: {}", log("gate.err")?))),
        }
    }

    /// Stop the job, its tests included, as a Ctrl-C would, and remove what only a running job needs. Errors: a
    /// command or git fails.
    pub fn stop(&self, host: &dyn Host) -> Result<(), OrchestratorError> {
        stop_job(host, &self.group, &self.dir, Known::Tracked)?;
        clean_up(host, &self.cwd, &self.dir)
    }

    /// Whether any process of the job's group is left.
    fn running(&self, host: &dyn Host) -> Result<bool, OrchestratorError> {
        let probe = ["sh", "-c", "kill -0 -\"$1\" 2>/dev/null", "_", &self.group];
        Ok(host.run(&probe, None, DEFAULT_TIMEOUT)?.status == 0)
    }
}

/// Whether said, the end of the gate's standard error, is gate.py's own report of a change it could not judge.
fn could_not_judge(said: &str) -> bool {
    said.lines().any(|line| line.starts_with("error: ")) && !said.lines().any(|line| line.starts_with("usage: "))
}

/// Stop process group, the job of the job directory dir, as STOP does, known to be the job's as known says.
fn stop_job(host: &dyn Host, group: &str, dir: &str, known: Known) -> Result<(), OrchestratorError> {
    let known = match known {
        Known::Tracked => "tracked",
        Known::Left => "left",
    };
    host.check(&["sh", "-c", STOP, "_", group, &job_name(dir), known], None, DEFAULT_TIMEOUT).map(drop)
}

/// The name the job of the job directory dir runs under, its $0: dir's bytes in hex, so that ps shows it as it is in
/// any locale, where it shows dir itself with `?` for each byte outside ASCII under LC_ALL=C.
fn job_name(dir: &str) -> String {
    let hex: String = dir.bytes().map(|b| format!("{b:02x}")).collect();
    format!("octoflow-gate-job-{hex}")
}

/// Remove what only a running job needs from the job directory dir, of an ended or stopped job: its group, so no later
/// start signals it, the snapshot's checkout, by git, so the repository forgets it and no other worktree's
/// registration is touched, and the release and its download. Errors: a command or git fails.
fn clean_up(host: &dyn Host, cwd: &str, dir: &str) -> Result<(), OrchestratorError> {
    let tree = format!("{dir}/tree");
    if host.is_worktree(cwd, &tree)? {
        // Another engine may remove it meanwhile: a start clears what the jobs before it left.
        if let Err(e) = host.remove_worktree(cwd, &tree, true) {
            if host.is_worktree(cwd, &tree)? {
                return Err(e);
            }
        }
    }
    let gone = [format!("{dir}/group"), format!("{dir}/release"), format!("{dir}/download.tar.gz")];
    host.check(&["rm", "-rf", "--", &gone[0], &gone[1], &gone[2]], None, DEFAULT_TIMEOUT).map(drop)
}

/// The release version and sha256 a pin names, read as ci/fetch-gate.sh reads one: the first line not starting with
/// #, `<version> <sha256>`, the version only letters, digits and `.+-`, as it goes into a URL, and the sha256 64
/// lowercase hex digits. Errors: no such line.
pub fn parse_pin(pin: &str) -> Result<(String, String), OrchestratorError> {
    let line = pin.lines().find(|l| !l.starts_with('#')).unwrap_or("");
    let (version, sha256) = line.split_once(' ').unwrap_or((line, ""));
    let version_ok = !version.is_empty() && version.bytes().all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b));
    let sha_ok = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !version_ok || !sha_ok {
        return Err(OrchestratorError(format!("its first line '{line}' is not '<version> <sha256>'")));
    }
    Ok((version.to_string(), sha256.to_string()))
}

#[cfg(test)]
mod tests;
