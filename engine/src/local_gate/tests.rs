//! local_gate on this machine: a real git repository for the base's files, real sh, sha256sum and tar, with curl
//! serving a stub release from a file and python3 running the stub's gate.py with sh, and LAUNCH with the real one.

use super::*;
use crate::host::tests::{environment, put, read, tool, Repo};
use crate::host::ProcessHost;
use crate::interrupt::Interrupt;
use crate::test_support::TempDir;
use std::time::Duration;

/// The quality file and quality.json the stub gate writes when it judges.
const QUALITY: &str = "GATE: ERROR\n\n## Issues on changed lines\n\n1. `a.sh:1` warning shellcheck SC2086\n";
const JSON: &str = "{\"schema_version\": 1, \"gate\": \"ERROR\"}";

/// What the stub gate says on standard error when it exits 2, unless <dir>/said holds something else.
const CANNOT_JUDGE: &str = "error: gate.toml: unknown key \"x\"";

/// A machine with the stub release served, a repository whose base commit holds base_files, and a cache of its own.
struct Bench {
    dir: TempDir,
    repo: Repo,
    /// The served tarball's sha256, as a pin names it.
    sha256: String,
}

impl Bench {
    /// The stub release is a gate.py that records its physical working directory and arguments in <dir>/gate-args
    /// and the names in that directory in <dir>/gate-files, writes coverage.xml there, sleeps 30 s when <dir>/hang
    /// exists, then exits as <dir>/exit says (0 when it is missing): with 2 it says <dir>/said, else CANNOT_JUDGE, on
    /// standard error and writes nothing; with 0 or 1 it writes QUALITY to --out and JSON beside it, unless
    /// <dir>/silent exists. The fake python3 runs `-c` with the real one, which fails when <dir>/old exists.
    fn new(base_files: &[(&str, &str)]) -> Bench {
        let dir = TempDir::new();
        let (bin, rel, served) = (dir.join("bin"), dir.join("rel"), dir.join("served.tar.gz"));
        tool(
            &bin,
            "curl",
            &format!(
                "#!/bin/sh\nwhile [ $# -gt 0 ]; do case $1 in -o) out=$2; shift ;; -*) ;; *) url=$1 ;; esac; shift; done\n\
                 echo \"$url\" >> '{log}'\ncp -- '{served}' \"$out\"\n",
                log = dir.join("curl-log"),
            ),
        );
        let real = std::process::Command::new("sh").args(["-c", "command -v python3"]).output().expect("python3");
        let real = String::from_utf8(real.stdout).unwrap().trim().to_string();
        assert!(!real.is_empty(), "python3 is on PATH");
        tool(
            &bin,
            "python3",
            &format!(
                "#!/bin/sh\ncase $1 in\n--version) echo 'Python 3.12.0' ;;\n\
                 -c) [ -e '{old}' ] && exit 1; exec '{real}' \"$@\" ;;\n*) exec sh \"$@\" ;;\nesac\n",
                old = dir.join("old"),
            ),
        );
        put(
            &format!("{rel}/gate.py"),
            &format!(
                "pwd -P > '{args}'; printf '%s\\n' \"$@\" >> '{args}'; ls -A > '{files}'; : > coverage.xml\n\
                 [ -e '{hang}' ] && sleep 30\n\
                 status=$(cat '{exit}' 2>/dev/null || echo 0)\n\
                 [ \"$status\" = 2 ] && {{ cat '{said}' >&2 2>/dev/null || echo '{CANNOT_JUDGE}' >&2; exit 2; }}\n\
                 while [ $# -gt 0 ]; do [ \"$1\" = --out ] && out=$2; shift; done\n\
                 [ -e '{silent}' ] || {{ printf '%s' '{QUALITY}' > \"$out\"; printf '%s' '{JSON}' > \"${{out%.md}}.json\"; }}\n\
                 exit \"$status\"\n",
                args = dir.join("gate-args"),
                files = dir.join("gate-files"),
                hang = dir.join("hang"),
                exit = dir.join("exit"),
                said = dir.join("said"),
                silent = dir.join("silent"),
            ),
        );
        put(&format!("{rel}/gate-rust-linux-x86_64"), "");
        let tar = std::process::Command::new("tar").args(["-czf", &served, "-C", &rel, "."]).status().expect("tar");
        assert!(tar.success(), "tar the stub release");
        let sum = std::process::Command::new("sha256sum").arg(&served).output().expect("sha256sum");
        let sha256 = String::from_utf8(sum.stdout).unwrap().split(' ').next().unwrap().to_string();
        let cache = dir.join("cache");
        // ps under the C locale shows a byte outside ASCII as `?`, as over an SSH connection that sets no locale.
        let vars = environment(Some(&bin), &[("XDG_CACHE_HOME", &cache), ("LC_ALL", "C")]);
        let repo = Repo::with_host(base_files, ProcessHost::in_environment(vars.into_iter(), &Interrupt::new()));
        Bench { dir, repo, sha256 }
    }

