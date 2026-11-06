//! The quality rounds of a run with the local gate (`--quality-gate local`): the gate run on the host in place of a
//! Jenkins build, played by FakeHost::run_command, with nothing pushed and nothing asked of Jenkins; a gate that
//! could not judge, a secret in the snapshot, a resume in a round, and the preflight.

use std::cell::RefCell;
use std::collections::VecDeque;

use super::*;
use crate::ci::fake::Outcome;
use crate::constants::CI_REF_PREFIX;
use crate::host::{Exec, ExecError};
use crate::prompts::QUALITY_PASSED_NOTE;
use crate::quality::{parse_gate, Leak, QUALITY_LEAK};

/// What one run of the fake gate does: judge, writing an Outcome's quality.md and quality.json, exiting 0 for OK and
/// 1 otherwise; or not judge, exiting 2 with this on standard error.
enum GateRun {
    Judges(Outcome),
    CannotJudge(&'static str),
}

/// The commands the fake gate was run with, in order: the gate's own runs, not the preflight's or the fetch's.
type GateCalls = Rc<RefCell<Vec<Vec<String>>>>;

/// Have f's host play the local gate's commands: python3 starts, the cache is writable, the built-in release is
/// fetched into /cache, and each run of the gate plays the next of runs, then green ones.
fn plays(f: &Fixture, runs: Vec<GateRun>) -> GateCalls {
    let calls = GateCalls::default();
    let recorded = Rc::clone(&calls);
    let mut runs: VecDeque<GateRun> = runs.into();
    f.host.state().run_command = Some(Box::new(move |host, argv| {
        let exits = |status: i32, stdout: &str, stderr: &str| {
            let command = argv.join(" ");
            Some(Ok(Exec { command, status, stdout: stdout.into(), stderr: stderr.into() }))
        };
        match argv {
            ["python3", "--version"] => exits(0, "Python 3.12.0\n", ""),
            ["sh", "-c", script] if script.contains("mkdir -p") => exits(0, "", ""),
            ["sh", "-c", _, "_", version, ..] if !argv.contains(&"python3") => exits(0, &format!("/cache/{version}\n"), ""),
            ["rm", "-f", "--", ..] => exits(0, "", ""),
            ["sh", "-c", _, "_", _, "python3", ..] => {
                recorded.borrow_mut().push(argv.iter().map(|a| a.to_string()).collect());
                let out = argv[argv.iter().position(|a| *a == "--out").expect("--out") + 1];
                match runs.pop_front().unwrap_or(GateRun::Judges(green())) {
                    GateRun::Judges(outcome) => {
                        let json = out.replace(".md", ".json");
                        host.write(out, &outcome.quality.expect("a quality.md"), false).expect("the fake host writes");
                        host.write(&json, &outcome.quality_json.expect("a quality.json"), false).expect("written");
                        exits(i32::from(outcome.result != "SUCCESS"), "", "")
                    }
                    GateRun::CannotJudge(said) => exits(2, "", said),
                }
            }
            _ => None,
        }
    }));
    calls
}

/// make_workflow with the local gate, playing runs.
fn gated_locally(script: Script, runs: Vec<GateRun>) -> (Fixture, GateCalls) {
    let f = Fixture::new(script).gated_locally();
    let calls = plays(&f, runs);
    (f, calls)
}

/// How many snapshots were pushed to a ref of their own, as a Jenkins round pushes each.
fn snapshot_pushes(f: &Fixture) -> usize {
    let calls = &f.host.state().git_calls;
    calls.iter().filter(|c| c[0] == "push" && c.iter().any(|a| a.contains(CI_REF_PREFIX))).count()
}

#[test]
fn the_local_gate_fails_then_passes_with_nothing_pushed() {
    let (builds, reviews) = (seen(), seen());
    let (f, calls) = gated_locally(
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), recording(fix_turn(1, 1), &builds)]),
            ("review", vec![recording(review_turn(1, APPROVE), &reviews)]),
        ],
        vec![GateRun::Judges(red())],
    );
    let mut wf = f.workflow(RunSettings::default());

    assert_eq!(wf.run(), verdict(APPROVE));

    let state = &f.state;
    let quality = f.quality_file(1, 1);
    assert!(quality.starts_with(
        "GATE: ERROR\n\noctoflow-gate 0.5.0, run locally, judged snapshot `snap1` (round 1, quality round 1 of 3).\n"
    ), "{quality}");
    assert!(quality.contains("1. `limiter.py:3` error ruff F401: `os` imported but unused\n"));
    assert_eq!(*builds.borrow(), [quality_fix(state, 1, 1) + &note()]);
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &state.quality_path(1, 2))));
    assert!(f.quality_file(1, 2).starts_with("GATE: OK"));
    // Each round judges the working tree since the base, its reports in a directory of its own.
    let calls = calls.borrow();
    assert_eq!(calls.len(), 2);
    let gate_dir = state.gate_dir(1, 2);
    assert_eq!(calls[1][4..], strings(&[
        "/proj", "python3", "/cache/0.5.0/gate.py", "--base", "abc123", "--config", &format!("{gate_dir}/judge/gate.toml"),
        "--rust-syntax", "/cache/0.5.0/gate-rust-linux-x86_64", "--out", &format!("{gate_dir}/quality.md"),
    ]));
    // Nothing went to origin or to Jenkins, and nothing is left to delete.
    assert_eq!(snapshot_pushes(&f), 0);
    assert!(f.ci.state().requests.is_empty());
    let saved = f.saved();
    assert_eq!(text(&saved, "quality_job").as_deref(), Some("local"));
    assert_eq!(saved.get("ci"), Some(&Value::Object(Object::new())));
    assert_eq!(wf.state().quality_rounds.len(), 2);
}

