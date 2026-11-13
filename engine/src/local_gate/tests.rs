//! local_gate on this machine: a real git repository for the base's files, real sh, sha256sum and tar, with curl
//! serving a stub release from a file, whose gate is a script for sh.

use super::*;
use crate::host::tests::{environment, put, read, tool, FakeSsh, Repo};
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
    /// The stub release is a gate that records its physical working directory and arguments in <dir>/gate-args
    /// and the names in that directory in <dir>/gate-files, writes coverage.xml there, sleeps 30 s when <dir>/hang
    /// exists, then exits as <dir>/exit says (0 when it is missing): with 2 it says <dir>/said, else CANNOT_JUDGE, on
    /// standard error and writes nothing; with 0 or 1 it writes QUALITY to --out and JSON beside it, unless
    /// <dir>/silent exists. When <dir>/hang exists, a SIGINT from the moment gate-args exists on is recorded in
    /// <dir>/interrupted and exits 130. uname names the machine the release's gate is built for, whatever the
    /// tests run on.
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
        tool(&bin, "uname", "#!/bin/sh\necho 'Linux x86_64'\n");
        tool(
            &rel,
            "gate",
            &format!(
                "#!/bin/sh\n\
                 [ -e '{hang}' ] && trap \": > '{interrupted}'; exit 130\" INT\n\
                 pwd -P > '{args}'; printf '%s\\n' \"$@\" >> '{args}'; ls -A > '{files}'; : > coverage.xml\n\
                 [ -e '{hang}' ] && sleep 30\n\
                 status=$(cat '{exit}' 2>/dev/null || echo 0)\n\
                 [ \"$status\" = 2 ] && {{ cat '{said}' >&2 2>/dev/null || echo '{CANNOT_JUDGE}' >&2; exit 2; }}\n\
                 while [ $# -gt 0 ]; do [ \"$1\" = --out ] && out=$2; shift; done\n\
                 [ -e '{silent}' ] || {{ printf '%s' '{QUALITY}' > \"$out\"; printf '%s' '{JSON}' > \"${{out%.md}}.json\"; }}\n\
                 exit \"$status\"\n",
                args = dir.join("gate-args"),
                files = dir.join("gate-files"),
                hang = dir.join("hang"),
                interrupted = dir.join("interrupted"),
                exit = dir.join("exit"),
                said = dir.join("said"),
                silent = dir.join("silent"),
            ),
        );
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

    fn gate_exits(&self, code: u8) {
        put(&self.dir.join("exit"), &code.to_string());
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
    assert_eq!(parse_pin(BUILT_IN_PIN).unwrap().0, "0.10.0");
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
    let (settings, quality) = (format!("{out}/judge/gate.toml"), format!("{out}/quality.md"));
    assert_eq!(args[1..], ["--base", &base, "--config", &settings, "--out", &quality]);
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
fn a_base_without_a_gate_toml_or_a_pyproject_toml_is_judged_by_the_gates_defaults() {
    let bench = Bench::pinned();
    // Neither file judges the change that adds it.
    bench.repo.put("gate.toml", "[budgets]\nmutation = 1\n");
    bench.repo.put("pyproject.toml", "[tool.ruff]\nselect = []\n");
    bench.judge().unwrap();
    let out = bench.job(1);
    assert_eq!(bench.gate_args()[4], format!("{out}/judge/gate.toml"));
    assert_eq!(read(&format!("{out}/judge/gate.toml")), "");
    assert!(!std::path::Path::new(&format!("{out}/judge/pyproject.toml")).exists());
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
    put(&bench.dir.join("said"), "usage: gate --base REV\ngate: error: unrecognized arguments: --out\n");
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.starts_with("octoflow-gate 1.2.3 exited 2: usage: gate --base REV"), "{e}");
}