    /// A bench whose base pins the stub release as version 1.2.3.
    fn pinned() -> Bench {
        let mut bench = Bench::new(&[("README", "base\n")]);
        let pin = format!("# the gate\n1.2.3 {}\n", bench.sha256);
        bench.repo.base = Some(bench.repo.commit(&[("ci/gate.pin", &pin)], "pin"));
        bench
    }

    /// start judging a snapshot of the repository's working tree on its base, into <dir>/out.
    fn start(&self) -> Result<Judging, OrchestratorError> {
        let (host, cwd, base) = (&self.repo.host, &self.repo.path, self.repo.base());
        start(host, cwd, &host.snapshot(cwd, base, "snapshot").unwrap(), base, &self.out())
    }

    /// What judging comes to, asked after until it ends.
    fn wait(&self, judging: &Judging) -> Result<LocalVerdict, OrchestratorError> {
        for _ in 0..600 {
            if let Some(verdict) = judging.verdict(&self.repo.host)? {
                return Ok(verdict);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the local gate did not end within 30 s");
    }

    /// start, then wait.
    fn judge(&self) -> Result<LocalVerdict, OrchestratorError> {
        self.wait(&self.start()?)
    }

    /// The round's directory.
    fn out(&self) -> String {
        self.dir.join("out")
    }

    /// The job directory of the round's kth start.
    fn job(&self, k: u32) -> String {
        format!("{}/job-{k}", self.out())
    }

    /// The URLs curl was asked for, in order.
    fn downloads(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("curl-log")).unwrap_or_default().lines().map(String::from).collect()
    }

    /// The gate's working directory, then its arguments, of its last run.
    fn gate_args(&self) -> Vec<String> {
        read(&self.dir.join("gate-args")).lines().map(String::from).collect()
    }

    fn gate_exits(&self, status: u8) {
        put(&self.dir.join("exit"), &status.to_string());
    }

    /// The names in the gate cache.
    fn cached(&self) -> Vec<String> {
        let names = std::fs::read_dir(self.dir.join("cache/octoflow/gate")).into_iter().flatten();
        names.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    /// Whether any process of judging's group is left.
    fn running(&self, judging: &Judging) -> bool {
        judging.running(&self.repo.host).unwrap()
    }

    /// Wait until the stub gate has started, which it records in gate-args.
    fn until_the_gate_runs(&self) {
        let args = self.dir.join("gate-args");
        for _ in 0..600 {
            if std::path::Path::new(&args).exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the stub gate did not start within 30 s");
    }
}

#[test]
fn a_pin_is_its_first_line_not_a_comment() {
    let sha = "a".repeat(64);
    assert_eq!(parse_pin(&format!("# c\n1.2.3 {sha}\n0.1 {sha}\n")).unwrap(), ("1.2.3".to_string(), sha.clone()));
    assert_eq!(parse_pin(BUILT_IN_PIN).unwrap().0, "0.5.0");
    for bad in [
        String::new(),
        "# only a comment\n".to_string(),
        "1.2.3\n".to_string(),
        format!("1.2.3 {}", &sha[1..]),
        format!("1.2.3 {}", sha.to_uppercase()),
        format!("1.2/3 {sha}"),
        format!(" {sha}"),
        format!("1.2.3 {sha} trailing"),
    ] {
        let e = parse_pin(&bad).unwrap_err().to_string();
        assert!(e.contains("is not '<version> <sha256>'"), "{bad:?}: {e}");
    }
}

#[test]
fn the_gate_judges_a_checkout_of_the_snapshot_by_the_bases_settings_and_both_reports_are_read() {
    let mut bench = Bench::pinned();
    let base = bench.repo.commit(&[("gate.toml", "[gate]\nstrict = true\n"), ("pyproject.toml", "[tool.x]\n")], "settings");
    bench.repo.base = Some(base.clone());
    // The change loosens the gate's settings in its working tree, which is not what judges it, and adds a file it
    // has not told git about, which is judged.
    bench.repo.put("gate.toml", "");
    bench.repo.put("pyproject.toml", "");
    bench.repo.put("new.py", "x = 1\n");
    bench.gate_exits(1);

    let verdict = bench.judge().unwrap();

    let want = LocalVerdict::Judged { release: "1.2.3".into(), quality: QUALITY.into(), json: JSON.into() };
    assert_eq!(verdict, want);
    let out = bench.job(1);
    let resolved = std::fs::canonicalize(&out).unwrap().display().to_string();
    let args = bench.gate_args();
    assert_eq!(args[0], format!("{resolved}/tree"));
    let mut want = vec![
        "--base".to_string(),
        base,
        "--config".into(),
        format!("{out}/judge/pyproject.toml"),
        "--out".into(),
        format!("{out}/quality.md"),
    ];
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        want.extend(["--rust-syntax".into(), format!("{out}/release/gate-rust-linux-x86_64")]);
    }
    assert_eq!(args[1..], want);
    assert!(read(&bench.dir.join("gate-files")).lines().any(|name| name == "new.py"));
    assert_eq!(read(&format!("{out}/judge/gate.toml")), "[gate]\nstrict = true\n");
    assert_eq!(read(&format!("{out}/judge/pyproject.toml")), "[tool.x]\n");
    // What the gate wrote stayed in its checkout, which is gone with the release, and the run's worktree is as the
    // Builder left it.
    for gone in ["group", "tree", "release", "download.tar.gz"] {
        assert!(!std::path::Path::new(&format!("{out}/{gone}")).exists(), "{gone}");
    }
    assert!(!std::path::Path::new(&format!("{}/coverage.xml", bench.repo.path)).exists());
    let url = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v1.2.3/octoflow-gate-1.2.3.tar.gz";
    assert_eq!(bench.downloads(), [url]);
    assert_eq!(bench.cached(), [format!("octoflow-gate-1.2.3-{}.tar.gz", bench.sha256)]);

    // A gate that passes is read as one that fails, and the release is fetched once per pin.
    bench.gate_exits(0);
    assert!(matches!(bench.judge().unwrap(), LocalVerdict::Judged { .. }));
    assert_eq!(bench.downloads().len(), 1);
}

#[test]
fn a_cached_release_that_no_longer_matches_the_pin_is_fetched_again() {
    let bench = Bench::pinned();
    bench.judge().unwrap();
    let cached = bench.dir.join(&format!("cache/octoflow/gate/octoflow-gate-1.2.3-{}.tar.gz", bench.sha256));
    std::fs::write(&cached, "edited").unwrap();

    assert!(matches!(bench.judge().unwrap(), LocalVerdict::Judged { .. }));

    assert_eq!(bench.downloads().len(), 2);
    let sum = std::process::Command::new("sha256sum").arg(&cached).output().expect("sha256sum");
    assert!(String::from_utf8(sum.stdout).unwrap().starts_with(&bench.sha256));
}

#[test]
fn a_base_without_a_pyproject_toml_or_a_gate_toml_is_judged_by_empty_rules() {
    let bench = Bench::pinned();
    // A pyproject.toml the change adds does not judge it.
    bench.repo.put("pyproject.toml", "[tool.ruff]\nselect = []\n");
    bench.judge().unwrap();
    let out = bench.job(1);
    assert_eq!(bench.gate_args()[4], format!("{out}/judge/pyproject.toml"));
    assert_eq!(read(&format!("{out}/judge/pyproject.toml")), "");
    assert!(!std::path::Path::new(&format!("{out}/judge/gate.toml")).exists());
}

#[test]
fn a_gate_that_cannot_judge_says_why() {
    let bench = Bench::pinned();
    bench.gate_exits(2);
    assert_eq!(bench.judge().unwrap(), LocalVerdict::CouldNotJudge(CANNOT_JUDGE.into()));
}

#[test]
fn an_exit_status_2_that_is_not_the_gates_own_is_an_error() {
    let bench = Bench::pinned();
    bench.gate_exits(2);
    put(&bench.dir.join("said"), "usage: gate.py [-h]\ngate.py: error: unrecognized arguments: --rust-syntax\n");
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.starts_with("octoflow-gate 1.2.3 exited 2: usage: gate.py"), "{e}");

    put(&bench.dir.join("said"), "python3: can't open file 'gate.py': [Errno 2] No such file or directory\n");
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.starts_with("octoflow-gate 1.2.3 exited 2: python3: can't open"), "{e}");
}

