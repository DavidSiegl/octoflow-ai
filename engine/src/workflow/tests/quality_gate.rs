//! TestQualityGate and TestStuckQualityLoop: the quality rounds of a gated run, on the local gate: the preflight,
//! the rounds, the heartbeat while the gate runs, the quality.json each round records, and a loop that stops when a
//! quality round fails on just what the one before it failed on.

use std::cell::RefCell;

use super::*;
use crate::constants::{CI_POLL_SECONDS, HEARTBEAT_SECONDS};
use crate::prompts::{QUALITY_PASSED_NOTE, QUALITY_STUCK_NOTE, QUALITY_UNRESOLVED_NOTE};
use crate::quality::finding_fingerprint;

/// make_workflow with the quality gate: Python's gated.
fn gated(script: Script) -> Fixture {
    Fixture::new(script).gated()
}

#[test]
fn without_the_gate_no_gate_runs() {
    let f = Fixture::new(approved());
    let mut wf = f.workflow(RunSettings::default());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(f.gate.borrow().jobs.is_empty());
    let saved = f.saved();
    assert_eq!(saved.get("quality_job"), Some(&Value::Null));
    assert_eq!(saved.get("quality_round"), Some(&Value::from(0)));
    assert_eq!(saved.get("ci"), Some(&Value::Object(Object::new())));
    assert!(f.host.state().snapshots.is_empty());
}

#[test]
fn gate_passes_first_time() {
    let seen = seen();
    let f = gated(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![build_turn(1)]),
        ("review", vec![recording(review_turn(1, APPROVE), &seen)]),
    ]);
    let mut wf = f.workflow(RunSettings::default());

    assert_eq!(wf.run(), verdict(APPROVE));

    assert_eq!(f.gate.borrow().jobs.len(), 1);
    assert_eq!(f.host.state().snapshots[0].0, "abc123");
    let state = &f.state;
    assert_eq!(
        f.quality_file(1, 1),
        "GATE: OK\n\noctoflow-gate 0.10.0, run locally, judged snapshot `snap1` (round 1, quality round 1 of 3).\n\
         \n`orchestrator.py gate` judged the working tree.\n\n## Issues on changed lines\n\nNone.\n"
    );
    assert!(seen.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &state.quality_path(1, 1))));
    assert!(seen.borrow()[0].contains(&format!("The Builder's report is in {}", state.build_path(1))));
    assert!(f.host.state().prs[0].body.contains("<summary>Quality gate (round 1)</summary>"));
    let saved = f.saved();
    assert_eq!(text(&saved, "phase").as_deref(), Some("done"));
    assert_eq!(saved.get("ci"), Some(&Value::Object(Object::new())));
}

#[test]
fn gate_fails_then_passes() {
    let (builds, reviews) = (seen(), seen());
    let f = gated_building(
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), recording(fix_turn(1, 1), &builds)]),
            ("review", vec![recording(review_turn(1, APPROVE), &reviews)]),
        ],
        vec![red(), green()],
    );
    let mut wf = f.workflow(RunSettings::default());

    assert_eq!(wf.run(), verdict(APPROVE));

    let state = &f.state;
    let quality = f.quality_file(1, 1);
    assert!(quality.starts_with(
        "GATE: ERROR\n\noctoflow-gate 0.10.0, run locally, judged snapshot `snap1` (round 1, quality round 1 of 3).\n"
    ));
    assert!(quality.contains("1. `limiter.py:3` error ruff F401: `os` imported but unused\n"));
    assert_eq!(*builds.borrow(), [quality_fix(state, 1, 1) + &note()]);
    assert!(reviews.borrow()[0].contains(&format!("The Builder's report is in {}", state.quality_build_path(1, 1))));
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &state.quality_path(1, 2))));
    assert!(f.quality_file(1, 2).starts_with("GATE: OK"));
    assert_eq!(f.gate.borrow().jobs.len(), 2);
}

#[test]
fn rounds_run_out() {
    let reviews = seen();
    let f = gated_building(
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), fix_turn(1, 1)]),
            ("review", vec![recording(review_turn(1, APPROVE), &reviews)]),
        ],
        vec![red(), red2()],
    );
    let mut wf = f.workflow(RunSettings { max_quality_rounds: 2, ..RunSettings::default() });

    assert_eq!(wf.run(), verdict(APPROVE));

    assert_eq!(prompts_to(&f, "build-a1b2c3"), 2);
    let state = &f.state;
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_UNRESOLVED_NOTE, &state.quality_path(1, 2))));
    assert!(reviews.borrow()[0].contains(&format!("The Builder's report is in {}", state.quality_build_path(1, 1))));
    assert_eq!(f.host.state().prs[0].body.matches("GATE: ERROR").count(), 1);
}

