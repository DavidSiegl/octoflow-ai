//! The machine the agents run on, as orchestrator.py's Host: its files, its clock and its git checkout, locally or
//! over `ssh -o BatchMode=yes`, and gh and gitleaks run there.
//!
//! [`Host`] is a trait so that the engine's tests can stand a fake in for the machine. Every operation is a provided
//! method built on two primitives, [`Host::exec`], which runs one command, and [`Host::ssh_target`]: [`ProcessHost`]
//! implements only those, and a fake overrides what it plays in memory, as `FakeHost` (host/fake.rs) plays git one
//! command at a time through [`Host::git_run`], so that Host's own git steps run on it.

use crate::constants::{NETWORK_TIMEOUT, REMOTE, RUNS_DIR};
use crate::error::OrchestratorError;
use crate::harness::{harness, SessionLog};
use serde_json::Value;
use crate::map::Map;
use crate::json::ValueExt;
use crate::paths::file_name;
use std::collections::HashSet;

mod process;
mod scan;

pub use process::ProcessHost;
pub use scan::LeaksFound;
pub(crate) use process::{run_process, ProcessError};

use crate::quality::Leak;

#[cfg(test)]
pub(crate) mod fake;

/// How long a command may take when its caller names no other time, in seconds.
pub const DEFAULT_TIMEOUT: u64 = 60;
/// The status the scripts of read, read_inside and session_log exit with when the file they read is not there, and
/// run_dir_resolved's when the run directory is not.
pub const MISSING_FILE_STATUS: i32 = 3;
/// gh's exit status when it is not logged in, and sh's when gh is not installed: no gh call can succeed then.
pub const GH_UNUSABLE_STATUS: [i32; 2] = [4, 127];
/// Runs a command in a directory: gh, which finds the repository from its working directory and has no -C, and
/// gitleaks, which reads an `[extend] path` from its working directory.
pub const IN_DIR: &str = "cd -- \"$1\" && shift && exec \"$@\"";
/// The name of a run directory's file that the engine's metrics command reads (crate::summary::run_metrics).
const METRICS_FILE: &str = "metrics.json";

/// sh, inside a loop over files $f: what ends that pass when the command before it failed. A file gone since the loop
/// found it is skipped, as a running run removes some of its files and prune a whole run (serve-run-files/03); one
/// still there fails the script.
const SKIP_F_IF_GONE: &str = "{ [ -f \"$f\" ] || continue; exit 1; }";

/// sh: sets m to the modification time of file $f, in seconds since the epoch; fails when stat does. stat -c is GNU,
/// stat -f is BSD.
const MTIME_OF_F: &str = "m=$(stat -c %Y -- \"$f\" 2>/dev/null || stat -f %m -- \"$f\")";

/// The status the scripts of run_files_inside and read_inside exit with when the file, or the run directory, leads
/// out of where it must stay (run_dir_resolved).
const OUTSIDE_STATUS: i32 = 5;

/// sh defining `below F D`, which succeeds when F, a file or directory, every link resolved, is below directory D, a
/// path with its links resolved, and leaves F's resolved path in r; it returns 1 when F leads out of D, and 2 when F
/// cannot be resolved, as when it, or a link on its way, was removed meanwhile, or its links loop. It uses only what
/// sh has on Linux and macOS alike: `cd -P` and `pwd -P` resolve a directory, and links are followed one at a time
/// with readlink without options (POSIX.1-2024, and GNU, BSD and BusyBox readlink alike, unlike readlink -f or
/// realpath), at most 40 deep, as Linux follows them. serve's view of a run (Host::run_files_inside, read_inside and
/// run_states_inside) is what stays below the run directory, and the run directory below the runs directory.
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

/// What one command printed, and how it ended, as [`Host::exec`] gives it: its output as the bytes it wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exec {
    /// The command as it ran, as one line of sh (shlex.join): over SSH, the ssh command that carried it.
    pub command: String,
    /// Its exit status, or minus the number of the signal that ended it, as Python's returncode.
    pub status: i32,
    /// What the command wrote to its standard output; over SSH, the remote command's, which ssh passes on.
    pub stdout: Vec<u8>,
    /// What it wrote to its standard error; over SSH, the remote command's and ssh's own, such as why it could not
    /// connect.
    pub stderr: Vec<u8>,
}

/// A command's exit status and output, as [`Host::run`] decodes it: `Completed` for text, `Completed<Vec<u8>>` for
/// output kept verbatim ([`Host::run_verbatim`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completed<T = String> {
    /// The exit status, or minus the number of the signal that ended it, as Python's returncode.
    pub status: i32,
    /// What it wrote to its standard output, as Exec::stdout says.
    pub stdout: T,
    /// What it wrote to its standard error, as Exec::stderr says. For a verbatim run, decoded with each byte that is
    /// not UTF-8 replaced, as it is only ever shown.
    pub stderr: String,
}

impl Completed {
    /// A command that exited with status, having printed stdout and stderr: what a fake answers a command with.
    pub fn new(status: i32, stdout: &str, stderr: &str) -> Completed {
        Completed { status, stdout: stdout.to_string(), stderr: stderr.to_string() }
    }
}

/// A file a change touches, as [`Host::changed_line_counts`] gives it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangedFile {
    /// Its path at the base: path, unless git paired the file with another as a rename.
    pub base_path: String,
    /// Its path in the change; for a file the change adds or deletes, the side without it has no file there.
    pub path: String,
    /// Lines added; 0 for a binary file.
    pub added: u64,
    /// Lines removed; 0 for a binary file.
    pub removed: u64,
}

/// A file in a run directory, as [`Host::run_files`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunFile {
    /// The run directory's path, a slash and the file's name.
    pub path: String,
    /// When it was last modified, in seconds since the epoch by its host's clock.
    pub mtime: i64,
}

/// A run directory's file as [`Host::read_inside`] reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunFileText {
    /// The file's text.
    Text(String),
    /// There is no such file, or no such run directory.
    Missing,
    /// The file, or the run directory, leads out of where it must stay; nothing was read.
    Outside,
}

/// A pull request as [`Host::pull_request`] reads it from gh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    /// OPEN, CLOSED or MERGED.
    pub state: String,
    /// The commit its head branch is at: 40 to 64 lowercase hex digits.
    pub head_ref_oid: String,
    /// When it was closed or merged, as GitHub writes the time; None while it is open.
    pub closed_at: Option<String>,
}

/// A pull request as [`Host::pull_request_merge`] reads it from gh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestMerge {
    /// OPEN, CLOSED or MERGED.
    pub state: String,
    /// Whether it is a draft, which cannot be merged until it is marked ready.
    pub is_draft: bool,
    /// MERGEABLE, CONFLICTING, or UNKNOWN while GitHub has not computed it.
    pub mergeable: String,
    /// Its description, as Markdown.
    pub body: String,
}

/// Why a gh command about a pull request failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GhError {
    /// gh is missing or logged out where the project is, so no gh call can succeed: Python's GhUnusable.
    Unusable(OrchestratorError),
    /// Any other failure: gh failed, or printed what is not the pull request asked for.
    Failed(OrchestratorError),
}

impl From<GhError> for OrchestratorError {
    fn from(e: GhError) -> OrchestratorError {
        match e {
            GhError::Unusable(e) | GhError::Failed(e) => e,
        }
    }
}

/// Why [`Host::exec`] gave no Exec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecError {
    /// The command's program is not on this host's PATH, so it could not be started: Python's FileNotFoundError from
    /// subprocess. Over SSH it is the remote shell that cannot find a program, which exits 127 as an Exec does.
    NotFound(OrchestratorError),
    /// Any other failure: the command could not be started otherwise, timed out, was interrupted, or its output could
    /// not be read.
    Failed(OrchestratorError),
}

impl From<ExecError> for OrchestratorError {
    fn from(e: ExecError) -> OrchestratorError {
        match e {
            ExecError::NotFound(e) | ExecError::Failed(e) => e,
        }
    }
}

impl From<OrchestratorError> for ExecError {
    fn from(e: OrchestratorError) -> ExecError {
        ExecError::Failed(e)
    }
}

/// Why a push of a run's work did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushError {
    /// The secret scan found secrets in what the push would publish: Python's LeaksFound.
    Leaks(LeaksFound),
    /// The scan could not judge, or the push failed.
    Failed(OrchestratorError),
}

impl From<OrchestratorError> for PushError {
    fn from(e: OrchestratorError) -> PushError {
        PushError::Failed(e)
    }
}

impl From<PushError> for OrchestratorError {
    fn from(e: PushError) -> OrchestratorError {
        match e {
            PushError::Leaks(leaks) => OrchestratorError(leaks.to_string()),
            PushError::Failed(e) => e,
        }
    }
}

/// The machine the agents run on: its files, its clock, its git checkouts, gh and gitleaks, reached locally or over
/// SSH. Each path is one on that machine; each command runs with the environment child_env gives
/// (crate::env::child_env), so no process it starts inherits the CI credentials.
///
/// Every error is an [`OrchestratorError`] naming the command, and, over SSH, the target, unless a method says
/// otherwise. Timeouts are in seconds; a method that names none gives each command [`DEFAULT_TIMEOUT`], and those that
/// reach origin or GitHub NETWORK_TIMEOUT.
pub trait Host {
    /// The SSH target the commands run on, as ssh takes it (`[user@]host`); None for this machine.
    fn ssh_target(&self) -> Option<&str>;

