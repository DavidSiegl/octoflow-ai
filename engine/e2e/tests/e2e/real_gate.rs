//! The scenario real_gate: a run with `--quality-gate local` judged by a real octoflow-gate release, downloaded for
//! real, rather than by the fake gate the recorded scenarios use. It checks that the engine can drive the release:
//! fetch it, start its job, pass it its flags, stop it, and read its quality.json. It asserts what the engine made of
//! the release's output, never the release's wording, so a release that only rewords passes; and it is no recording,
//! as a release's report differs from one release to the next.
//!
//! It needs the network and the gate's platform, Linux x86_64, so it runs only under `OCTOFLOW_REAL_GATE=1`, which
//! Jenkins's Test stage sets; elsewhere it passes at once, saying why on standard error. The release is the one the
//! engine has built in (BUILT_IN_PIN), kept in `$OCTOFLOW_GATE_CACHE/octoflow/gate/`, else in the cache the
//! developer's own octoflow uses, `${XDG_CACHE_HOME:-~/.cache}/octoflow/gate/`, so that it is downloaded once.
//! `OCTOFLOW_GATE_TARBALL=<dir>/octoflow-gate-<version>.tar.gz` judges by that tarball instead, a release candidate
//! not published yet: the project pins it, and the world's own cache holds it, so nothing downloads it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use octoflow_e2e::{engine, procs, run_dir, str_at};
use octoflow_engine::local_gate::{parse_pin, BUILT_IN_PIN};

use crate::scenarios::{approved, writes, Action};
use crate::world::World;

const TASK: &str = "Write the answer";
const SPEC: &str = "# Write the answer\n\nWrite `answer.txt`, holding 42.\n";

/// The project's settings: one [[test]], which needs no tool beyond sh, so the gate judges without the linters a
/// language would need. Its first run in the world holds, until stopped, after writing its pid where the scenario
/// looks (HELD_PID); every later run writes a JUnit report whose one test fails unless answer.txt holds 42.
const GATE_TOML: &str = r#"[[test]]
name = "answer"
command = ["sh", "-c", '''
held=$HOME/gate-test-held
if mkdir "$held" 2>/dev/null; then
    echo $$ > "$held/pid.tmp" && mv "$held/pid.tmp" "$held/pid"
    exec sleep 600
fi
if [ "$(cat answer.txt 2>/dev/null)" = 42 ]; then failure=''; else failure='<failure message="not 42"/>'; fi
case='<testcase classname="answer" name="holds_42">'$failure'</testcase>'
printf '<testsuites><testsuite name="answer" tests="1">%s</testsuite></testsuites>\n' "$case" > junit.xml
''']
junit = ["junit.xml"]
"#;

/// Where the held test writes its pid, relative to the world's directory: in the engine's HOME, which the gate's
/// tests inherit.
const HELD_PID: &str = "home/gate-test-held/pid";

/// The Builder's report of quality round q (0 for its first), having written answer to answer.txt.
fn answered(q: u32, answer: &str) -> Action {
    let file = if q == 0 { "build-1.md".to_string() } else { format!("build-1-q{q}.md") };
    writes(&file, &format!("Wrote {answer} to answer.txt.\n")).edit(&[("answer.txt", &format!("{answer}\n"))])
}