#[test]
fn a_release_from_before_the_gate_was_one_program_is_refused_before_any_download() {
    let bench = Bench::new(&[("ci/gate.pin", &format!("0.6.0 {}\n", "0".repeat(64)))]);
    let e = bench.start().unwrap_err().to_string();
    let want = "the run's base's ci/gate.pin: octoflow-gate 0.6.0 ships no gate, which the local gate runs from 0.7.0 on";
    assert_eq!(e, want);
    assert!(bench.downloads().is_empty());
    assert!(!std::path::Path::new(&bench.job(1)).exists());
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
    assert!(e.starts_with("fetching octoflow-gate 0.10.0: "), "{e}");
    let url = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v0.10.0/octoflow-gate-0.10.0.tar.gz";
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
fn a_stop_reaches_the_gate_as_a_ctrl_c_would() {
    // The gate stops its tests, in sessions of their own, when it is interrupted: a SIGINT it never got, or ignored,
    // would leave them running once its group is killed.
    let bench = Bench::pinned();
    put(&bench.dir.join("hang"), "");
    let judging = bench.start().unwrap();
    bench.until_the_gate_runs();

    judging.stop(&bench.repo.host).unwrap();

    assert!(std::path::Path::new(&bench.dir.join("interrupted")).exists());
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
fn a_gate_whose_jobs_leader_died_unreaped_is_still_stopped() {
    let bench = Bench::pinned();
    put(&bench.dir.join("hang"), "");
    let judging = bench.start().unwrap();
    bench.until_the_gate_runs();
    // Under a PID 1 that reaps no orphans the killed leader stays listed as a zombie; ps says so for it here.
    let real = std::process::Command::new("sh").args(["-c", "command -v ps"]).output().expect("ps");
    let real = String::from_utf8(real.stdout).unwrap().trim().to_string();
    let killed = std::process::Command::new("kill").args(["-KILL", &judging.group]).status().expect("kill");
    assert!(killed.success());
    tool(
        &bench.dir.join("bin"),
        "ps",
        &format!(
            "#!/bin/sh\nfor a; do [ \"$a\" = '{g}' ] && {{ echo 'Z    [sh] <defunct>'; exit 0; }}; done\n\
             exec '{real}' \"$@\"\n",
            g = judging.group,
        ),
    );
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
fn the_preflight_needs_linux_on_x86_64_the_jobs_tools_a_full_ps_an_env_that_restores_signals_and_a_writable_cache() {
    let bench = Bench::pinned();
    preflight(&bench.repo.host).unwrap();
    assert!(std::path::Path::new(&bench.dir.join("cache/octoflow/gate")).is_dir());

    let host = |vars: Vec<_>| ProcessHost::in_environment(vars.into_iter(), &Interrupt::new());
    let fails = "the local quality gate fetches octoflow-gate and runs it on the project's machine: ";

    // The release's gate does not run on a Mac.
    let mac = bench.dir.join("mac");
    tool(&mac, "uname", "#!/bin/sh\necho 'Darwin arm64'\n");
    let e = preflight(&host(environment(Some(&mac), &[]))).unwrap_err().to_string();
    assert_eq!(e, format!("{fails}the gate is built for Linux x86_64, and this machine is Darwin arm64"));

    // A PATH with sh and uname alone has no curl.
    let only = bench.dir.join("only");
    std::fs::create_dir_all(&only).unwrap();
    let sh = std::process::Command::new("sh").args(["-c", "command -v sh"]).output().expect("sh");
    std::os::unix::fs::symlink(String::from_utf8(sh.stdout).unwrap().trim(), format!("{only}/sh")).unwrap();
    std::os::unix::fs::symlink(bench.dir.join("bin/uname"), format!("{only}/uname")).unwrap();
    let e = preflight(&host(environment(None, &[("PATH", &only)]))).unwrap_err().to_string();
    assert_eq!(e, format!("{fails}curl is not on PATH"));

    // A ps that does not name a process's command line, as BusyBox's does not, cannot tell the job from another.
    let busybox = bench.dir.join("busybox");
    tool(&busybox, "ps", "#!/bin/sh\necho 'ps: unrecognized option: w' >&2\nexit 1\n");
    let cache = bench.dir.join("cache");
    let e = preflight(&host(environment(Some(&busybox), &[("XDG_CACHE_HOME", &cache)]))).unwrap_err().to_string();
    assert_eq!(e, format!("{fails}ps does not take -ww -o stat=,args= -p, which stopping the gate needs"));

    // An env that cannot restore SIGINT, as BusyBox's, would start the gate deaf to a stop (LAUNCH).
    let old_env = bench.dir.join("old-env");
    tool(&old_env, "env", "#!/bin/sh\necho \"env: unrecognized option '$1'\" >&2\nexit 125\n");
    let e = preflight(&host(environment(Some(&old_env), &[("XDG_CACHE_HOME", &cache)]))).unwrap_err().to_string();
    assert_eq!(e, format!("{fails}env does not take --default-signal, which starting the gate needs"));

    // A cache under a file cannot be made.
    let file = bench.dir.join("file");
    put(&file, "");
    let e = preflight(&host(environment(Some(&bench.dir.join("bin")), &[("XDG_CACHE_HOME", &file)])))
        .unwrap_err()
        .to_string();
    assert!(e.starts_with(fails) && e.contains("cannot write the cache"), "{e}");
}

#[test]
fn set_up_writes_the_built_in_pin_and_starter_settings_at_the_top_level_uncommitted() {
    let pyproject = "[tool.pytest.ini_options]\n";
    let repo = Repo::new(&[("Cargo.toml", "[package]\n"), ("pyproject.toml", pyproject), ("sub/README", "x\n")]);
    let files = set_up(&repo.host, &format!("{}/sub", repo.path)).unwrap();
    let written = |path| SetUpFile { path, written: true };
    assert_eq!(files, [written("ci/gate.pin"), written("gate.toml")]);
    assert_eq!(read(&format!("{}/ci/gate.pin", repo.path)), format!("{BUILT_IN_PIN}\n"));
    let settings = read(&format!("{}/gate.toml", repo.path));
    assert_eq!(settings, starter_settings(&["Cargo.toml"], Some(pyproject)));
    assert_eq!(tests_of(&settings).len(), 2, "{settings}");
    assert_eq!(repo.git(&["status", "--porcelain"]), "?? ci/\n?? gate.toml\n");
}

#[test]
fn set_up_keeps_a_file_already_there() {
    let repo = Repo::new(&[("gate.toml", "# mine\n")]);
    let files = set_up(&repo.host, &repo.path).unwrap();
    assert_eq!(files, [SetUpFile { path: "ci/gate.pin", written: true }, SetUpFile { path: "gate.toml", written: false }]);
    assert_eq!(read(&format!("{}/gate.toml", repo.path)), "# mine\n");
    repo.put("ci/gate.pin", "0.4.0 mine\n");
    std::fs::remove_file(format!("{}/gate.toml", repo.path)).unwrap();
    let files = set_up(&repo.host, &repo.path).unwrap();
    assert_eq!(files, [SetUpFile { path: "ci/gate.pin", written: false }, SetUpFile { path: "gate.toml", written: true }]);
    assert_eq!(read(&format!("{}/ci/gate.pin", repo.path)), "0.4.0 mine\n");
}

#[test]
fn set_up_keeps_a_file_made_after_it_looked() {
    let repo = Repo::new(&[("Cargo.toml", "[package]\n")]);
    // An ln and an mv that make gate.toml, as a second set_up would, just before they put theirs there.
    let bin = TempDir::new();
    for name in ["ln", "mv"] {
        tool(
            &bin.path(),
            name,
            &format!(
                "#!/bin/sh\neval \"last=\\${{$#}}\"\ncase $last in *gate.toml) printf '# mine\\n' > \"$last\";; esac\n\
                 for s in /usr/bin/{name} /bin/{name}; do [ -x \"$s\" ] && exec \"$s\" \"$@\"; done\nexit 127\n"
            ),
        );
    }
    let host = ProcessHost::in_environment(environment(Some(&bin.path()), &[]).into_iter(), &Interrupt::new());
    let files = set_up(&host, &repo.path).unwrap();
    assert_eq!(files, [SetUpFile { path: "ci/gate.pin", written: true }, SetUpFile { path: "gate.toml", written: false }]);
    assert_eq!(read(&format!("{}/gate.toml", repo.path)), "# mine\n");
    assert_eq!(repo.git(&["status", "--porcelain"]), "?? ci/\n?? gate.toml\n");
}

#[test]
fn set_up_keeps_a_directory_made_after_it_looked_and_leaves_nothing_in_it() {
    let repo = Repo::new(&[("Cargo.toml", "[package]\n")]);
    // An ln that finds a directory at gate.toml, which it would make its link in.
    let bin = TempDir::new();
    tool(
        &bin.path(),
        "ln",
        "#!/bin/sh\neval \"last=\\${$#}\"\ncase $last in *gate.toml) mkdir -- \"$last\" || exit 1;; esac\n\
         for s in /usr/bin/ln /bin/ln; do [ -x \"$s\" ] && exec \"$s\" \"$@\"; done\nexit 127\n",
    );
    let host = ProcessHost::in_environment(environment(Some(&bin.path()), &[]).into_iter(), &Interrupt::new());
    let files = set_up(&host, &repo.path).unwrap();
    assert_eq!(files, [SetUpFile { path: "ci/gate.pin", written: true }, SetUpFile { path: "gate.toml", written: false }]);
    let left: Vec<_> = std::fs::read_dir(format!("{}/gate.toml", repo.path)).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn set_up_is_two_commands_over_ssh_from_a_directory_under_home() {
    let home = TempDir::new();
    let ssh = FakeSsh::new(&[("HOME", &home.path())]);
    let repo = Repo::with_host(&[("go.mod", "module m\n"), ("pyproject.toml", "[tool.pytest]\n")], ssh.host.clone());
    std::os::unix::fs::symlink(&repo.path, home.join("proj")).unwrap();
    let before = ssh.calls().len();
    let files = set_up(&ssh.host, "~/proj").unwrap();
    assert_eq!(ssh.calls().len() - before, 2);
    assert!(files.iter().all(|f| f.written), "{files:?}");
    let settings = read(&format!("{}/gate.toml", repo.path));
    assert_eq!(settings, starter_settings(&["go.mod"], Some("[tool.pytest]\n")));
}

#[test]
fn set_up_needs_a_checkout() {
    let repo = Repo::new(&[]);
    let e = set_up(&repo.host, &repo.outside).unwrap_err().to_string();
    assert!(e.starts_with(&format!("looking at {}: ", repo.outside)) && e.contains("not a git repository"), "{e}");
    assert!(set_up(&repo.host, &format!("{}/gone", repo.outside)).is_err());
    assert!(!std::path::Path::new(&format!("{}/gate.toml", repo.outside)).exists());
}

/// A [[test]] table as the gate reads it: (name, command, junit).
type TestTable<S, L> = (S, L, Option<L>);

/// The [[test]] tables of a starter gate.toml.
fn tests_of(settings: &str) -> Vec<TestTable<String, Vec<String>>> {
    let table: toml::Table = toml::from_str(settings).unwrap_or_else(|e| panic!("{e}\n{settings}"));
    let Some(tests) = table.get("test") else { return vec![] };
    let strings = |v: &toml::Value| -> Vec<String> {
        v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect()
    };
    tests
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let t = t.as_table().unwrap();
            let keys: Vec<&str> = t.keys().map(String::as_str).collect();
            assert!(keys.iter().all(|k| ["name", "command", "junit", "coverage"].contains(k)), "{keys:?}");
            (t["name"].as_str().unwrap().to_string(), strings(&t["command"]), t.get("junit").map(strings))
        })
        .collect()
}

/// Assert that the starter gate.toml for a top level holding present, and pyproject.toml's text, has the [[test]]s
/// of want and asks for them to be checked.
fn assert_guessed(present: &[&str], pyproject: Option<&str>, want: &[TestTable<&str, &str>]) {
    let argv = |words: &str| words.split(' ').map(String::from).collect::<Vec<_>>();
    let settings = starter_settings(present, pyproject);
    let want: Vec<_> = want.iter().map(|(n, c, j)| (n.to_string(), argv(c), j.map(|j| vec![j.to_string()]))).collect();
    assert_eq!(tests_of(&settings), want, "{present:?} {pyproject:?}");
    assert!(settings.contains("check that each command runs this project's tests"), "{settings}");
}

#[test]
fn starter_settings_guess_the_tests_from_the_projects_kind() {
    let cargo = ("cargo-test", "cargo test --workspace", None);
    let pytest = ("pytest", "python3 -m pytest --junitxml=junit.xml", Some("junit.xml"));
    assert_guessed(&["Cargo.toml"], None, &[cargo]);
    assert_guessed(&["pytest.ini"], None, &[pytest]);
    let uv_pytest = ("pytest", "uv run pytest --junitxml=junit.xml", Some("junit.xml"));
    assert_guessed(&["uv.lock"], Some("[project]\n\n[tool.pytest.ini_options]\n"), &[uv_pytest]);
    assert_guessed(&["go.mod", "package.json"], None, &[("npm-test", "npm test", None), ("go-test", "go test ./...", None)]);
    assert_guessed(&["Cargo.toml", "conftest.py"], None, &[cargo, pytest]);
}

#[test]
fn a_go_project_s_starter_settings_name_golangci_lint() {
    // The gate fails every round of a project that tracks Go files and lacks golangci-lint (octoflow-gate 0.9.0).
    let go = starter_settings(&["go.mod"], None);
    assert!(go.contains("golangci-lint, which must be on PATH"), "{go}");
    assert!(go.contains("# [linters]\n# golangci-lint = []\n"), "{go}");
    assert_eq!(tests_of(&go), [("go-test".to_string(), vec!["go".into(), "test".into(), "./...".into()], None)]);
    assert!(!starter_settings(&["Cargo.toml"], None).contains("golangci-lint"));
}

#[test]
fn a_pyproject_toml_without_pytest_settings_is_no_python_tests() {
    // As this repository's: it configures tools, and pytest would collect nothing.
    let tools = "[tool.pymarkdown]\nplugins.md013.enabled = false\n";
    assert_guessed(&["Cargo.toml", "uv.lock"], Some(tools), &[("cargo-test", "cargo test --workspace", None)]);
}

#[test]
fn starter_settings_without_a_known_kind_say_to_add_the_tests() {
    let settings = starter_settings(&["uv.lock"], Some("[project]\nname = \"p\"\n"));
    assert_eq!(tests_of(&settings), []);
    assert!(settings.contains("Add a [[test]] for each command that runs them"), "{settings}");
}
