//! The quality rounds of a run with the local gate (`--quality-gate local`): the gate run on the host as a job of its
//! own, played by FakeHost::run_command; a gate that
//! could not judge, a secret in the snapshot, a resume in a round, a gate stopped by the deadline or a Ctrl-C, and the
//! preflight.

use super::*;
use crate::host::{Exec, Scanned};
use crate::local_gate;
use crate::prompts::QUALITY_PASSED_NOTE;
use crate::quality::{parse_gate, Leak, QUALITY_LEAK};

/// Fixture::new with the local gate, playing runs.
fn gated_locally(script: Script, runs: Vec<GateRun>) -> Fixture {
    let f = Fixture::new(script).gated();
    f.gate.borrow_mut().runs = runs.into();
    f
}

#[test]
fn the_local_gate_fails_then_passes_with_nothing_pushed() {
    let (builds, reviews) = (seen(), seen());
    let f = gated_locally(
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
    assert!(
        quality.starts_with(&format!(
            "GATE: ERROR\n\n{}, run locally, judged snapshot `snap1` (round 1, quality round 1 of 3).\n",
            built_in_gate()
        )),
        "{quality}"
    );
    assert!(quality.contains("1. `limiter.py:3` error ruff F401: `os` imported but unused\n"));
    assert_eq!(*builds.borrow(), [quality_fix(state, 1, 1) + &gated_note()]);
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &state.quality_path(1, 2))));
    assert!(f.quality_file(1, 2).starts_with("GATE: OK"));
    // Each round judges a checkout of its snapshot, gone once judged, its files in a directory of its own.
    let calls = f.gate.borrow();
    assert_eq!(calls.jobs.len(), 2);
    let (job, (version, sha256)) =
        (format!("{}/job-1", state.gate_dir(1, 2)), local_gate::parse_pin(local_gate::BUILT_IN_PIN).unwrap());
    let url = format!(
        "https://github.com/DavidSiegl/octoflow-gate/releases/download/v{version}/octoflow-gate-{version}.tar.gz"
    );
    assert_eq!(calls.jobs[1], strings(&[&job, &version, &sha256, &url, "abc123"]));
    let tree = format!("{job}/tree");
    let host = f.host.state();
    assert!(
        host.git_calls.contains(&strings(&["worktree", "add", "--detach", &tree, "snap2"])),
        "{:?}",
        host.git_calls
    );
    assert!(!host.worktrees.contains_key(tree.as_str()));
    assert_eq!(host.files.get(&format!("{job}/judge/gate.toml")).map(String::as_str), Some(""));
    // gitleaks scanned each snapshot's change alone, as it goes nowhere; the publish at the end scans as a push does.
    let scanned: Vec<_> =
        host.scans.iter().filter(|scan| scan.commit.starts_with("snap")).map(|scan| scan.scanned).collect();
    assert_eq!(scanned, [Scanned::SinceBase, Scanned::SinceBase], "{:?}", host.scans);
    assert_eq!(calls.stops, 0);
    drop(host);
    let saved = f.saved();
    assert_eq!(text(&saved, "quality_job").as_deref(), Some("local"));
    assert_eq!(saved.get("ci"), Some(&Value::Object(Object::new())));
    assert_eq!(wf.state().quality_rounds.len(), 2);
}

#[test]
fn a_gate_that_cannot_judge_goes_back_to_the_builder_with_what_it_said() {
    let builds = seen();
    let f = gated_locally(
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
    assert!(quality
        .contains("octoflow-gate, run locally, could not judge snapshot `snap1` (round 1, quality round 1 of 3)"));
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
            (
                "build",
                vec![
                    build_turn(1),
                    Box::new(|prompt, state, host| {
                        host.state().leaks.clear();
                        fix_turn(1, 1)(prompt, state, host)
                    }),
                ],
            ),
            ("review", vec![review_turn(1, APPROVE)]),
        ],
    )
    .gated();

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));

    let quality = f.quality_file(1, 1);
    assert_eq!(parse_gate(&quality), Some(QUALITY_LEAK));
    assert!(quality.contains("the local gate did not judge the change"), "{quality}");
    // Only the round after the secret went was judged.
    assert_eq!(f.gate.borrow().jobs.len(), 1);
}

#[test]
fn a_resume_in_a_round_without_its_quality_file_runs_the_gate_again() {
    let mut state = quality_state(1);
    state.quality_job = Some("local".into());
    let host = Rc::new(FakeHost::default());
    host.state().files.insert(state.build_path(1), "report 1".to_string());
    let f = Fixture::resume(host, state, vec![("review", vec![review_turn(1, APPROVE)])], &[], &[]).gated();

    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));

    assert_eq!(f.gate.borrow().jobs.len(), 1);
    assert!(f
        .quality_file(1, 1)
        .starts_with(&format!("GATE: OK\n\n{}, run locally, judged snapshot `snap1`", built_in_gate())));
}