    /// Runs argv on this host and gives its exit status and output, as they are, once it has exited and closed its
    /// output. stdin, when given, is its standard input, else it reads none. Errors: ExecError::NotFound when argv[0]
    /// is not on PATH; ExecError::Failed when argv cannot be started otherwise, or runs longer than timeout seconds,
    /// in which case every process it started that has not left its process group is killed.
    fn exec(&self, argv: &[&str], stdin: Option<&[u8]>, timeout: u64) -> Result<Exec, ExecError>;

    /// argv's exit status and output as text, each line end made "\n", which suits output that is parsed, such as
    /// JSON or a state file. stdin is as for exec. Errors: as exec's, and output that is not UTF-8.
    fn run(&self, argv: &[&str], stdin: Option<&str>, timeout: u64) -> Result<Completed, OrchestratorError> {
        decoded(self.exec(argv, stdin.map(str::as_bytes), timeout)?)
    }

    /// argv's exit status and output as the command printed it, for output that carries paths or file contents: line
    /// ends as they are, and bytes that are not UTF-8 kept. stdin and the errors are as exec's.
    fn run_verbatim(
        &self,
        argv: &[&str],
        stdin: Option<&[u8]>,
        timeout: u64,
    ) -> Result<Completed<Vec<u8>>, OrchestratorError> {
        let exec = self.exec(argv, stdin, timeout)?;
        Ok(Completed {
            status: exec.status,
            stdout: exec.stdout,
            stderr: String::from_utf8_lossy(&exec.stderr).into_owned(),
        })
    }

    /// argv's standard output, run as run runs it. Errors: as run's, and argv exiting with a status other than 0
    /// (checked).
    fn check(&self, argv: &[&str], stdin: Option<&str>, timeout: u64) -> Result<String, OrchestratorError> {
        checked(self, argv, self.run(argv, stdin, timeout)?)
    }

    /// The file's contents, or None when it does not exist yet. Errors: it cannot be read, or is not UTF-8.
    fn read(&self, path: &str) -> Result<Option<String>, OrchestratorError> {
        let script = format!("[ -f \"$1\" ] || exit {MISSING_FILE_STATUS}; cat -- \"$1\"");
        let proc = self.run(&["sh", "-c", &script, "_", path], None, DEFAULT_TIMEOUT)?;
        match proc.status {
            MISSING_FILE_STATUS => Ok(None),
            0 => Ok(Some(proc.stdout)),
            _ => Err(OrchestratorError(format!("reading {path}: {}", proc.stderr.trim()))),
        }
    }

    /// Whether anything, a file, a directory or a link, is at path. Errors: it cannot be told, as when the host cannot
    /// be reached.
    fn exists(&self, path: &str) -> Result<bool, OrchestratorError> {
        let script = format!("[ -e \"$1\" ] || [ -L \"$1\" ] || exit {MISSING_FILE_STATUS}");
        let proc = self.run(&["sh", "-c", &script, "_", path], None, DEFAULT_TIMEOUT)?;
        match proc.status {
            0 => Ok(true),
            MISSING_FILE_STATUS => Ok(false),
            _ => Err(OrchestratorError(format!("checking for {path}: {}", proc.stderr.trim()))),
        }
    }

    /// Replace the file's contents with text, making its directory if it is missing; with keep_mtime, the existing
    /// file keeps its modification time. keep_mtime is for a state.json written by someone other than its run: its
    /// age is the run's heartbeat. The file is written aside and renamed into place, so a reader never sees half of
    /// it.
    fn write(&self, path: &str, text: &str, keep_mtime: bool) -> Result<(), OrchestratorError> {
        let keep = if keep_mtime { " && touch -r \"$1\" -- \"$t\"" } else { "" };
        let script = format!(
            "t=\"$1.tmp.$$\"; mkdir -p -- \"$(dirname -- \"$1\")\" && cat > \"$t\"{keep} && mv -f -- \"$t\" \"$1\" \
             || {{ rm -f -- \"$t\"; exit 1; }}"
        );
        self.check(&["sh", "-c", &script, "_", path], Some(text), DEFAULT_TIMEOUT).map(drop)
    }

    /// Move the file at path to new_path, replacing what is there.
    fn rename(&self, path: &str, new_path: &str) -> Result<(), OrchestratorError> {
        self.check(&["mv", "-f", "--", path, new_path], None, DEFAULT_TIMEOUT).map(drop)
    }

    /// The time on this host, in seconds since the epoch. now and mtime both read this host, the one that stamps the
    /// handoff files, so skew between machines cannot distort a turn's duration; an mtime can still trail now by a few
    /// milliseconds (Workflow::end_turn says why). Errors: date prints what is not a time.
    fn now(&self) -> Result<i64, OrchestratorError> {
        seconds(&self.check(&["date", "+%s"], None, DEFAULT_TIMEOUT)?, "date +%s")
    }

    /// When the file was last modified, in seconds since the epoch by this host's clock. Errors: there is no such
    /// file, or stat prints what is not a time.
    fn mtime(&self, path: &str) -> Result<i64, OrchestratorError> {
        // stat -c is GNU, stat -f is BSD.
        let script = "stat -c %Y -- \"$1\" 2>/dev/null || stat -f %m -- \"$1\"";
        seconds(&self.check(&["sh", "-c", script, "_", path], None, DEFAULT_TIMEOUT)?, &format!("stat of {path}"))
    }

    /// The absolute form of a directory's path, a leading ~ expanded to the home directory on this host. Errors:
    /// there is no such directory.
    fn resolve_dir(&self, path: &str) -> Result<String, OrchestratorError> {
        let (script, arg) = if path == "~" || path.starts_with("~/") {
            ("cd -- \"$HOME$1\" && pwd", &path[1..])
        } else {
            ("cd -- \"$1\" && pwd", path)
        };
        Ok(self.check(&["sh", "-c", script, "_", arg], None, DEFAULT_TIMEOUT)?.trim().to_string())
    }

    /// HEAD's commit, or None when cwd is not a git repository with a commit.
    fn git_head(&self, cwd: &str) -> Result<Option<String>, OrchestratorError> {
        let proc = self.git_run(cwd, &["rev-parse", "--verify", "HEAD"], DEFAULT_TIMEOUT)?;
        Ok((proc.status == 0).then(|| proc.stdout.trim().to_string()))
    }

    /// The standard output of git args in cwd, as run gives it. Errors: as run's, and git exiting with a status other
    /// than 0.
    fn git(&self, cwd: &str, args: &[&str]) -> Result<String, OrchestratorError> {
        git_timed(self, cwd, args, DEFAULT_TIMEOUT)
    }

    /// The standard output of git args in cwd, verbatim, as run_verbatim gives it. Errors: as git's.
    fn git_verbatim(&self, cwd: &str, args: &[&str]) -> Result<Vec<u8>, OrchestratorError> {
        let proc = self.git_run_verbatim(cwd, args, DEFAULT_TIMEOUT)?;
        checked(self, &git_argv(cwd, args), proc)
    }

    /// A git command in cwd whose failure the caller interprets, git using exit status 1 for answers such as "no";
    /// as run runs it.
    fn git_run(&self, cwd: &str, args: &[&str], timeout: u64) -> Result<Completed, OrchestratorError> {
        self.run(&git_argv(cwd, args), None, timeout)
    }

    /// git_run, with the output verbatim, as run_verbatim gives it.
    fn git_run_verbatim(
        &self,
        cwd: &str,
        args: &[&str],
        timeout: u64,
    ) -> Result<Completed<Vec<u8>>, OrchestratorError> {
        self.run_verbatim(&git_argv(cwd, args), None, timeout)
    }

    /// Fetch the branch from origin and return its remote-tracking ref, `origin/<branch>`. Errors: the fetch fails,
    /// as for a branch origin lacks.
    fn fetch(&self, cwd: &str, branch: &str) -> Result<String, OrchestratorError> {
        git_timed(self, cwd, &["fetch", "--quiet", REMOTE, branch], NETWORK_TIMEOUT)
            .map_err(|e| OrchestratorError(format!("could not fetch {branch} from {REMOTE}: {e}")))?;
        Ok(format!("{REMOTE}/{branch}"))
    }

    /// Bring the checked-out branch up to origin's; one only ahead of origin's is left as it is. Errors: the fetch
    /// fails, or the branch has diverged from origin's or cannot be fast-forwarded to it, saying to reconcile it.
    fn fast_forward(&self, cwd: &str, branch: &str) -> Result<(), OrchestratorError> {
        let upstream = self.fetch(cwd, branch)?;
        let args = ["merge", "--ff-only", "--quiet", upstream.as_str()];
        let proc = self.git_run(cwd, &args, DEFAULT_TIMEOUT)?;
        if proc.status == 0 {
            return Ok(());
        }
        let why = if git_test(self, cwd, &["merge-base", "--is-ancestor", "HEAD", &upstream])? {
            format!("{branch} cannot be fast-forwarded to {upstream}")
        } else {
            format!("{branch} has diverged from {upstream}")
        };
        Err(OrchestratorError(format!(
            "{why}; reconcile it first. git {} failed: {}",
            shlex_join(&args),
            proc.stderr.trim()
        )))
    }

