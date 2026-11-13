//! The quality rounds of a run with the local gate (`--quality-gate local`): the gate run on the host in place of a
//! Jenkins build, played by FakeHost::run_command, with nothing pushed and nothing asked of Jenkins; a gate that
//! could not judge, a secret in the snapshot, a resume in a round, a gate stopped by the deadline or a Ctrl-C, and the
//! preflight.

use std::cell::RefCell;
use std::collections::VecDeque;

use super::*;
use crate::ci::fake::Outcome;
use crate::constants::CI_REF_PREFIX;
use crate::host::{Exec, Scanned};
use crate::local_gate;
use crate::prompts::QUALITY_PASSED_NOTE;
use crate::quality::{parse_gate, Leak, QUALITY_LEAK};

/// What one run of the fake gate does: judge, writing an Outcome's quality.md and quality.json, exiting 0 for OK and
/// 1 otherwise; not judge, exiting 2 with this on standard error; or run on until it is stopped.
enum GateRun {
    Judges(Outcome),
    CannotJudge(&'static str),
    Hangs,
}

/// What the fake local gate was asked, in order.
#[derive(Default)]
struct GateCalls {
    /// The arguments of each job started, after LAUNCH's own: the round's directory, version, sha256, URL and base.
    jobs: Vec<Vec<String>>,
    /// How many times a job was stopped.
    stops: usize,
}

/// Have f's host play the local gate's commands: the preflight passes, and each job started plays the next of runs,
/// then green ones, its files written at once, except a hanging one's, which runs until it is stopped.
fn plays(f: &Fixture, runs: Vec<GateRun>) -> Rc<RefCell<GateCalls>> {
    let calls = Rc::new(RefCell::new(GateCalls::default()));
    let recorded = Rc::clone(&calls);
    let mut runs: VecDeque<GateRun> = runs.into();
    let hanging = Rc::new(RefCell::new(false));
    let interrupt = f.interrupt.clone();
    f.host.state().run_command = Some(Box::new(move |host, argv| {
        let exits = |code: i32, stdout: &str| {
            let command = argv.join(" ");
            Some(Ok(Exec { command, status: code, stdout: stdout.as_bytes().to_vec(), stderr: Vec::new() }))
        };
        match argv {
            ["sh", "-c", script] if script.contains("mkdir -p") => exits(0, ""),
            // Each round's first start makes job-1.
            ["sh", "-c", script, "_", _out] if script.contains("job-$k") => exits(0, "1\n"),
            ["rm", "-rf", "--", paths @ ..] => {
                let mut state = host.state();
                let gone: Vec<String> = state
                    .files
                    .keys()
                    .filter(|file| paths.iter().any(|p| file == p || file.starts_with(&format!("{p}/"))))
                    .map(String::from)
                    .collect();
                for file in gone {
                    state.files.remove(&file);
                }
                drop(state);
                exits(0, "")
            }
            ["sh", "-c", _, "_", _log, "sh", "-c", _, _name, job @ ..] => {
                recorded.borrow_mut().jobs.push(job.iter().map(|a| a.to_string()).collect());
                let out = job[0];
                let write = |name: &str, text: &str| host.write(&format!("{out}/{name}"), text, false).expect("written");
                match runs.pop_front().unwrap_or(GateRun::Judges(green())) {
                    GateRun::Judges(outcome) => {
                        write("quality.md", &outcome.quality.expect("a quality.md"));
                        write("quality.json", &outcome.quality_json.expect("a quality.json"));
                        write("exit", if outcome.result == "SUCCESS" { "0\n" } else { "1\n" });
                    }
                    GateRun::CannotJudge(said) => {
                        write("gate.err", said);
                        write("exit", "2\n");
                    }
                    GateRun::Hangs => *hanging.borrow_mut() = true,
                }
                exits(0, "4242\n")
            }
            ["sh", "-c", script, "_", "4242"] if script.starts_with("kill -0") => exits(i32::from(!*hanging.borrow()), ""),
            ["sh", "-c", _, "_", "4242", _name, "tracked"] => {
                assert!(!interrupt.is_raised(), "a stop started with a Ctrl-C pending, which ProcessHost refuses");
                recorded.borrow_mut().stops += 1;
                *hanging.borrow_mut() = false;
                exits(0, "")
            }
            _ => None,
        }
    }));
    calls
}

/// make_workflow with the local gate, playing runs.
fn gated_locally(script: Script, runs: Vec<GateRun>) -> (Fixture, Rc<RefCell<GateCalls>>) {
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
        "GATE: ERROR\n\noctoflow-gate 0.10.0, run locally, judged snapshot `snap1` (round 1, quality round 1 of 3).\n"
    ), "{quality}");
    assert!(quality.contains("1. `limiter.py:3` error ruff F401: `os` imported but unused\n"));
    assert_eq!(*builds.borrow(), [quality_fix(state, 1, 1) + &note()]);
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &state.quality_path(1, 2))));
    assert!(f.quality_file(1, 2).starts_with("GATE: OK"));
    // Each round judges a checkout of its snapshot, gone once judged, its files in a directory of its own.
    let calls = calls.borrow();
    assert_eq!(calls.jobs.len(), 2);
    let (job, (version, sha256)) = (format!("{}/job-1", state.gate_dir(1, 2)), local_gate::parse_pin(local_gate::BUILT_IN_PIN).unwrap());
    let url = format!("https://github.com/DavidSiegl/octoflow-gate/releases/download/v{version}/octoflow-gate-{version}.tar.gz");
    assert_eq!(calls.jobs[1], strings(&[&job, &version, &sha256, &url, "abc123"]));
    let tree = format!("{job}/tree");
    let host = f.host.state();
    assert!(host.git_calls.contains(&strings(&["worktree", "add", "--detach", &tree, "snap2"])), "{:?}", host.git_calls);
    assert!(!host.worktrees.contains_key(tree.as_str()));
    assert_eq!(host.files.get(&format!("{job}/judge/gate.toml")).map(String::as_str), Some(""));
    // gitleaks scanned each snapshot's change alone, as it goes nowhere; the publish at the end scans as a push does.
    let scanned: Vec<_> = host.scans.iter().filter(|scan| scan.commit.starts_with("snap")).map(|scan| scan.scanned).collect();
    assert_eq!(scanned, [Scanned::SinceBase, Scanned::SinceBase], "{:?}", host.scans);
    assert_eq!(calls.stops, 0);
    drop(host);
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
        vec![GateRun::CannotJudge("error: gate.toml: unknown key \"x\"\n")],
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
    assert_eq!(calls.borrow().jobs.len(), 1);
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

    assert_eq!(calls.borrow().jobs.len(), 1);
    assert!(f.quality_file(1, 1).starts_with("GATE: OK\n\noctoflow-gate 0.10.0, run locally, judged snapshot `snap1`"));
}