#[test]
fn a_later_round_has_its_own_quality_rounds() {
    let reviews = seen();
    let f = gated(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![build_turn(1), build_turn(2)]),
        ("review", vec![review_turn(1, CHANGES_REQUESTED), recording(review_turn(2, APPROVE), &reviews)]),
    ]);
    let mut wf = f.workflow(RunSettings::default());

    assert_eq!(wf.run(), verdict(APPROVE));

    let judged: Vec<String> = f.gate.borrow().jobs.iter().map(|job| job[0].clone()).collect();
    assert_eq!(judged, [format!("{}/job-1", f.state.gate_dir(1, 1)), format!("{}/job-1", f.state.gate_dir(2, 1))]);
    assert!(f.quality_file(2, 1).contains("judged snapshot `snap2` (round 2, quality round 1 of 3)"));
    assert!(reviews.borrow()[0].contains(&format!("the new report is in {}", f.state.build_path(2))));
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &f.state.quality_path(2, 1))));
}

/// Run f, whose preflight is to fail with an error holding each of patterns, before any herdr call; the error, saved
/// too.
fn preflight_fails(f: &Fixture, settings: RunSettings, patterns: &[&str]) {
    let e = failure(f.workflow(settings).run());
    for pattern in patterns {
        assert!(e.contains(pattern), "{e}");
    }
    assert!(f.herdr.state().calls.is_empty());
    let saved = text(&f.saved(), "error").unwrap_or_default();
    for pattern in patterns {
        assert!(saved.contains(pattern), "{saved}");
    }
}

/// A run on host, with the quality gate, of a Spec writer that is never asked.
fn gated_on(host: FakeHost) -> Fixture {
    Fixture::on(Rc::new(host), new_state(), vec![("spec", vec![])]).gated()
}

#[test]
fn not_a_git_repository_is_refused() {
    for pull_request in [true, false] {
        let f = gated_on(FakeHost::new(None, "main"));
        let settings = RunSettings { pull_request, ..RunSettings::default() };
        preflight_fails(&f, settings, &["the quality gate needs a git repository"]);
        assert!(f.gate.borrow().jobs.is_empty(), "pull_request {pull_request}");
    }
}

#[test]
fn no_pr_needs_a_clean_tree() {
    let host = FakeHost::default();
    host.state().changed.insert("/proj/wip.py".into());
    preflight_fails(&gated_on(host), no_pr(), &["uncommitted changes"]);
}

#[test]
fn heartbeat_during_a_twenty_minute_build() {
    let f = gated_building(approved(), vec![Outcome { polls: 115, ..green() }]);
    let beats = Rc::new(RefCell::new(Vec::new()));
    let (clock, written) = (Rc::clone(&f.clock), Rc::clone(&beats));
    f.host.state().before_write = Some(Box::new(move |path, _| {
        if path.ends_with("state.json") {
            written.borrow_mut().push(clock.now());
        }
        Ok(())
    }));

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    assert!(f.clock.now() > 1100.0);
    let gaps: Vec<f64> = beats.borrow().windows(2).map(|w| w[1] - w[0]).collect();
    let most = gaps.iter().copied().fold(0.0, f64::max);
    assert!(most <= (HEARTBEAT_SECONDS + CI_POLL_SECONDS) as f64, "{most}");
}

#[test]
fn quality_file_is_followed_whatever_the_exit_status() {
    let f = gated_building(approved(), vec![Outcome { exit: 1, ..green() }]);
    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    assert!(f.quality_file(1, 1).starts_with("GATE: OK"));
}

// -- TestStuckQualityLoop ---------------------------------------------------------------------------------------

/// A gated run whose gate runs judge as outcomes say, with builds turns of the Builder in round 1; its fixture, run
/// to APPROVE, and the reviews' prompts.
fn stuck_loop(outcomes: Vec<Outcome>, builds: u32) -> (Fixture, Rc<RefCell<Vec<String>>>) {
    stuck_on(|script| gated_building(script, outcomes), builds)
}

/// stuck_loop, on the fixture make makes of the run's script.
fn stuck_on(make: impl FnOnce(Script) -> Fixture, builds: u32) -> (Fixture, Rc<RefCell<Vec<String>>>) {
    let reviews = seen();
    let mut turns = vec![build_turn(1)];
    turns.extend((1..builds).map(|q| fix_turn(1, q)));
    let f = make(vec![
        ("spec", vec![spec_turn()]),
        ("build", turns),
        ("review", vec![recording(review_turn(1, APPROVE), &reviews)]),
    ]);
    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    (f, reviews)
}

/// A build whose gate fails on findings.
fn failing_on(findings: &[&str]) -> Outcome {
    Outcome::gate("ERROR", findings)
}

const F401: &str = "`limiter.py:3` error ruff F401: `os` imported but unused";
const ARG_TYPE: &str = "`limiter.py:9` error mypy arg-type: Argument 1 has incompatible type";

#[test]
fn the_same_findings_again_go_to_review() {
    let (f, reviews) = stuck_loop(vec![red(), red()], 3);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 2);
    assert_eq!(f.saved().get("quality_round"), Some(&Value::from(2)));
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_STUCK_NOTE, &f.state.quality_path(1, 2))));
    let unresolved: String = QUALITY_UNRESOLVED_NOTE.chars().take(40).collect();
    assert!(!reviews.borrow()[0].contains(&unresolved));
    assert!(f.told.lines().iter().any(|l| l
        == "[quality] round 1, quality round 2 failed on just what quality round 1 failed on; going on rather than \
            back to build"));
}