    /// Merge origin's latest branch into HEAD with a merge commit, and return the files that conflict. Nothing is
    /// merged when HEAD already holds it, so a repeated call merges once. A conflicting merge is aborted, leaving HEAD
    /// and the working tree as they were. Errors: the fetch fails, or the merge fails without a conflict.
    fn merge_upstream(&self, cwd: &str, branch: &str) -> Result<Vec<String>, OrchestratorError> {
        let upstream = self.fetch(cwd, branch)?;
        if git_test(self, cwd, &["merge-base", "--is-ancestor", &upstream, "HEAD"])? {
            return Ok(vec![]);
        }
        let merge = self.git_run(cwd, &["merge", "--no-edit", "--quiet", &upstream], DEFAULT_TIMEOUT)?;
        if merge.status == 0 {
            return Ok(vec![]);
        }
        let conflicts = nul_separated(&git_diff(self, cwd, &["--name-only", "-z", "--diff-filter=U"])?);
        self.abort_merge(cwd)?;
        if conflicts.is_empty() {
            return Err(OrchestratorError(format!(
                "git merge {upstream} failed in {cwd}: {}",
                merge.stderr.trim()
            )));
        }
        Ok(conflicts)
    }

    /// Abort a merge in progress, if there is one; true when there was.
    fn abort_merge(&self, cwd: &str) -> Result<bool, OrchestratorError> {
        if !git_test(self, cwd, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])? {
            return Ok(false);
        }
        self.git(cwd, &["merge", "--abort"])?;
        Ok(true)
    }

    /// Commit the working tree as it is, untracked files included, on top of parent, with message; returns the
    /// commit. No ref moves and the index stays as it was. `.gitignore` is honoured, which keeps `.orchestrator/` out,
    /// so the snapshot holds what committing the change would. Errors: git fails, or prints what is no commit.
    fn snapshot(&self, cwd: &str, parent: &str, message: &str) -> Result<String, OrchestratorError> {
        let tree = working_tree(self, cwd)?;
        let out = self.git(cwd, &["commit-tree", &tree, "-p", parent, "-m", message])?;
        let sha = out.trim();
        if !is_commit_id(sha) {
            return Err(OrchestratorError(format!(
                "snapshot of {cwd}: git commit-tree printed {sha:?}, not a commit"
            )));
        }
        Ok(sha.to_string())
    }

    /// The working tree as committing it would hold it, untracked files included and ignored ones not, as a git tree
    /// object, which restore_tree takes; None when cwd is in no git repository. Needs no commit. No ref moves and the
    /// index stays as it was; nothing keeps the tree from git gc, which prunes no object younger than two weeks.
    /// Errors: git fails, as `git add -A` does on a nested repository without a commit.
    fn tree_snapshot(&self, cwd: &str) -> Result<Option<String>, OrchestratorError> {
        // git exits 128 outside a repository, and gives this question no other answer with that status.
        if self.git_run(cwd, &["rev-parse", "--is-inside-work-tree"], DEFAULT_TIMEOUT)?.status == 128 {
            return Ok(None);
        }
        working_tree(self, cwd).map(Some)
    }

    /// Put the working tree back as tree, which tree_snapshot made of it, holds it: every file, untracked ones
    /// included, that differs from tree's is written as tree has it, or deleted where tree has none, as restore does.
    /// Ignored files are left as they are, by the ignore rules tree holds: changed `.gitignore` files are put back
    /// first, so a rule added since hides nothing added since. A directory a deleted file leaves empty stays, which
    /// git does not see. The index stays as it is. Errors: as tree_snapshot's and restore's, as for a nested
    /// repository without a commit added since; the tree may then be partly restored.
    fn restore_tree(&self, cwd: &str, tree: &str) -> Result<(), OrchestratorError> {
        let differ = utf8_paths(&names_between(self, cwd, tree, &working_tree(self, cwd)?, &[])?)?;
        let ignores: Vec<String> = differ.iter().filter(|p| file_name(p) == ".gitignore").cloned().collect();
        if ignores.is_empty() {
            return self.restore(cwd, tree, &differ);
        }
        self.restore(cwd, tree, &ignores)?;
        let differ = utf8_paths(&names_between(self, cwd, tree, &working_tree(self, cwd)?, &[])?)?;
        self.restore(cwd, tree, &differ)
    }

    // Locked files (Step.locks). Their paths are relative to the repository's top level, whatever directory of the
    // repository cwd is.

    /// Snapshot the working tree on top of base, store the commit under ref, replacing what ref held, and return the
    /// paths whose files the snapshot changes from base: added, edited or deleted, a rename as its two paths. ref is a
    /// local ref, which keeps the commit from git gc; nothing pushes it.
    fn lock(&self, cwd: &str, base: &str, git_ref: &str) -> Result<Vec<String>, OrchestratorError> {
        let sha = self.snapshot(cwd, base, &format!("locked files, {git_ref}"))?;
        self.git(cwd, &["update-ref", git_ref, &sha])?;
        utf8_paths(&names_between(self, cwd, base, &sha, &[])?)
    }

    /// Of paths, the ones whose file in the working tree, untracked files included, differs from ref's, a file
    /// missing on one side counting as different; and the diff from ref to the working tree over them, as text for a
    /// reader, a byte that is not UTF-8 replaced. ([], "") when none differs. Reads only: ref, the tree and the index
    /// stay as they are.
    fn locked_changes(
        &self,
        cwd: &str,
        git_ref: &str,
        paths: &[String],
    ) -> Result<(Vec<String>, String), OrchestratorError> {
        if paths.is_empty() {
            return Ok((vec![], String::new()));
        }
        let sha = self.snapshot(cwd, git_ref, &format!("the working tree against {git_ref}"))?;
        let differ = utf8_paths(&names_between(self, cwd, git_ref, &sha, paths)?)?;
        if differ.is_empty() {
            return Ok((vec![], String::new()));
        }
        let mut args = vec!["--no-color", "--no-ext-diff", "--no-renames", git_ref, &sha, "--"];
        let literal = top_literal(&differ);
        args.extend(literal.iter().map(String::as_str));
        let diff = git_diff(self, cwd, &args)?;
        Ok((differ, String::from_utf8_lossy(&diff).into_owned()))
    }

    /// Make each of paths in the working tree what it is in ref, a commit or a tree: written as ref has it, or deleted
    /// where ref has none. A directory there, such as a nested repository, keeps what the rules ref holds ignore (its
    /// `.gitignore` files, those outside it, `.git/info/exclude` and `core.excludesFile`), an ignored subdirectory
    /// whole, a repository's included, and a repository ref holds as a gitlink, whole; it loses everything else,
    /// every `.git` and every `.gitignore` ref lacks among it included, so a file only such a `.gitignore` ignored
    /// goes. The directory itself stays while it holds anything. Any number of paths. The index stays as it is.
    fn restore(&self, cwd: &str, git_ref: &str, paths: &[String]) -> Result<(), OrchestratorError> {
        if paths.is_empty() {
            return Ok(());
        }
        // The whole of ref is listed, and the paths go over stdin below, since an argv has a limit.
        let listing = self.git_verbatim(cwd, &["ls-tree", "-r", "--name-only", "-z", "--full-tree", git_ref])?;
        let held: HashSet<String> = utf8_paths(&listing)?.into_iter().collect();
        let (kept, gone): (Vec<&String>, Vec<&String>) = paths.iter().partition(|p| held.contains(*p));
        // Deleted first: where ref has a directory and the tree a file in its place, git restore makes the directory
        // again, and the file's path then names it.
        if !gone.is_empty() {
            let top = utf8_path(&self.git_verbatim(cwd, &["rev-parse", "--show-toplevel"])?)?;
            let top = top.strip_suffix('\n').unwrap_or(&top);
            let printed = sh_over_paths(self, top, r#"cd -- "$1" && xargs -0 sh -c 'for p; do
                if [ -d "$p" ] && [ ! -L "$p" ]; then printf "%s\0" "$p"; fi
            done' _"#, &gone)?;
            let dirs: HashSet<String> = nul_separated(printed.as_bytes()).into_iter().collect();
            let (dirs, files): (Vec<&String>, Vec<&String>) = gone.into_iter().partition(|p| dirs.contains(*p));
            if !files.is_empty() {
                sh_over_paths(self, top, r#"cd -- "$1" && xargs -0 rm -f --"#, &files)?;
            }
            clean_directories(self, top, &dirs, &held)?;
        }
        // git restore refuses a path that neither ref nor the index has, so only these.
        if !kept.is_empty() {
            let source = format!("--source={git_ref}");
            let listed: String = top_literal(&kept).iter().map(|p| format!("{p}\0")).collect();
            let args = ["restore", source.as_str(), "--worktree", "--pathspec-from-file=-", "--pathspec-file-nul"];
            self.check(&git_argv(cwd, &args), Some(&listed), DEFAULT_TIMEOUT)?;
        }
        Ok(())
    }

    /// Delete every local ref of cwd's repository whose name starts with prefix, such as `refs/orchestrator/<key>/`,
    /// and return whether there was any; none is no error. Errors: a git command fails, cwd outside a repository git
    /// will read included, some of the refs perhaps deleted already.
    fn delete_refs(&self, cwd: &str, prefix: &str) -> Result<bool, OrchestratorError> {
        let listed = self.git(cwd, &["for-each-ref", "--format=%(refname)", prefix])?;
        for git_ref in listed.split_whitespace() {
            self.git(cwd, &["update-ref", "-d", git_ref])?;
        }
        Ok(!listed.is_empty())
    }

    /// `git diff -U0` from base to commit, or to the working tree's tracked files without one, in the one shape
    /// crate::patch::added_lines parses, whatever the user's git config (git_diff). Verbatim: a line that is not
    /// UTF-8, such as Latin-1 text or a binary file git diffs as text, is kept, so a change to such a file is judged
    /// like any other, and only git's "\n" ends a line, so nothing inside an added line can pass for a header.
    fn change_diff(&self, cwd: &str, base: &str, commit: Option<&str>) -> Result<Vec<u8>, OrchestratorError> {
        let mut args = vec!["-U0", "-M", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", base];
        args.extend(commit);
        args.push("--");
        git_diff(self, cwd, &args)
    }

    /// The file at path, relative to the repository's top level and taken literally, as commit has it, its bytes as
    /// git stores them; None where commit has no file there. A symbolic link gives the path it points to. Errors: git
    /// cannot read commit, or cwd is not in a git repository.
    fn file_at(&self, cwd: &str, commit: &str, path: &str) -> Result<Option<Vec<u8>>, OrchestratorError> {
        Ok(scan::blob_at(self, cwd, commit, path)?.map(|(text, _)| text))
    }

    /// Each file the change from base to commit touches, a rename paired as change_diff pairs it, with the lines it
    /// adds and removes, from `git diff --numstat`. Errors: git fails, or names a path that is not UTF-8.
    fn changed_line_counts(&self, cwd: &str, base: &str, commit: &str) -> Result<Vec<ChangedFile>, OrchestratorError> {
        let out = git_diff(self, cwd, &["--numstat", "-z", "-M", "--no-ext-diff", base, commit])?;
        numstat(&out)
    }

    /// Push commit to origin's branch, replacing what is there, once scan_unpublished finds no secret in what it
    /// would publish; base and report_path are as scan_unpublished takes them. Errors: PushError::Leaks with what the
    /// scan found, or PushError::Failed when it cannot judge or the push fails; nothing is pushed then.
    fn push_ref(&self, cwd: &str, commit: &str, branch: &str, base: &str, report_path: &str) -> Result<(), PushError> {
        self.refuse_leaks(cwd, commit, base, report_path)?;
        // --force replaces what a failed attempt left under the same name.
        let refspec = format!("{commit}:refs/heads/{branch}");
        git_timed(self, cwd, &["push", "--quiet", "--force", REMOTE, &refspec], NETWORK_TIMEOUT)?;
        Ok(())
    }

    /// Push the local branch to origin's branch of that name and track it, once scan_unpublished finds no secret in
    /// what its commit would publish; base and report_path are as scan_unpublished takes them. Errors: as push_ref's.
    fn push_branch(&self, cwd: &str, branch: &str, base: &str, report_path: &str) -> Result<(), PushError> {
        let commit = self.git(cwd, &["rev-parse", "--verify", &format!("refs/heads/{branch}")])?;
        self.refuse_leaks(cwd, commit.trim(), base, report_path)?;
        git_timed(self, cwd, &["push", "--quiet", "--set-upstream", REMOTE, branch], NETWORK_TIMEOUT)?;
        Ok(())
    }

    /// Why gitleaks with its `git` subcommand (8.19 and later), which scan_unpublished runs, cannot run here; None
    /// when it can. Its help, not its version, is asked: a distribution's build may print no version number.
    fn gitleaks_problem(&self) -> Option<String> {
        match self.run(&["gitleaks", "git", "--help"], None, DEFAULT_TIMEOUT) {
            Err(e) => Some(format!("gitleaks cannot be started ({e})")),
            Ok(proc) if proc.status != 0 => Some("`gitleaks git` does not run".to_string()),
            Ok(_) => None,
        }
    }

    /// Errors, saying how to install it, when gitleaks_problem finds a problem.
    fn check_gitleaks(&self) -> Result<(), OrchestratorError> {
        match self.gitleaks_problem() {
            Some(problem) => Err(OrchestratorError(format!(
                "this run pushes, and every push is scanned for secrets first, but {problem}: {}",
                crate::constants::GITLEAKS_INSTALL
            ))),
            None => Ok(()),
        }
    }

    /// PushError::Leaks when scan_unpublished finds any in what a push of commit would publish, and
    /// PushError::Failed when it cannot judge, as scan_unpublished says, which also says what it writes beside
    /// report_path.
    fn refuse_leaks(&self, cwd: &str, commit: &str, base: &str, report_path: &str) -> Result<(), PushError> {
        let leaks = self.scan_unpublished(cwd, commit, base, report_path)?;
        if leaks.is_empty() {
            return Ok(());
        }
        Err(PushError::Leaks(LeaksFound { leaks, report_path: report_path.to_string() }))
    }

    /// The secrets gitleaks finds, and the `gitleaks:allow` comments added, in the commits a push of commit would
    /// publish: those reachable from it and from no origin ref. A merge's own content is what it adds beyond git's
    /// automatic merge of its parents (git's remerge diff), and a range holding a merge of three or more parents, of
    /// which git gives no such diff, is refused. A remote-tracking ref for a branch origin has since deleted or
    /// rewritten narrows the range: what only it reaches was public once, and is not scanned again.
    ///
    /// The rules come from base, a commit before the change: its .gitleaks.toml (else gitleaks' defaults), each file
    /// it extends with `[extend] path`, in turn, and its .gitleaksignore (else none), and nothing the change holds, so
    /// that the change cannot allowlist itself. Every file git diffs as text is diffed with no textconv, whatever
    /// any tree's .gitattributes says. A change git diffs as binary is left out on purpose, and nothing catches a
    /// secret in it. git does so when either side of a file's change is binary to git: most often a NUL in its first
    /// 8000 bytes or a size over core.bigFileThreshold, and also whatever the user's git configuration or attribute
    /// files ($GIT_DIR/info/attributes, core.attributesFile) say makes a path binary, as `-diff` does. So a text
    /// file replacing a binary one goes unread too. gitleaks skips such a change before any rule runs, path rules and its own `pkcs12-file` rule
    /// included, and the comment check never reads it, so a `.p12` keystore, or a key or `gitleaks:allow` comment in
    /// such a change, goes unreported (TODO.md, "The secret scan does not look inside a change git diffs as binary").
    /// gitleaks honours a `gitleaks:allow` comment silently, so each one an added line holds is a Leak of its own.
    /// gitleaks runs with --redact and writes its JSON report to report_path, a path in the run's directory, beside
    /// the .gitleaksignore copy, the rules directory and the empty directory it scans from.
    ///
    /// Errors, failing closed: git cannot read base or the range; base's rules cannot be taken whole from base (a
    /// file in the chain is no TOML, extends what base lacks or what leaves the repository, or is a symbolic link) or
    /// its .gitleaksignore is a symbolic link; the range holds an octopus merge; or gitleaks cannot be started, exits
    /// other than 0 or 1, writes no report or one that is not a list of findings, or exits 1 with an empty one, as it
    /// does when it cannot load its config.
    fn scan_unpublished(
        &self,
        cwd: &str,
        commit: &str,
        base: &str,
        report_path: &str,
    ) -> Result<Vec<Leak>, OrchestratorError> {
        scan::scan_unpublished(self, cwd, commit, base, report_path)
    }

    /// Delete origin's branch.
    fn delete_remote_branch(&self, cwd: &str, branch: &str) -> Result<(), OrchestratorError> {
        let git_ref = format!("refs/heads/{branch}");
        git_timed(self, cwd, &["push", "--quiet", REMOTE, "--delete", &git_ref], NETWORK_TIMEOUT).map(drop)
    }

    /// origin's branches whose names start with prefix.
    fn remote_branches(&self, cwd: &str, prefix: &str) -> Result<Vec<String>, OrchestratorError> {
        let out = git_timed(self, cwd, &["ls-remote", "--heads", REMOTE], NETWORK_TIMEOUT)?;
        let wanted = format!("refs/heads/{prefix}");
        Ok(out
            .lines()
            .filter_map(|line| line.split_once('\t').map(|(_, name)| name))
            .filter(|name| name.starts_with(&wanted))
            .map(|name| name["refs/heads/".len()..].to_string())
            .collect())
    }

    /// The commit origin's branch is at; None when origin has no such branch.
    fn remote_branch_commit(&self, cwd: &str, branch: &str) -> Result<Option<String>, OrchestratorError> {
        let git_ref = format!("refs/heads/{branch}");
        let out = git_timed(self, cwd, &["ls-remote", REMOTE, &git_ref], NETWORK_TIMEOUT)?;
        // ls-remote matches its pattern against the end of each ref's name, so the exact name is picked out.
        Ok(out.lines().find_map(|line| {
            let (commit, name) = line.split_once('\t').unwrap_or((line, ""));
            (name == git_ref).then(|| commit.to_string())
        }))
    }

    /// Delete origin's branch while it is at commit; the lease fails the push if anyone pushed since.
    fn delete_remote_branch_at(&self, cwd: &str, branch: &str, commit: &str) -> Result<(), OrchestratorError> {
        let git_ref = format!("refs/heads/{branch}");
        let lease = format!("--force-with-lease={git_ref}:{commit}");
        git_timed(self, cwd, &["push", "--quiet", &lease, REMOTE, "--delete", &git_ref], NETWORK_TIMEOUT).map(drop)
    }

    /// Drop `origin/<branch>`, which outlives the branch on origin until a fetch prunes it; none is no error.
    fn delete_tracking_ref(&self, cwd: &str, branch: &str) -> Result<(), OrchestratorError> {
        self.git(cwd, &["update-ref", "-d", &format!("refs/remotes/{REMOTE}/{branch}")]).map(drop)
    }

    /// The commit the local branch is at; None when there is no such branch.
    fn branch_commit(&self, cwd: &str, branch: &str) -> Result<Option<String>, OrchestratorError> {
        let git_ref = format!("refs/heads/{branch}");
        let args = ["rev-parse", "-q", "--verify", git_ref.as_str()];
        let proc = self.git_run(cwd, &args, DEFAULT_TIMEOUT)?;
        if proc.status == 1 {
            return Ok(None);
        }
        Ok(Some(checked(self, &git_argv(cwd, &args), proc)?.trim().to_string()))
    }

    /// Whether this clone has commit.
    fn has_commit(&self, cwd: &str, commit: &str) -> Result<bool, OrchestratorError> {
        git_test(self, cwd, &["rev-parse", "-q", "--verify", &format!("{commit}^{{commit}}")])
    }

    /// Whether commit is of or one of its ancestors.
    fn is_ancestor(&self, cwd: &str, commit: &str, of: &str) -> Result<bool, OrchestratorError> {
        git_test(self, cwd, &["merge-base", "--is-ancestor", commit, of])
    }

    /// How many commits the local branch has that commit lacks. Errors: git fails, or prints what is no count.
    fn commits_beyond(&self, cwd: &str, commit: &str, branch: &str) -> Result<u64, OrchestratorError> {
        let range = format!("{commit}..refs/heads/{branch}");
        let out = self.git(cwd, &["rev-list", "--count", &range])?;
        let count = out.trim();
        match count.parse() {
            Ok(n) if count.bytes().all(|b| b.is_ascii_digit()) => Ok(n),
            _ => Err(OrchestratorError(format!("git rev-list --count {range} printed {count:?}, not a count"))),
        }
    }

    /// Delete the local branch, whether or not it is merged.
    fn delete_branch(&self, cwd: &str, branch: &str) -> Result<(), OrchestratorError> {
        // -D: -d checks the branch against HEAD or its upstream, not its pull request, so it refuses a squash merge.
        self.git(cwd, &["branch", "--quiet", "-D", branch]).map(drop)
    }

    /// Check commit out, on a detached HEAD, in a new worktree of cwd's repository at path.
    fn add_worktree(&self, cwd: &str, path: &str, commit: &str) -> Result<(), OrchestratorError> {
        self.git(cwd, &["worktree", "add", "--detach", path, commit]).map(drop)
    }

    /// Whether cwd's repository has a remote named origin: git names its URL. Any exit of git's but 0 counts as
    /// none, as outside a repository. Errors: git cannot be run, or runs out of time (run's).
    fn has_origin(&self, cwd: &str) -> Result<bool, OrchestratorError> {
        Ok(self.git_run(cwd, &["remote", "get-url", REMOTE], DEFAULT_TIMEOUT)?.status == 0)
    }

    /// Whether path is a worktree of cwd's repository, registered and still there.
    fn is_worktree(&self, cwd: &str, path: &str) -> Result<bool, OrchestratorError> {
        // git lists a worktree by its physical path, which differs from path when path goes through a symlink.
        let proc = self.run(&["sh", "-c", "cd -P -- \"$1\" && pwd -P", "_", path], None, DEFAULT_TIMEOUT)?;
        if proc.status != 0 {
            return Ok(false);
        }
        let listed = format!("worktree {}", proc.stdout.trim());
        let out = self.git(cwd, &["worktree", "list", "--porcelain"])?;
        // One record per worktree, separated by a blank line; "prunable" marks one whose directory is gone.
        Ok(out.split("\n\n").any(|record| {
            let lines: Vec<&str> = record.lines().collect();
            lines.first() == Some(&listed.as_str()) && !lines.iter().any(|l| l.starts_with("prunable"))
        }))
    }

    /// Remove the worktree at path; without force, git refuses one with changes it would lose.
    fn remove_worktree(&self, cwd: &str, path: &str, force: bool) -> Result<(), OrchestratorError> {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(path);
        self.git(cwd, &args).map(drop)
    }

    /// Remove the worktree at path without force, as remove_worktree, but with git's refusal (a locked worktree, a
    /// submodule, changes it would lose) an answer: Ok(Some(git's message)), the worktree still there; Ok(None) once
    /// removed. Errors: git cannot be run to its end (it does not start, runs out of time, or is interrupted).
    fn try_remove_worktree(&self, cwd: &str, path: &str) -> Result<Option<String>, OrchestratorError> {
        let proc = self.git_run(cwd, &["worktree", "remove", path], DEFAULT_TIMEOUT)?;
        Ok((proc.status != 0).then(|| proc.stderr.trim().to_string()))
    }

    /// Forget the worktrees of cwd's repository whose directory is gone, as `git worktree prune`: git then no longer
    /// counts their branches as checked out. Errors: as Host::git's.
    fn prune_worktrees(&self, cwd: &str) -> Result<(), OrchestratorError> {
        self.git(cwd, &["worktree", "prune"]).map(drop)
    }

    /// The branches checked out in the repository's worktrees, the main checkout's included, each with its path, in
    /// the order git lists them. A worktree whose directory is gone (git marks it prunable) holds nothing, as
    /// is_worktree has it.
    fn checked_out_branches(&self, cwd: &str) -> Result<Map<String>, OrchestratorError> {
        let mut branches = Map::new();
        // One record per worktree, separated by a blank line.
        for record in self.git(cwd, &["worktree", "list", "--porcelain"])?.split("\n\n") {
            if record.lines().any(|line| line.starts_with("prunable")) {
                continue;
            }
            let mut path = "";
            for line in record.lines() {
                let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
                if key == "worktree" {
                    path = rest;
                } else if key == "branch" {
                    branches.insert(rest.strip_prefix("refs/heads/").unwrap_or(rest), path.to_string());
                }
            }
        }
        Ok(branches)
    }

    /// The branch checked out in the worktree at cwd, or the checkout there; None on a detached HEAD. Errors: git
    /// fails otherwise, as when cwd is no worktree.
    fn checked_out_branch(&self, cwd: &str) -> Result<Option<String>, OrchestratorError> {
        let args = ["symbolic-ref", "--quiet", "HEAD"];
        let proc = self.git_run(cwd, &args, DEFAULT_TIMEOUT)?;
        if proc.status == 1 {
            return Ok(None);
        }
        let git_ref = checked(self, &git_argv(cwd, &args), proc)?;
        let git_ref = git_ref.trim();
        Ok(Some(git_ref.strip_prefix("refs/heads/").unwrap_or(git_ref).to_string()))
    }

    /// A gh command in cwd whose failure the caller interprets.
    fn gh_run(&self, cwd: &str, args: &[&str]) -> Result<Completed, OrchestratorError> {
        let mut argv = vec!["sh", "-c", IN_DIR, "_", cwd, "gh"];
        argv.extend(args);
        self.run(&argv, None, NETWORK_TIMEOUT)
    }

    /// The pull request at url's state, head commit and closing time, as gh reports them. Errors: GhError::Unusable
    /// when gh is missing or logged out, GhError::Failed when it fails otherwise or prints what is not that.
    fn pull_request(&self, cwd: &str, url: &str) -> Result<PullRequest, GhError> {
        let (pr, out) = pr_view(self, cwd, url, "state,headRefOid,closedAt")?;
        // Each field read and judged in turn, so that the first one wrong is the one an error names.
        let parsed = (|| {
            let Some(state) = subscript(&pr, "state")?.as_str() else { return Ok(None) };
            let head = subscript(&pr, "headRefOid")?;
            let Some(head) = head.as_str() else {
                return Err(format!("headRefOid is {}, not a string", head.kind()));
            };
            if !is_commit_id(head) {
                return Ok(None);
            }
            let closed = subscript(&pr, "closedAt")?;
            if !(closed.is_null() || closed.as_str().is_some()) {
                return Ok(None);
            }
            let closed_at = closed.as_str().map(String::from);
            Ok(Some(PullRequest { state: state.to_string(), head_ref_oid: head.to_string(), closed_at }))
        })();
        pr_fields(url, &out, parsed)
    }

    /// The pull request at url's state, whether it is a draft, whether it can be merged and its body, as gh reports
    /// them. Errors: as pull_request's.
    fn pull_request_merge(&self, cwd: &str, url: &str) -> Result<PullRequestMerge, GhError> {
        let (pr, out) = pr_view(self, cwd, url, "state,isDraft,mergeable,body")?;
        // As pull_request reads its fields.
        let parsed = (|| {
            let Some(state) = subscript(&pr, "state")?.as_str() else { return Ok(None) };
            let Some(is_draft) = subscript(&pr, "isDraft")?.as_bool() else { return Ok(None) };
            let mergeable = subscript(&pr, "mergeable")?.as_str();
            let Some(mergeable) = mergeable.filter(|m| ["MERGEABLE", "CONFLICTING", "UNKNOWN"].contains(m)) else {
                return Ok(None);
            };
            let Some(body) = subscript(&pr, "body")?.as_str() else { return Ok(None) };
            let (state, mergeable, body) = (state.to_string(), mergeable.to_string(), body.to_string());
            Ok(Some(PullRequestMerge { state, is_draft, mergeable, body }))
        })();
        pr_fields(url, &out, parsed)
    }

    /// Make body the description of the pull request at url, with gh.
    fn edit_pr_body(&self, cwd: &str, url: &str, body: &str) -> Result<(), OrchestratorError> {
        let argv = ["sh", "-c", IN_DIR, "_", cwd, "gh", "pr", "edit", url, "--body-file", "-"];
        self.check(&argv, Some(body), NETWORK_TIMEOUT).map(drop)
    }

    /// Take the pull request at url out of draft, with gh.
    fn mark_pr_ready(&self, cwd: &str, url: &str) -> Result<(), OrchestratorError> {
        self.check(&["sh", "-c", IN_DIR, "_", cwd, "gh", "pr", "ready", url], None, NETWORK_TIMEOUT).map(drop)
    }

    /// Open a pull request of head into base with gh, a draft when draft says so, and return its URL. Errors: gh
    /// fails, or prints no URL.
    fn create_pr(
        &self,
        cwd: &str,
        base: &str,
        head: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<String, OrchestratorError> {
        let mut argv =
            vec!["sh", "-c", IN_DIR, "_", cwd, "gh", "pr", "create", "--base", base, "--head", head, "--title", title];
        argv.extend(["--body-file", "-"]);
        if draft {
            argv.push("--draft");
        }
        let out = self.check(&argv, Some(body), NETWORK_TIMEOUT)?;
        match out.split_whitespace().last() {
            Some(url) => Ok(url.to_string()),
            None => Err(OrchestratorError(format!("gh pr create printed no URL for {head}"))),
        }
    }

    /// Each run's saved state under the project at cwd, with the seconds since its state.json was last written,
    /// measured by this host's own clock, the one that stamped the file, so clock skew between the machines involved
    /// cannot shift it. A run whose directory is removed while the runs are read, as prune removes one, is among them
    /// or not, never an error. Errors: a state.json that is still there cannot be stat'ed or read, or is not JSON.
    fn run_states(&self, cwd: &str) -> Result<Vec<(u64, Value)>, OrchestratorError> {
        let script = format!("now=$(date +%s) || exit 1; {}", run_states_loop("true"));
        parsed_run_states(cwd, &self.check(&["sh", "-c", &script, "_", cwd], None, DEFAULT_TIMEOUT)?)
    }

    /// As run_states, but serve's view of the runs: a run whose directory, every link resolved, is not below the
    /// runs directory, links resolved, or whose state.json, links resolved, is not below its run directory, is left
    /// out, as a run without a state.json is. The links are resolved on this host, as BELOW says. Errors: as
    /// run_states'.
    fn run_states_inside(&self, cwd: &str) -> Result<Vec<(u64, Value)>, OrchestratorError> {
        // Without a runs directory there are no runs; f becomes the resolved path, which is what is stat'ed and read.
        let script = format!(
            "{BELOW}now=$(date +%s) || exit 1; runs=$(cd -P -- \"$1/{RUNS_DIR}\" 2>/dev/null && pwd -P) || exit 0; {}",
            run_states_loop("below \"${f%/*}\" \"$runs\" && below \"$f\" \"$r\" && f=$r")
        );
        parsed_run_states(cwd, &self.check(&["sh", "-c", &script, "_", cwd], None, DEFAULT_TIMEOUT)?)
    }

    /// The log in which the harness kind keeps a session's token usage (crate::harness::SessionLog), found in the
    /// home directory here, as it is on disk; None for a harness without one, a session id that is not a plain name,
    /// or no such file.
    fn session_log(&self, kind: &str, session: &str) -> Result<Option<Vec<u8>>, OrchestratorError> {
        let Some(SessionLog { glob: pattern, .. }) = harness(kind).ok().and_then(|h| h.session_log) else {
            return Ok(None);
        };
        if session.is_empty() || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Ok(None);
        }
        let script = format!(
            "cd -- \"$HOME\" || exit 1; for f in $1; do [ -f \"$f\" ] && exec cat -- \"$f\"; done; \
             exit {MISSING_FILE_STATUS}"
        );
        let glob = pattern.replace("{session}", session);
        let proc = self.run_verbatim(&["sh", "-c", &script, "_", &glob], None, DEFAULT_TIMEOUT)?;
        if proc.status == MISSING_FILE_STATUS {
            return Ok(None);
        }
        checked(self, &["sh", "-c", &script], proc).map(Some)
    }

    /// The metrics.json of each run of the project at cwd that has one, oldest run first. Errors: one that is not
    /// JSON.
    fn run_metrics(&self, cwd: &str) -> Result<Vec<Value>, OrchestratorError> {
        // Each file ends at a NUL, which no JSON text holds, so one a human pretty-printed reads as well.
        let script = format!(
            "for f in \"$1\"/{RUNS_DIR}/*/{METRICS_FILE}; do [ -f \"$f\" ] || continue; cat -- \"$f\" && printf '\\000'; \
             done; true"
        );
        let out = self.check(&["sh", "-c", &script, "_", cwd], None, DEFAULT_TIMEOUT)?;
        out.split('\0')
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                serde_json::from_str::<Value>(text)
                    .map_err(|e| OrchestratorError(format!("corrupt {METRICS_FILE} under {cwd}/{RUNS_DIR}: {e}")))
            })
            .collect()
    }

    /// The files in a run directory, oldest first, those of one time by name, except state.json and write's temporary
    /// files; none for a directory that is not there. A file removed while the directory is listed, as a running
    /// run removes some, is in the list or not, never an error. Errors: stat fails on a file that is still there, or
    /// prints what is not a time.
    fn run_files(&self, run_dir: &str) -> Result<Vec<RunFile>, OrchestratorError> {
        let out = self.check(&["sh", "-c", &run_files_loop("true"), "_", run_dir], None, DEFAULT_TIMEOUT)?;
        parsed_run_files(run_dir, &out)
    }

    /// As run_files, but only what stays inside the run directory, as serve shows it to the desktop app: a file whose
    /// path, every link resolved, is not in run_dir, links resolved, is left out, and a run_dir that, links resolved,
    /// is not in its parent directory, the runs directory, links resolved, lists none. The links are resolved on this
    /// host, as BELOW says. Errors: as run_files'.
    fn run_files_inside(&self, run_dir: &str) -> Result<Vec<RunFile>, OrchestratorError> {
        let script = format!("{}{}", run_dir_resolved(), run_files_loop("below \"$f\" \"$d\""));
        let argv = ["sh", "-c", &script, "_", run_dir];
        let proc = self.run(&argv, None, DEFAULT_TIMEOUT)?;
        if proc.status == MISSING_FILE_STATUS || proc.status == OUTSIDE_STATUS {
            return Ok(vec![]);
        }
        parsed_run_files(run_dir, &checked(self, &argv, proc)?)
    }

    /// The text of the file name, a plain name, in run_dir, if it stays inside the run directory as run_files_inside
    /// lists only those that do: Outside, having read nothing, for a file that leads out or a run_dir that does;
    /// Missing for a file not there, or removed while it is resolved. Errors: the file cannot be read, or is not UTF-8.
    fn read_inside(&self, run_dir: &str, name: &str) -> Result<RunFileText, OrchestratorError> {
        // What is read is the path below resolved, so the links it walked are not followed again; one put in its
        // place after that is, a race TODO.md records ("serve's link checks race the agent").
        let script = format!(
            "{}f=$1/$2; [ -f \"$f\" ] || exit {MISSING_FILE_STATUS}; below \"$f\" \"$d\"; \
             case $? in 0) ;; 1) exit {OUTSIDE_STATUS};; *) exit {MISSING_FILE_STATUS};; esac; cat -- \"$r\"",
            run_dir_resolved()
        );
        let proc = self.run(&["sh", "-c", &script, "_", run_dir, name], None, DEFAULT_TIMEOUT)?;
        match proc.status {
            0 => Ok(RunFileText::Text(proc.stdout)),
            MISSING_FILE_STATUS => Ok(RunFileText::Missing),
            OUTSIDE_STATUS => Ok(RunFileText::Outside),
            _ => Err(OrchestratorError(format!("reading {run_dir}/{name}: {}", proc.stderr.trim()))),
        }
    }
}

