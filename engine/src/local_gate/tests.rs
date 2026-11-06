//! local_gate on this machine: a real git repository for the base's files, real sh, sha256sum and tar, with curl
//! serving a stub release from a file and python3 running the stub's gate.py with sh.

use super::*;
use crate::host::tests::{environment, put, read, tool, Repo};
use crate::host::ProcessHost;
use crate::interrupt::Interrupt;
use crate::test_support::TempDir;

/// The quality file and quality.json the stub gate writes when it judges.
const QUALITY: &str = "GATE: ERROR\n\n## Issues on changed lines\n\n1. `a.sh:1` warning shellcheck SC2086\n";
const JSON: &str = "{\"schema_version\": 1, \"gate\": \"ERROR\"}";

/// A machine with the stub release served, a repository whose base commit holds base_files, and a cache of its own.
struct Bench {
    dir: TempDir,
    repo: Repo,
    /// The served tarball's sha256, as a pin names it.
    sha256: String,
}

impl Bench {
    /// The stub release is a gate.py that records its working directory and arguments in <dir>/gate-args, then exits
    /// as <dir>/exit says (0 when it is missing): with 2 it says why on standard error and writes nothing; with 0 or 1
    /// it writes QUALITY to --out and JSON beside it, unless <dir>/silent exists.
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
        tool(&bin, "python3", "#!/bin/sh\n[ \"$1\" = --version ] && { echo 'Python 3.12.0'; exit 0; }\nexec sh \"$@\"\n");
        put(
            &format!("{rel}/gate.py"),
            &format!(
                "pwd > '{args}'; printf '%s\\n' \"$@\" >> '{args}'\n\
                 status=$(cat '{exit}' 2>/dev/null || echo 0)\n\
                 [ \"$status\" = 2 ] && {{ echo 'gate.toml: unknown key \"x\"' >&2; exit 2; }}\n\
                 while [ $# -gt 0 ]; do [ \"$1\" = --out ] && out=$2; shift; done\n\
                 [ -e '{silent}' ] || {{ printf '%s' '{QUALITY}' > \"$out\"; printf '%s' '{JSON}' > \"${{out%.md}}.json\"; }}\n\
                 exit \"$status\"\n",
                args = dir.join("gate-args"),
                exit = dir.join("exit"),
                silent = dir.join("silent"),
            ),
        );
        put(&format!("{rel}/gate-rust-linux-x86_64"), "");
        let tar = std::process::Command::new("tar").args(["-czf", &served, "-C", &rel, "."]).status().expect("tar");
        assert!(tar.success(), "tar the stub release");
        let sum = std::process::Command::new("sha256sum").arg(&served).output().expect("sha256sum");
        let sha256 = String::from_utf8(sum.stdout).unwrap().split(' ').next().unwrap().to_string();
        let cache = dir.join("cache");
        let vars = environment(Some(&bin), &[("XDG_CACHE_HOME", &cache)]);
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

    /// judge the repository's working tree since its base, into <dir>/out.
    fn judge(&self) -> Result<LocalVerdict, OrchestratorError> {
        judge(&self.repo.host, &self.repo.path, self.repo.base(), &self.out(), 60)
    }