/// A run judged by the real release: its first quality round is stopped by a kill of the engine while the gate's test
/// runs, and resumed, which ends that round's job; the round then fails on the test, the Builder answers it, and the
/// second round passes, so the run publishes.
#[test]
fn real_gate() {
    if std::env::var("OCTOFLOW_REAL_GATE").as_deref() != Ok("1") {
        eprintln!(
            "real_gate skipped: it downloads an octoflow-gate release and runs it, which needs the network and Linux \
             x86_64; OCTOFLOW_REAL_GATE=1 runs it"
        );
        return;
    }
    let files = [(".gitignore", "junit.xml\n"), ("gate.toml", GATE_TOML)];
    let mut w = World::with_project("real_gate", &engine::binary(), &files);
    let (cache, release) = match std::env::var_os("OCTOFLOW_GATE_TARBALL") {
        Some(tarball) => candidate_pinned(&w, Path::new(&tarball)),
        None => (shared_cache(), parse_pin(BUILT_IN_PIN).expect("BUILT_IN_PIN is a pin").0),
    };
    w.real_gate(&cache);
    w.agent("spec", [writes("spec.md", SPEC)]);
    w.agent("build", [answered(0, "41"), answered(1, "42")]);
    w.agent("review", [approved(1)]);

    let run = w.start(&["run", "--quality-gate", "local", "--", TASK], true);
    let held = w.root().join(HELD_PID);
    w.wait_for(|| held.exists(), "the gate's test to start");
    w.invocation(run).kill();
    let key = w.run_key(1);
    let resumed = w.run(&["resume", "--force", &key]);
    let pid = read(&held).trim().parse::<i32>().expect("the held test's pid");
    assert!(procs::ended(pid).expect("reading /proc"), "the resume left the stopped round's test, pid {pid}, running");

    let inv = w.invocation(resumed);
    let (exit, stdout) = (inv.wait(), inv.stdout());
    assert!(exit == 0 && stdout.starts_with("APPROVE: "), "the resume ended {exit}: {stdout}{}", inv.stderr());
    let state = w.run_state(&key);
    let rounds = state["quality_rounds"].as_array().expect("quality_rounds");
    let gates: Vec<&str> = rounds.iter().map(|r| str_at(r, "gate")).collect();
    assert_eq!(gates, ["ERROR", "OK"], "{rounds:?}");
    let failed = rounds[0]["conditions"].as_array().is_some_and(|c| !c.is_empty());
    assert!(failed, "the failed round names no failed condition: {:?}", rounds[0]);
    let quality = read(&run_dir(&w.project, &key).join("quality-1-1.md"));
    assert!(quality.starts_with("GATE: ERROR\n"), "{quality}");
    let judged_by = format!("octoflow-gate {release}");
    assert!(quality.contains(&judged_by), "judged by another release than {release}: {quality}");
    assert!(quality.contains("holds_42"), "the quality file names no failing test: {quality}");
}

/// The cache real_gate keeps the built-in release in: OCTOFLOW_GATE_CACHE, else the developer's own XDG cache.
fn shared_cache() -> PathBuf {
    let set = |v: Option<std::ffi::OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    set(std::env::var_os("OCTOFLOW_GATE_CACHE"))
        .or_else(|| set(std::env::var_os("XDG_CACHE_HOME")))
        .or_else(|| set(std::env::var_os("HOME")).map(|home| home.join(".cache")))
        .expect("neither OCTOFLOW_GATE_CACHE, XDG_CACHE_HOME nor HOME is set")
}

/// Pin w's project to the release candidate tarball, `octoflow-gate-<version>.tar.gz`, put it in a cache of w's own
/// where the engine's job finds it already downloaded, and return that cache and the version.
fn candidate_pinned(w: &World, tarball: &Path) -> (PathBuf, String) {
    let name = tarball.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let version = name
        .strip_prefix("octoflow-gate-")
        .and_then(|rest| rest.strip_suffix(".tar.gz"))
        .unwrap_or_else(|| panic!("OCTOFLOW_GATE_TARBALL {} is no octoflow-gate-<version>.tar.gz", tarball.display()));
    let sum = Command::new("sha256sum").arg(tarball).output().unwrap_or_else(|e| panic!("running sha256sum: {e}"));
    assert!(sum.status.success(), "sha256sum {}: {}", tarball.display(), String::from_utf8_lossy(&sum.stderr));
    let sha256 = String::from_utf8_lossy(&sum.stdout).split(' ').next().unwrap_or_default().to_string();
    // Named as the engine's job names a release it downloaded (local_gate's JOB).
    let cache = w.root().join("gate-cache");
    let kept = cache.join("octoflow/gate").join(format!("octoflow-gate-{version}-{sha256}.tar.gz"));
    fs::create_dir_all(kept.parent().expect("a directory")).unwrap_or_else(|e| panic!("making the gate cache: {e}"));
    fs::copy(tarball, &kept).unwrap_or_else(|e| panic!("copying {} to {}: {e}", tarball.display(), kept.display()));
    w.write("project/ci/gate.pin", &format!("{version} {sha256}\n"));
    w.git(&w.project, &["add", "ci/gate.pin"]);
    w.git(&w.project, &["commit", "--quiet", "-m", "Pin the gate candidate"]);
    w.git(&w.project, &["push", "--quiet", "origin", "main"]);
    (cache, version.to_string())
}

/// The text of the file at path; panics naming it when it cannot be read.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}