/// sh printing, for each state.json $f of a run under the project $1 for which the sh condition keep succeeds, its
/// age in seconds by $now, a blank and its text, ended by a NUL, which no JSON text holds: what parsed_run_states
/// reads. A state.json a human pretty-printed reads as well as the engine's one line.
fn run_states_loop(keep: &str) -> String {
    format!(
        "for f in \"$1\"/{RUNS_DIR}/*/state.json; do [ -f \"$f\" ] || continue; {keep} || continue; \
         {MTIME_OF_F} || {SKIP_F_IF_GONE}; s=$(cat -- \"$f\") || {SKIP_F_IF_GONE}; \
         printf \"%s %s\\000\" \"$((now - m))\" \"$s\"; done; true"
    )
}

/// The runs run_states_loop printed as out, under the project at cwd, as run_states gives them. Errors: an age that
/// is not a number, or a state that is not JSON.
fn parsed_run_states(cwd: &str, out: &str) -> Result<Vec<(u64, Value)>, OrchestratorError> {
    let mut runs = vec![];
    for entry in out.split('\0') {
        if entry.trim().is_empty() {
            continue;
        }
        let (age, text) = entry.split_once(' ').unwrap_or((entry, ""));
        let corrupt = |e: String| OrchestratorError(format!("corrupt run state under {cwd}/{RUNS_DIR}: {e}"));
        let not_int = || corrupt(format!("invalid literal for int() with base 10: {age:?}"));
        let age: i64 = python_int(age).ok_or_else(not_int)?;
        runs.push((age.max(0).unsigned_abs(), serde_json::from_str::<Value>(text).map_err(|e| corrupt(e.to_string()))?));
    }
    Ok(runs)
}