#[test]
fn a_gate_that_judged_without_its_reports_is_an_error_not_the_last_judgement() {
    let bench = Bench::pinned();
    bench.judge().unwrap();
    put(&bench.dir.join("silent"), "");
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.contains("octoflow-gate 1.2.3 judged but wrote no ") && e.contains("/out/job-2/quality.md"), "{e}");
}

#[test]
fn any_other_exit_status_is_an_error_naming_it() {
    let bench = Bench::pinned();
    bench.gate_exits(3);
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.starts_with("octoflow-gate 1.2.3 exited 3"), "{e}");
}

#[test]
fn a_tarball_that_does_not_match_the_pin_is_never_unpacked_or_cached() {
    let bench = Bench::new(&[("ci/gate.pin", &format!("1.2.3 {}\n", "0".repeat(64)))]);
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.starts_with("fetching octoflow-gate 1.2.3: "), "{e}");
    assert!(e.contains(&format!("has sha256 {}, not the pinned", bench.sha256)), "{e}");
    assert!(bench.cached().is_empty(), "{:?}", bench.cached());
    assert!(!std::path::Path::new(&format!("{}/download.tar.gz", bench.job(1))).exists());
}

#[test]
fn a_base_without_a_pin_is_judged_by_the_built_in_one() {
    let bench = Bench::new(&[("README", "base\n")]);
    // The stub is not the built-in release, so the fetch fails, having asked for it.
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.starts_with("fetching octoflow-gate 0.5.0: "), "{e}");
    let url = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v0.5.0/octoflow-gate-0.5.0.tar.gz";
    assert_eq!(bench.downloads(), [url]);
}