#[test]
fn a_pinless_run_resumed_by_an_engine_with_another_built_in_pin_is_judged_by_the_first_engine_s_release() {
    // The engine that judged the run's first quality round was built with 0.10.0; this one is built with another.
    let first = "0.10.0 1111111111111111111111111111111111111111111111111111111111111111";
    assert_ne!(first, local_gate::BUILT_IN_PIN);
    let mut state = quality_state(1);
    state.built_in_gate_pin = Some(first.to_string());
    let host = Rc::new(FakeHost::default());
    host.state().files.insert(state.build_path(1), "report 1".to_string());
    let script = vec![("build", vec![fix_turn(1, 1)]), ("review", vec![review_turn(1, APPROVE)])];
    let f = Fixture::resume(host, state, script, &[], &[]).gated();
    f.gate.borrow_mut().runs = vec![GateRun::Judges(red())].into();

    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));

    let releases: Vec<(String, String)> =
        f.gate.borrow().jobs.iter().map(|job| (job[1].clone(), job[2].clone())).collect();
    let (version, sha256) = local_gate::parse_pin(first).unwrap();
    assert_eq!(releases, [(version.clone(), sha256.clone()), (version, sha256)]);
    assert!(
        f.quality_file(1, 2).starts_with("GATE: OK\n\noctoflow-gate 0.10.0, run locally"),
        "{}",
        f.quality_file(1, 2)
    );
    assert_eq!(text(&f.saved(), "built_in_gate_pin").as_deref(), Some(first));
}

#[test]
fn a_pinless_run_saved_without_a_gate_pin_takes_this_engine_s_at_its_next_round() {
    let mut state = quality_state(1);
    state.built_in_gate_pin = None;
    let host = Rc::new(FakeHost::default());
    host.state().files.insert(state.build_path(1), "report 1".to_string());
    let f = Fixture::resume(host, state, vec![("review", vec![review_turn(1, APPROVE)])], &[], &[]).gated();

    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));

    let (version, _) = local_gate::parse_pin(local_gate::BUILT_IN_PIN).unwrap();
    assert_eq!(f.gate.borrow().jobs[0][1], version);
    assert_eq!(text(&f.saved(), "built_in_gate_pin").as_deref(), Some(local_gate::BUILT_IN_PIN));
}

#[test]
fn a_pinless_run_saves_its_built_in_gate_pin_before_the_gate_can_judge() {
    let script =
        vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)]), ("review", vec![review_turn(1, APPROVE)])];
    let f = gated_locally(script, vec![]);

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));

    // An engine killed once the first job judged, before it read the verdict, leaves the pin for the resume.
    let saved = f.gate.borrow().saved_at_end[0].clone().expect("the run was saved before its gate judged");
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved["built_in_gate_pin"].as_str(), Some(local_gate::BUILT_IN_PIN));
}

#[test]
fn on_a_machine_that_fails_the_preflight_the_run_fails_before_the_interview() {
    let f = Fixture::new(vec![("spec", vec![])]).gated();
    f.host.state().run_command = Some(Box::new(|_, argv| {
        let (command, stderr) = (argv.join(" "), b"curl is not on PATH\n".to_vec());
        Some(Ok(Exec { command, status: 1, stdout: Vec::new(), stderr }))
    }));

    let e = failure(f.workflow(RunSettings::default()).run());

    assert!(
        e.contains("the local quality gate fetches octoflow-gate and runs it on the project's machine: curl is not"),
        "{e}"
    );
    assert!(f.herdr.state().calls.is_empty());
}

#[test]
fn a_gate_still_running_when_the_rounds_deadline_passes_is_stopped() {
    let f = gated_locally(vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])], vec![GateRun::Hangs]);

    let e = failure(f.workflow(RunSettings::default()).run());

    assert!(e.starts_with("quality gate: the local gate did not finish within 1200s"), "{e}");
    assert_eq!(f.gate.borrow().stops, 1);
    assert!(!f.host.state().worktrees.contains_key(format!("{}/job-1/tree", f.state.gate_dir(1, 1)).as_str()));
}

#[test]
fn a_ctrl_c_while_the_gate_runs_stops_it() {
    let f = gated_locally(vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])], vec![GateRun::Hangs]);
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

    assert_eq!(f.gate.borrow().stops, 1);
}