/// sh printing, for each file $f in the directory $1 that run_files lists and for which the sh condition keep
/// succeeds, "<mtime> <path>" ended by a NUL, which no path holds: what parsed_run_files reads.
fn run_files_loop(keep: &str) -> String {
    format!(
        "for f in \"$1\"/*; do [ -f \"$f\" ] || continue; \
         case \"${{f##*/}}\" in state.json|*.tmp.*) continue;; esac; \
         {keep} || continue; {MTIME_OF_F} || {SKIP_F_IF_GONE}; printf \"%s %s\\0\" \"$m\" \"$f\"; done; true"
    )
}

/// The files run_files_loop printed as out, listing run_dir, oldest first, those of one time by name. Errors: an
/// entry whose time is not one.
fn parsed_run_files(run_dir: &str, out: &str) -> Result<Vec<RunFile>, OrchestratorError> {
    let mut files = vec![];
    for entry in out.split('\0').filter(|e| !e.is_empty()) {
        let (mtime, path) = entry.split_once(' ').unwrap_or((entry, ""));
        let mtime = python_int(mtime).ok_or_else(|| {
            OrchestratorError(format!("listing {run_dir}: stat printed {mtime:?}, not a time"))
        })?;
        files.push((mtime, path.to_string()));
    }
    files.sort();
    Ok(files.into_iter().map(|(mtime, path)| RunFile { path, mtime }).collect())
}