#[test]
fn a_fixed_finding_and_a_new_one_keep_the_loop_going() {
    let both = failing_on(&[F401, ARG_TYPE]);
    let new = failing_on(&[ARG_TYPE, "`limiter.py:1` error ruff E501: Line too long"]);
    let (f, reviews) = stuck_loop(vec![both, new, red()], 3);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 3);
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_UNRESOLVED_NOTE, &f.state.quality_path(1, 3))));
}

#[test]
fn a_fixed_finding_keeps_the_loop_going() {
    let (f, _) = stuck_loop(vec![failing_on(&[F401, ARG_TYPE]), red(), red()], 3);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 3);
}

#[test]
fn a_finding_on_a_moved_line_is_the_same() {
    let moved = failing_on(&["`limiter.py:12` error ruff F401: `os` imported but unused"]);
    let (f, _) = stuck_loop(vec![red(), moved], 3);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 2);
}

#[test]
fn a_gate_that_could_not_judge_is_not_stuck() {
    let (f, _) = stuck_on(
        |script| {
            let f = Fixture::new(script).gated();
            let cannot = || GateRun::CannotJudge("error: cannot install\n");
            f.gate.borrow_mut().runs = [cannot(), cannot(), cannot()].into();
            f
        },
        3,
    );
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 3);
}

// -- quality.json ------------------------------------------------------------------------------------------------

/// outcome with its quality.json changed by change, as a JSON object.
fn reporting(outcome: Outcome, change: impl FnOnce(&mut Object)) -> Outcome {
    let mut report: Object = serde_json::from_str(&outcome.quality_json).unwrap();
    change(&mut report);
    Outcome { quality_json: Value::Object(report).to_string(), ..outcome }
}

/// red, its quality.json listing one more failing finding, of mypy, than its quality file.
fn red_with_more_in_its_json() -> Outcome {
    reporting(red(), |r| {
        let more = serde_json::json!({"check": "mypy", "fails": true, "fingerprint": "0123456789abcdef"});
        r["findings"].as_array_mut().unwrap().push(more);
    })
}

/// A run whose Builder answers one quality round and whose Reviewer approves.
fn fixed_once() -> Script {
    vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![build_turn(1), fix_turn(1, 1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]
}

/// The fingerprints_by_check the run recorded for quality round q of round 1.
fn recorded_fingerprints(f: &Fixture, q: u32) -> Value {
    let saved = f.saved();
    let rounds = saved.get("quality_rounds").and_then(Value::as_array).unwrap();
    let round = rounds.iter().find(|r| r.get("quality_round") == Some(&Value::from(q))).unwrap();
    round.get("fingerprints_by_check").cloned().unwrap()
}

#[test]
fn the_round_records_what_quality_json_says_failed() {
    let f = gated_building(fixed_once(), vec![red_with_more_in_its_json(), green()]);
    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    let f401 = finding_fingerprint(Some("limiter.py"), "ruff F401", "`os` imported but unused");
    assert_eq!(recorded_fingerprints(&f, 1), serde_json::json!({"ruff": [f401], "mypy": ["0123456789abcdef"]}));
    // The agents read the quality file, which the build wrote: quality.json changes none of its words.
    assert!(!f.quality_file(1, 1).contains("mypy"));
}

#[test]
fn stuck_compares_every_failure_quality_json_lists() {
    // The quality files are the same, but quality.json fails round 2 on a finding round 1 did not have.
    let (f, _) = stuck_loop(vec![red(), red_with_more_in_its_json(), red()], 3);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 3);
}

#[test]
fn an_unknown_schema_version_stops_the_round_naming_it() {
    let newer = reporting(red(), |r| {
        r.insert("schema_version".into(), Value::from(2));
    });
    let f = gated_building(approved(), vec![newer]);
    let e = failure(f.workflow(RunSettings::default()).run());
    assert!(e.contains("quality.json has schema_version 2, and this engine reads only schema_version 1"), "{e}");
    assert!(f.host.state().files.get(&f.state.quality_path(1, 1)).is_none(), "no quality file is written");
    assert_build_is_left_to_build_again(&f);
}

/// The run saved nothing of its round's judgement, so that a resume judges the snapshot again.
fn assert_build_is_left_to_build_again(f: &Fixture) {
    assert_eq!(f.saved().get("ci"), Some(&Value::Object(Object::new())));
    assert!(f.host.state().files.get(&f.state.quality_path(1, 1)).is_none());
}

#[test]
fn a_quality_json_whose_gate_is_not_its_quality_file_s_is_rejected() {
    let f = gated_building(approved(), vec![reporting(green(), |r| {
        r.insert("gate".into(), Value::from("ERROR"));
    })]);
    let e = failure(f.workflow(RunSettings::default()).run());
    assert!(e.contains("its quality.md says GATE: OK, but its quality.json says ERROR"), "{e}");
    assert!(f.host.state().files.get(&f.state.quality_path(1, 1)).is_none(), "no quality file is written");
    assert_build_is_left_to_build_again(&f);
}