#[test]
fn a_gate_that_cannot_judge_goes_back_to_the_builder_with_what_it_said() {
    let builds = seen();
    let (f, _) = gated_locally(
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), recording(fix_turn(1, 1), &builds)]),
            ("review", vec![review_turn(1, APPROVE)]),
        ],
        vec![GateRun::CannotJudge("gate.toml: unknown key \"x\"\n")],
    );

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));

    let quality = f.quality_file(1, 1);
    assert!(quality.starts_with("GATE: BUILD_FAILED\n"), "{quality}");
    assert!(quality.contains("octoflow-gate, run locally, could not judge snapshot `snap1` (round 1, quality round 1 of 3)"));
    assert!(quality.contains("gate.toml: unknown key \"x\""), "{quality}");
    assert!(builds.borrow()[0].contains(&f.state.quality_path(1, 1)));
    assert!(f.quality_file(1, 2).starts_with("GATE: OK"));
}

#[test]
fn a_secret_in_the_snapshot_is_not_judged() {
    let host = FakeHost::default();
    let leak = Leak {
        rule: "github-pat".into(),
        file: "limiter.py".into(),
        line: 3,
        commit: crate::host::fake::SCANNED.into(),
        fingerprint: "c0ffee:limiter.py:github-pat:3".into(),
    };
    host.state().leaks = vec![leak];
    let f = Fixture::on(
        Rc::new(host),
        new_state(),
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), Box::new(|prompt, state, host| {
                host.state().leaks.clear();
                fix_turn(1, 1)(prompt, state, host)
            })]),
            ("review", vec![review_turn(1, APPROVE)]),
        ],
    )
    .gated_locally();
    let calls = plays(&f, vec![]);

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));

    let quality = f.quality_file(1, 1);
    assert_eq!(parse_gate(&quality), Some(QUALITY_LEAK));
    assert!(quality.contains("the local gate did not judge the change"), "{quality}");
    // Only the round after the secret went was judged.
    assert_eq!(calls.borrow().len(), 1);
    assert_eq!(snapshot_pushes(&f), 0);
}

#[test]
fn a_resume_in_a_round_without_its_quality_file_runs_the_gate_again() {
    let mut state = quality_state(1);
    state.quality_job = Some("local".into());
    let host = Rc::new(FakeHost::default());
    host.state().files.insert(state.build_path(1), "report 1".to_string());
    let f = Fixture::resume(host, state, vec![("review", vec![review_turn(1, APPROVE)])], &[], &[]).gated_locally();
    let calls = plays(&f, vec![]);

    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));

    assert_eq!(calls.borrow().len(), 1);
    assert!(f.quality_file(1, 1).starts_with("GATE: OK\n\noctoflow-gate 0.5.0, run locally, judged snapshot `snap1`"));
}

#[test]
fn without_python3_the_run_fails_before_the_interview() {
    let f = Fixture::new(vec![("spec", vec![])]).gated_locally();
    f.host.state().run_command = Some(Box::new(|_, argv| {
        let missing = OrchestratorError(format!("{} is not on PATH", argv[0]));
        (argv[0] == "python3").then_some(Err(ExecError::NotFound(missing)))
    }));

    let e = failure(f.workflow(RunSettings::default()).run());

    assert!(e.contains("the local quality gate runs octoflow-gate with python3 from PATH, and python3 is not on PATH"), "{e}");
    assert!(f.herdr.state().calls.is_empty());
}