#[test]
fn a_bad_pin_names_the_file() {
    let bench = Bench::new(&[("ci/gate.pin", "latest\n")]);
    let e = bench.start().unwrap_err().to_string();
    assert_eq!(e, "the run's base's ci/gate.pin: its first line 'latest' is not '<version> <sha256>'");
    assert!(bench.downloads().is_empty());
}

#[test]
fn a_stopped_gate_is_gone_with_its_checkout() {
    let bench = Bench::pinned();
    put(&bench.dir.join("hang"), "");
    let judging = bench.start().unwrap();
    bench.until_the_gate_runs();
    assert_eq!(judging.verdict(&bench.repo.host).unwrap(), None);

    judging.stop(&bench.repo.host).unwrap();

    assert!(!bench.running(&judging));
    assert!(!std::path::Path::new(&format!("{}/tree", bench.job(1))).exists());
}

#[test]
fn a_start_stops_the_job_an_earlier_one_left_and_judges_afresh() {
    let bench = Bench::pinned();
    put(&bench.dir.join("hang"), "");
    let left = bench.start().unwrap();
    bench.until_the_gate_runs();
    std::fs::remove_file(bench.dir.join("hang")).unwrap();

    let verdict = bench.judge().unwrap();

    assert!(!bench.running(&left));
    assert!(matches!(verdict, LocalVerdict::Judged { .. }));
}

#[test]
fn what_a_replaced_start_does_to_its_own_files_leaves_the_new_job_alone() {
    let bench = Bench::pinned();
    put(&bench.dir.join("hang"), "");
    let replaced = bench.start().unwrap();
    bench.until_the_gate_runs();
    let taking_over = bench.start().unwrap();

    // The engine taken over finds its job gone and stops it, its clean-up coming after the new start, as it may when
    // the two engines' stops overlap.
    let host = &bench.repo.host;
    assert!(replaced.verdict(host).is_err());
    clean_up(host, &bench.repo.path, &replaced.dir).unwrap();
    replaced.stop(host).unwrap();

    assert!(bench.running(&taking_over));
    assert!(std::path::Path::new(&format!("{}/tree", bench.job(2))).exists());
    taking_over.stop(host).unwrap();
}