/// proc's output, proc being argv run on host; Errors, naming argv, the SSH target if any, and proc's standard error,
/// when it exited with a status other than 0.
pub fn checked<H: Host + ?Sized, T>(host: &H, argv: &[&str], proc: Completed<T>) -> Result<T, OrchestratorError> {
    if proc.status == 0 {
        return Ok(proc.stdout);
    }
    let on = host.ssh_target().map(|t| format!(" on {t}")).unwrap_or_default();
    Err(OrchestratorError(format!("{} failed{on}: {}", shlex_join(argv), proc.stderr.trim())))
}

/// text with each "\r\n" and each lone "\r" made "\n": how the engine reads what a program printed (Host::run,
/// herdr, a Jenkins build's console) and a prompt file, so that what splits it into lines has one line end to split
/// at, and a progress bar's updates, each ended by a lone "\r", are lines of their own.
pub fn line_ends_as_lf(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// argv as one line of sh that runs it, each word quoted by shlex_quote, between single spaces.
pub fn shlex_join<S: AsRef<str>>(argv: &[S]) -> String {
    argv.iter().map(|word| shlex_quote(word.as_ref())).collect::<Vec<_>>().join(" ")
}

/// word as sh reads it back as one word, as Python's shlex.quote writes it: as it is when it is not empty and holds
/// nothing but ASCII letters, digits and `_@%+=:,./-`, else in single quotes, a quote inside written `'"'"'`.
pub fn shlex_quote(word: &str) -> String {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\"'\"'"))
}