#[test]
fn on_a_machine_that_fails_the_preflight_the_run_fails_before_the_interview() {
    let f = Fixture::new(vec![("spec", vec![])]).gated_locally();
    f.host.state().run_command = Some(Box::new(|_, argv| {
        let (command, stderr) = (argv.join(" "), b"curl is not on PATH\n".to_vec());
        Some(Ok(Exec { command, status: 1, stdout: Vec::new(), stderr }))
    }));

    let e = failure(f.workflow(RunSettings::default()).run());

    assert!(e.contains("the local quality gate fetches octoflow-gate and runs it on the project's machine: curl is not"), "{e}");
    assert!(f.herdr.state().calls.is_empty());
}

#[test]
fn a_gate_still_running_when_the_rounds_deadline_passes_is_stopped() {
    let (f, calls) = gated_locally(
        vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])],
        vec![GateRun::Hangs],
    );

    let e = failure(f.workflow(RunSettings::default()).run());

    assert!(e.starts_with("quality gate: the local gate did not finish within 1200s"), "{e}");
    assert_eq!(calls.borrow().stops, 1);
    assert!(!f.host.state().worktrees.contains_key(format!("{}/job-1/tree", f.state.gate_dir(1, 1)).as_str()));
}

#[test]
fn a_ctrl_c_while_the_gate_runs_stops_it() {
    let (f, calls) = gated_locally(
        vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])],
        vec![GateRun::Hangs],
    );
    let interrupt = f.interrupt.clone();
    let mut played = f.host.state().run_command.take().expect("the gate is played");
    f.host.state().run_command = Some(Box::new(move |host, argv| {
        // The Ctrl-C comes while the engine asks after the running gate.
        if argv.get(2).is_some_and(|script| script.starts_with("kill -0")) {
            interrupt.raise();
        }
        played(host, argv)
    }));

    assert_eq!(f.workflow(RunSettings::default()).run(), Err(Stopped::Interrupted));

    assert_eq!(calls.borrow().stops, 1);
}