    fn out(&self) -> String {
        self.dir.join("out")
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

    /// The cache's directory for release version.
    fn cached(&self, version: &str) -> String {
        self.dir.join(&format!("cache/octoflow/gate/{version}"))
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
fn the_gate_judges_the_working_tree_by_the_bases_settings_and_both_reports_are_read() {
    let mut bench = Bench::pinned();
    let base = bench.repo.commit(&[("gate.toml", "[gate]\nstrict = true\n"), ("pyproject.toml", "[tool.x]\n")], "settings");
    bench.repo.base = Some(base.clone());
    // The change loosens the gate's settings in its working tree, which is not what judges it.
    bench.repo.put("gate.toml", "");
    bench.gate_exits(1);

    let verdict = bench.judge().unwrap();

    let want = LocalVerdict::Judged { release: "1.2.3".into(), quality: QUALITY.into(), json: JSON.into() };
    assert_eq!(verdict, want);
    let (out, release) = (bench.out(), bench.cached("1.2.3"));
    let resolved = std::fs::canonicalize(&bench.repo.path).unwrap().display().to_string();
    let args = bench.gate_args();
    assert_eq!(args[0], resolved);
    let want = [
        "--base".to_string(),
        base,
        "--config".into(),
        format!("{out}/judge/pyproject.toml"),
        "--rust-syntax".into(),
        format!("{release}/gate-rust-linux-x86_64"),
        "--out".into(),
        format!("{out}/quality.md"),
    ];
    assert_eq!(args[1..], want);
    assert_eq!(read(&format!("{out}/judge/gate.toml")), "[gate]\nstrict = true\n");
    assert_eq!(read(&format!("{out}/judge/pyproject.toml")), "[tool.x]\n");
    let url = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v1.2.3/octoflow-gate-1.2.3.tar.gz";
    assert_eq!(bench.downloads(), [url]);
    assert_eq!(read(&format!("{release}/.sha256")), format!("{}\n", bench.sha256));

    // A gate that passes is read as one that fails, and the release is fetched once per version.
    bench.gate_exits(0);
    assert!(matches!(bench.judge().unwrap(), LocalVerdict::Judged { .. }));
    assert_eq!(bench.downloads().len(), 1);
}

#[test]
fn without_a_pyproject_toml_the_config_is_the_bases_gate_toml_or_an_empty_one() {
    let bench = Bench::pinned();
    bench.judge().unwrap();
    let out = bench.out();
    assert_eq!(bench.gate_args()[4], format!("{out}/judge/gate.toml"));
    assert_eq!(read(&format!("{out}/judge/gate.toml")), "");
}

#[test]
fn a_gate_that_cannot_judge_says_why() {
    let bench = Bench::pinned();
    bench.gate_exits(2);
    assert_eq!(bench.judge().unwrap(), LocalVerdict::CouldNotJudge("gate.toml: unknown key \"x\"".into()));
}

#[test]
fn a_gate_that_judged_without_its_reports_is_an_error_not_the_last_judgement() {
    let bench = Bench::pinned();
    bench.judge().unwrap();
    put(&bench.dir.join("silent"), "");
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.contains("octoflow-gate 1.2.3 exited 0 but wrote no ") && e.contains("/out/quality.md"), "{e}");
}

#[test]
fn any_other_exit_status_is_an_error_naming_it() {
    let bench = Bench::pinned();
    bench.gate_exits(3);
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.contains("octoflow-gate 1.2.3 in ") && e.contains(" exited 3"), "{e}");
}

#[test]
fn a_tarball_that_does_not_match_the_pin_is_never_unpacked() {
    let bench = Bench::new(&[("ci/gate.pin", &format!("1.2.3 {}\n", "0".repeat(64)))]);
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.starts_with("fetching octoflow-gate 1.2.3: "), "{e}");
    assert!(e.contains(&format!("has sha256 {}, not the pinned", bench.sha256)), "{e}");
    let left: Vec<_> = std::fs::read_dir(bench.dir.join("cache/octoflow/gate")).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_release_cached_against_another_sha256_is_refused() {
    let bench = Bench::pinned();
    bench.judge().unwrap();
    put(&format!("{}/.sha256", bench.cached("1.2.3")), &format!("{}\n", "f".repeat(64)));
    let e = bench.judge().unwrap_err().to_string();
    assert!(e.contains("was checked against another sha256 than the pinned") && e.contains("delete it"), "{e}");
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
    let e = bench.judge().unwrap_err().to_string();
    assert_eq!(e, "the run's base's ci/gate.pin: its first line 'latest' is not '<version> <sha256>'");
    assert!(bench.downloads().is_empty());
}

#[test]
fn the_preflight_needs_python3_and_a_writable_cache() {
    let bench = Bench::pinned();
    preflight(&bench.repo.host).unwrap();
    assert!(std::path::Path::new(&bench.dir.join("cache/octoflow/gate")).is_dir());

    let empty = TempDir::new();
    let vars = environment(None, &[("PATH", &empty.path())]);
    let e = preflight(&ProcessHost::in_environment(vars.into_iter(), &Interrupt::new())).unwrap_err().to_string();
    assert!(e.starts_with("the local quality gate runs octoflow-gate with python3 from PATH, and "), "{e}");

    // A cache under a file cannot be made.
    let file = bench.dir.join("file");
    put(&file, "");
    let vars = environment(Some(&bench.dir.join("bin")), &[("XDG_CACHE_HOME", &file)]);
    let e = preflight(&ProcessHost::in_environment(vars.into_iter(), &Interrupt::new())).unwrap_err().to_string();
    assert!(e.starts_with("the local quality gate keeps its releases in a cache directory: "), "{e}");
}