/// `git -C cwd args`.
fn git_argv<'a>(cwd: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec!["git", "-C", cwd];
    argv.extend(args);
    argv
}

/// Host::git with timeout in seconds.
fn git_timed<H: Host + ?Sized>(host: &H, cwd: &str, args: &[&str], timeout: u64) -> Result<String, OrchestratorError> {
    checked(host, &git_argv(cwd, args), host.git_run(cwd, args, timeout)?)
}

/// Whether a yes-or-no git command answers yes: status 0 is yes, 1 is no. Errors: any other status.
fn git_test<H: Host + ?Sized>(host: &H, cwd: &str, args: &[&str]) -> Result<bool, OrchestratorError> {
    let proc = host.git_run(cwd, args, DEFAULT_TIMEOUT)?;
    match proc.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => checked(host, &git_argv(cwd, args), proc).map(|_| false),
    }
}

/// exec's exit status and output as Host::run gives them: as text, its line ends made "\n" (line_ends_as_lf).
/// Errors: the output is not UTF-8.
pub fn decoded(exec: Exec) -> Result<Completed, OrchestratorError> {
    let Exec { command, status, stdout, stderr } = exec;
    let decode = |bytes: Vec<u8>| {
        String::from_utf8(bytes)
            .map(|text| line_ends_as_lf(&text))
            .map_err(|e| OrchestratorError(format!("{command}: printed what is not UTF-8: {}", e.utf8_error())))
    };
    let stdout = decode(stdout)?;
    Ok(Completed { status, stdout, stderr: decode(stderr)? })
}

/// git pathspecs that each match exactly one of paths, relative to the repository's top level, whatever directory git
/// runs in, and with no glob in them.
fn top_literal<S: AsRef<str>>(paths: &[S]) -> Vec<String> {
    paths.iter().map(|p| format!(":(top,literal){}", p.as_ref())).collect()
}

/// The working tree in cwd as `git add -A` would stage it, as a tree object; the index stays as it was. Errors: git
/// fails.
fn working_tree<H: Host + ?Sized>(host: &H, cwd: &str) -> Result<String, OrchestratorError> {
    // Through a copy of the index, in one sh -c script, because git -C cannot pass GIT_INDEX_FILE.
    let script = "cd -- \"$1\" || exit 1; i=$(git rev-parse --git-path index) || exit 1; t=$(mktemp) || exit 1; \
                  trap 'rm -f -- \"$t\"' EXIT; \
                  if [ -f \"$i\" ]; then cp -- \"$i\" \"$t\" || exit 1; else rm -f -- \"$t\"; fi; \
                  GIT_INDEX_FILE=$t git add -A && GIT_INDEX_FILE=$t git write-tree";
    // The `else` above: a repository whose index was never written, from which git starts the copy afresh.
    Ok(host.check(&["sh", "-c", script, "_", cwd], None, DEFAULT_TIMEOUT)?.trim().to_string())
}

/// Run script with `sh -c`, its `$1` top, and paths on its stdin, each ended by a NUL; what it printed. Errors: `sh`
/// cannot be started, the script exits non-zero, or it outlasts DEFAULT_TIMEOUT.
fn sh_over_paths<H: Host + ?Sized, S: AsRef<str>>(
    host: &H,
    top: &str,
    script: &str,
    paths: &[S],
) -> Result<String, OrchestratorError> {
    let listed: String = paths.iter().map(|p| format!("{}\0", p.as_ref())).collect();
    host.check(&["sh", "-c", script, "_", top], Some(&listed), DEFAULT_TIMEOUT)
}