#[test]
fn a_gate_whose_jobs_leader_has_died_is_still_stopped() {
    let bench = Bench::pinned();
    put(&bench.dir.join("hang"), "");
    let judging = bench.start().unwrap();
    bench.until_the_gate_runs();
    let leader = |args: &[&str]| std::process::Command::new(args[0]).args(&args[1..]).status().expect(args[0]).success();
    assert!(leader(&["kill", "-KILL", &judging.group]));
    for _ in 0..100 {
        if !leader(&["sh", "-c", "ps -p \"$1\" >/dev/null", "_", &judging.group]) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(bench.running(&judging), "the gate outlives its job's leader");

    judging.stop(&bench.repo.host).unwrap();

    assert!(!bench.running(&judging));
}

#[test]
fn a_gate_whose_directorys_path_is_not_ascii_is_stopped() {
    let bench = Bench::pinned();
    put(&bench.dir.join("hang"), "");
    let (host, cwd, base) = (&bench.repo.host, &bench.repo.path, bench.repo.base());
    let out = bench.dir.join("jürgen/out");
    let judging = start(host, cwd, &host.snapshot(cwd, base, "snapshot").unwrap(), base, &out).unwrap();
    bench.until_the_gate_runs();

    judging.stop(host).unwrap();

    assert!(!bench.running(&judging));
}

#[test]
fn a_start_signals_no_group_the_job_a_crash_left_no_longer_is() {
    use std::os::unix::process::CommandExt;
    let bench = Bench::pinned();
    // The crashed job's pid, since given to a session leader of something else.
    let mut other = std::process::Command::new("sleep").arg("30").process_group(0).spawn().expect("sleep");
    put(&format!("{}/group", bench.job(1)), &other.id().to_string());

    let verdict = bench.judge();
    let alive = other.try_wait().unwrap().is_none();
    other.kill().unwrap();
    other.wait().unwrap();

    assert!(matches!(verdict.unwrap(), LocalVerdict::Judged { .. }));
    assert!(alive, "the other process group was signalled");
}

#[test]
fn a_judgement_leaves_the_repositorys_other_worktrees_registered() {
    let bench = Bench::pinned();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(&bench.repo.path).args(args).output().expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    // A worktree on a disk that is not mounted now, which only a prune would forget.
    let elsewhere = bench.dir.join("elsewhere");
    git(&["worktree", "add", "--detach", &elsewhere, "HEAD"]);
    std::fs::rename(&elsewhere, bench.dir.join("unmounted")).unwrap();

    bench.judge().unwrap();

    assert!(git(&["worktree", "list", "--porcelain"]).contains("/elsewhere\n"));
}

#[test]
fn the_preflight_needs_python3_3_11_the_fetchs_tools_a_full_ps_and_a_writable_cache() {
    let bench = Bench::pinned();
    preflight(&bench.repo.host).unwrap();
    assert!(std::path::Path::new(&bench.dir.join("cache/octoflow/gate")).is_dir());

    let host = |vars: Vec<_>| ProcessHost::in_environment(vars.into_iter(), &Interrupt::new());
    let empty = TempDir::new();
    let e = preflight(&host(environment(None, &[("PATH", &empty.path())]))).unwrap_err().to_string();
    let python = "the local quality gate runs octoflow-gate with python3 3.11 or later from PATH, and ";
    assert!(e.starts_with(python), "{e}");

    put(&bench.dir.join("old"), "");
    let e = preflight(&bench.repo.host).unwrap_err().to_string();
    assert_eq!(e, format!("{python}it is older"));
    std::fs::remove_file(bench.dir.join("old")).unwrap();

    // A PATH with sh and python3 alone has no curl.
    let only = bench.dir.join("only");
    std::fs::create_dir_all(&only).unwrap();
    let sh = std::process::Command::new("sh").args(["-c", "command -v sh"]).output().expect("sh");
    std::os::unix::fs::symlink(String::from_utf8(sh.stdout).unwrap().trim(), format!("{only}/sh")).unwrap();
    std::os::unix::fs::symlink(bench.dir.join("bin/python3"), format!("{only}/python3")).unwrap();
    let e = preflight(&host(environment(None, &[("PATH", &only)]))).unwrap_err().to_string();
    assert_eq!(e, "the local quality gate fetches octoflow-gate into a cache of its own: curl is not on PATH");

    // A ps that does not name a process's command line, as BusyBox's does not, cannot tell the job from another.
    let busybox = bench.dir.join("busybox");
    tool(&busybox, "ps", "#!/bin/sh\necho 'ps: unrecognized option: w' >&2\nexit 1\n");
    let cache = bench.dir.join("cache");
    let e = preflight(&host(environment(Some(&busybox), &[("XDG_CACHE_HOME", &cache)]))).unwrap_err().to_string();
    assert_eq!(
        e,
        "the local quality gate fetches octoflow-gate into a cache of its own: ps does not take -ww -o args= -p, which \
         stopping the gate needs"
    );

    // A cache under a file cannot be made.
    let file = bench.dir.join("file");
    put(&file, "");
    let e = preflight(&host(environment(Some(&bench.dir.join("bin")), &[("XDG_CACHE_HOME", &file)])))
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("the local quality gate fetches octoflow-gate into a cache of its own: "), "{e}");
    assert!(e.contains("cannot write the cache"), "{e}");
}