/// Clean each of dirs, directories under top that ref lacks, as Host::restore's doc says of such a directory. held is
/// every file and gitlink ref holds. Errors: a command fails, as `rm` on a file it may not remove.
fn clean_directories<H: Host + ?Sized>(
    host: &H,
    top: &str,
    dirs: &[&String],
    held: &HashSet<String>,
) -> Result<(), OrchestratorError> {
    if dirs.is_empty() {
        return Ok(());
    }
    // Cleaned rather than removed: their ignored files are the human's, and no ref holds them. Walked a level at a
    // time, each level losing its .git, so that git sees what is in it as the outer repository's, and its .gitignore
    // unless ref holds it, before git judges the next level's subdirectories, so that only the rules ref holds judge
    // them. Not entered: a subdirectory those rules ignore, or one ref holds, a repository of the tree before.
    let mut level: Vec<String> = dirs.iter().map(|p| p.to_string()).collect();
    while !level.is_empty() {
        let unheld: Vec<String> =
            level.iter().map(|p| format!("{p}/.gitignore")).filter(|g| !held.contains(g)).collect();
        if !unheld.is_empty() {
            sh_over_paths(host, top, r#"cd -- "$1" && xargs -0 sh -c 'for g; do
                [ -d "./$g" ] || rm -f -- "./$g" || exit 1
            done' _"#, &unheld)?;
        }
        let found = sh_over_paths(host, top, r#"cd -- "$1" && xargs -0 sh -c 'for p; do
            rm -rf -- "./$p/.git" || exit 1
            find "./$p" -mindepth 1 -maxdepth 1 -type d ! -name .git -print0 || exit 1
        done' _"#, &level)?;
        let below: Vec<String> = nul_separated(found.as_bytes())
            .into_iter()
            .map(|c| c.strip_prefix("./").unwrap_or(&c).to_string())
            .filter(|c| !held.contains(c))
            .collect();
        if below.is_empty() {
            break;
        }
        // check-ignore prints the ignored ones and exits 1 when there are none.
        let script = r#"cd -- "$1" && { git check-ignore --no-index -z --stdin; [ $? -le 1 ]; }"#;
        let ignored: HashSet<String> = nul_separated(sh_over_paths(host, top, script, &below)?.as_bytes()).into_iter().collect();
        level = below.into_iter().filter(|c| !ignored.contains(c)).collect();
    }
    // Without -x, it leaves what the outer rules ignore; with one -f, a repository the walk did not enter.
    sh_over_paths(host, top, r#"cd -- "$1" && xargs -0 sh -c 'for p; do
        git clean -fdq -- ":(top,literal)$p" || exit 1
    done' _"#, dirs)?;
    Ok(())
}

/// `git diff args` in cwd, its output verbatim as git_verbatim gives it, whatever the user's git config and whatever
/// directory of the repository cwd is: every path is relative to the repository's top level, a diff covers the whole
/// repository, and a path that is not ASCII comes as its bytes rather than quoted in octal. Every `git diff` of the
/// engine's runs through it; args are what follows `diff`. Errors: as git_verbatim's.
pub(crate) fn git_diff<H: Host + ?Sized>(host: &H, cwd: &str, args: &[&str]) -> Result<Vec<u8>, OrchestratorError> {
    // --no-relative: under the user's diff.relative, a diff run in a subdirectory lists only that subdirectory's
    // files, named relative to it, so that a top_literal pathspec for a file elsewhere matches nothing.
    let mut argv = vec!["-c", "core.quotePath=false", "diff", "--no-relative"];
    argv.extend(args);
    host.git_verbatim(cwd, &argv)
}

/// The arguments of the one `git status` the engine runs, which `git_status` sends and a fake host answers. They
/// override every user setting that hides part of a change from `git status`: `status.showUntrackedFiles` (new files
/// are listed one by one, also those in a new directory), `diff.ignoreSubmodules` and `submodule.<name>.ignore` (a
/// submodule at another commit is listed), and `core.quotePath` (a name that is not ASCII comes as its bytes, not
/// quoted in octal).
pub(crate) const GIT_STATUS_ARGS: &[&str] =
    &["-c", "core.quotePath=false", "status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"];

/// What `git status --porcelain` lists in cwd, one change or untracked file to a line, under the user's settings that
/// GIT_STATUS_ARGS names overridden. Empty means a clean working tree. Every `git status` of the engine's runs through
/// it. Errors: as Host::git's.
pub(crate) fn git_status<H: Host + ?Sized>(host: &H, cwd: &str) -> Result<String, OrchestratorError> {
    host.git(cwd, GIT_STATUS_ARGS)
}

/// The paths whose files differ between commits a and b, their mode included, a rename as its two paths, each ended
/// by a NUL, as git prints them; with paths, which are relative to the repository's top level, only those. Errors:
/// git fails.
pub(crate) fn names_between<H: Host + ?Sized>(
    host: &H,
    cwd: &str,
    a: &str,
    b: &str,
    paths: &[String],
) -> Result<Vec<u8>, OrchestratorError> {
    let mut args = vec!["--name-only", "-z", "--no-renames", "--no-ext-diff", a, b, "--"];
    let literal = top_literal(paths);
    args.extend(literal.iter().map(String::as_str));
    git_diff(host, cwd, &args)
}

/// The NUL-separated names in out, the empty ones left out, a byte that is not UTF-8 replaced by U+FFFD.
pub(crate) fn nul_separated(out: &[u8]) -> Vec<String> {
    out.split(|&b| b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect()
}

/// The NUL-separated paths git printed in out, verbatim, the empty ones left out. Errors: one that is not UTF-8.
fn utf8_paths(out: &[u8]) -> Result<Vec<String>, OrchestratorError> {
    out.split(|&b| b == 0).filter(|p| !p.is_empty()).map(utf8_path).collect()
}

/// A path git printed verbatim, as text. Errors: it is not UTF-8, which the Python engine reads as os.fsdecode
/// decodes it (TODO.md, "The Rust engine reads some input ... otherwise").
fn utf8_path(bytes: &[u8]) -> Result<String, OrchestratorError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        OrchestratorError(format!("git printed a path that is not UTF-8: {:?}", String::from_utf8_lossy(bytes)))
    })
}

/// The files of a `git diff --numstat -z` out. -z gives each entry as `added<TAB>removed<TAB>path`, or for a rename
/// `added<TAB>removed<TAB>` and then the old and new paths as fields of their own; a binary file counts "-". The list
/// ends with an empty field.
fn numstat(out: &[u8]) -> Result<Vec<ChangedFile>, OrchestratorError> {
    let fields: Vec<&[u8]> = out.split(|&b| b == 0).collect();
    let bad = || OrchestratorError(format!("git diff --numstat printed {:?}", String::from_utf8_lossy(out)));
    let count = |n: &[u8]| -> Result<u64, OrchestratorError> {
        if n == b"-" {
            return Ok(0);
        }
        std::str::from_utf8(n).ok().and_then(python_int).and_then(|n| u64::try_from(n).ok()).ok_or_else(bad)
    };
    let mut files = vec![];
    let mut i = 0;
    while i < fields.len() && !fields[i].is_empty() {
        let mut parts = fields[i].splitn(3, |&b| b == b'\t');
        let (added, removed, path) = match (parts.next(), parts.next(), parts.next()) {
            (Some(a), Some(r), Some(p)) => (count(a)?, count(r)?, p),
            _ => return Err(bad()),
        };
        let (base_path, path) = if path.is_empty() {
            let (old, new) = (fields.get(i + 1).ok_or_else(bad)?, fields.get(i + 2).ok_or_else(bad)?);
            i += 3;
            (utf8_path(old)?, utf8_path(new)?)
        } else {
            i += 1;
            (utf8_path(path)?, utf8_path(path)?)
        };
        files.push(ChangedFile { base_path, path, added, removed });
    }
    Ok(files)
}

/// gh pr view's JSON of json_fields, comma-separated, for the pull request at url, and the text it came as. Errors:
/// GhError::Unusable when gh is missing or logged out, GhError::Failed when it fails otherwise or prints what is no
/// JSON.
fn pr_view<H: Host + ?Sized>(host: &H, cwd: &str, url: &str, json_fields: &str) -> Result<(Value, String), GhError> {
    let args = ["pr", "view", url, "--json", json_fields];
    let proc = host.gh_run(cwd, &args).map_err(GhError::Failed)?;
    if GH_UNUSABLE_STATUS.contains(&proc.status) {
        let said = if proc.stderr.is_empty() { &proc.stdout } else { &proc.stderr };
        return Err(GhError::Unusable(OrchestratorError(format!(
            "gh {} failed with status {}: {}",
            shlex_join(&args),
            proc.status,
            said.trim()
        ))));
    }
    let mut argv = vec!["gh"];
    argv.extend(args);
    let out = checked(host, &argv, proc).map_err(GhError::Failed)?;
    match serde_json::from_str::<Value>(&out) {
        Ok(pr) => Ok((pr, out)),
        Err(e) => {
            Err(GhError::Failed(OrchestratorError(format!("gh pr view {url} printed {:?}: {e}", out.trim()))))
        }
    }
}

/// What a view of the pull request at url gave, out being what gh printed: the fields parsed found, or the error
/// naming out when parsing it failed (Err, saying why) or found it not a pull request's (None).
fn pr_fields<T>(url: &str, out: &str, parsed: Result<Option<T>, String>) -> Result<T, GhError> {
    let printed = format!("{:?}", out.trim());
    match parsed {
        Ok(Some(fields)) => Ok(fields),
        Ok(None) => Err(GhError::Failed(OrchestratorError(format!(
            "gh pr view {url} printed {printed}, not a pull request's state"
        )))),
        Err(why) => Err(GhError::Failed(OrchestratorError(format!("gh pr view {url} printed {printed}: {why}")))),
    }
}

/// The entry key of value, a JSON object. Errors: an object without key, naming it, or a value that is no object,
/// naming its type.
fn subscript<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    match value {
        Value::Object(map) => map.get(key).ok_or_else(|| format!("{key:?} is missing")),
        other => Err(format!("{} where an object with {key:?} belongs", other.kind())),
    }
}

/// Whether text is a full commit id: 40 to 64 lowercase hex digits, as SHA-1 and SHA-256 repositories write them.
fn is_commit_id(text: &str) -> bool {
    (40..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The seconds a command printed, out, as int() reads them; what names what printed them. Errors: out is no whole
/// number.
fn seconds(out: &str, what: &str) -> Result<i64, OrchestratorError> {
    let text = out.trim();
    python_int(text).ok_or_else(|| OrchestratorError(format!("{what} printed {text:?}, not a time")))
}

/// text as Python's int() reads a whole number in base 10, but for the blanks around it, which are what
/// char::is_whitespace takes: an optional sign, and ASCII digits with single underscores between them; None for
/// anything else, or a number beyond 64 bits.
fn python_int(text: &str) -> Option<i64> {
    let text = text.trim();
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    let well_formed = !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_');
    if !well_formed {
        return None;
    }
    text.replace('_', "").parse().ok()
}

#[cfg(test)]
pub(crate) mod tests;
