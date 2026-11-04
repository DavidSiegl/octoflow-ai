//! Workflows with several verdict steps (acceptance-verdict/01): a round runs them in order and ends at the first
//! that does not approve, one round counter and max_rounds bound them all, and the run records each one's verdict
//! per round.

use super::*;
use crate::pipeline::{Pipeline, Step};
use crate::test_support::{accepting, roles};

const D: &str = "/proj/.orchestrator/runs/20260929-120000-a1b2c3";

fn with_pipeline(p: &Pipeline, max_rounds: u32) -> RunSettings {
    RunSettings { pipeline: Some(p.clone()), max_rounds, ..RunSettings::default() }
}

/// The Product Manager's turn of round n in test_support's accepting workflow, with verdict.
fn accept_turn(n: u32, verdict: &str) -> Turn {
    writes(move |s| format!("{}/accept-{n}.md", s.dir()), &format!("**VERDICT: {verdict}**\n1. criterion"), "done")
}

/// The verdicts the run recorded, as (round, step id, verdict), in order.
fn verdicts(s: &RunState) -> Vec<(u32, &str, &str)> {
    s.verdicts.iter().map(|v| (v.round, v.step.as_str(), v.verdict.as_str())).collect()
}

/// The run's summary.md.
fn summary(f: &Fixture) -> String {
    f.host.state().files.get(&format!("{D}/summary.md")).cloned().expect("the run wrote summary.md")
}

#[test]
fn both_approve_in_one_round() {
    let p = accepting();
    let seen = seen();
    let f = Fixture::on(Rc::new(FakeHost::default()), new_run(&p), vec![
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
        ("accept", vec![recording(accept_turn(1, APPROVE), &seen)]),
    ]);
    let mut wf = f.workflow(with_pipeline(&p, 3));
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(*seen.borrow(), [format!("Accept {D}/build-1.md against add a rate limiter; write {D}/accept-1.md.")]);
    assert_eq!(verdicts(wf.state()), [(1, "review", APPROVE), (1, "accept", APPROVE)]);
    let pr = f.host.state().prs[0].clone();
    assert!(!pr.draft);
    assert!(pr.body.contains("Verdicts after 1 round: review **APPROVE**, accept **APPROVE**."), "{}", pr.body);
    assert!(pr.body.contains("<details>\n<summary>Review (review-1.md)</summary>\n\n**VERDICT: APPROVE**\n1. finding"));
    assert!(pr.body.contains("<details>\n<summary>Review (accept-1.md)</summary>\n\n**VERDICT: APPROVE**\n1. criterion"));
    assert!(summary(&f).contains("- Round 1: review APPROVE; accept APPROVE\n"));
}

#[test]
fn the_second_requests_changes_and_the_next_round_runs_both_again() {
    let p = accepting();
    let (seen, built) = (seen(), seen());
    let f = Fixture::on(Rc::new(FakeHost::default()), new_run(&p), vec![
        ("build", vec![build_turn(1), recording(build_turn(2), &built)]),
        ("review", vec![review_turn(1, APPROVE), review_turn(2, APPROVE)]),
        ("accept", vec![recording(accept_turn(1, CHANGES_REQUESTED), &seen), recording(accept_turn(2, APPROVE), &seen)]),
    ]);
    let mut wf = f.workflow(with_pipeline(&p, 3));
    assert_eq!(wf.run(), verdict(APPROVE));
    // The Builder is pointed at the acceptance that sent the round back, not at the review that approved.
    assert_eq!(*built.borrow(), [format!("Fix {D}/accept-1.md; report to {D}/build-2.md.{}", note())]);
    assert_eq!(seen.borrow()[1], format!("Accept again; write {D}/accept-2.md."));
    assert_eq!(names(&f, "prompt"), ["build", "review", "accept", "build", "review", "accept"].map(|r| format!("{r}-a1b2c3")));
    let s = wf.state();
    assert_eq!((s.round, s.phase.as_str()), (2, "done"));
    assert_eq!(
        verdicts(s),
        [(1, "review", APPROVE), (1, "accept", CHANGES_REQUESTED), (2, "review", APPROVE), (2, "accept", APPROVE)]
    );
    assert!(summary(&f).contains("- Round 1: review APPROVE; accept CHANGES_REQUESTED\n- Round 2: review APPROVE; accept APPROVE\n"));
}

#[test]
fn the_first_requests_changes_and_the_second_does_not_run() {
    let p = accepting();
    let (accepted, built) = (seen(), seen());
    let f = Fixture::on(Rc::new(FakeHost::default()), new_run(&p), vec![
        ("build", vec![build_turn(1), recording(build_turn(2), &built)]),
        ("review", vec![review_turn(1, CHANGES_REQUESTED), review_turn(2, APPROVE)]),
        ("accept", vec![recording(accept_turn(2, APPROVE), &accepted)]),
    ]);
    let mut wf = f.workflow(with_pipeline(&p, 3));
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(names(&f, "prompt"), ["build", "review", "build", "review", "accept"].map(|r| format!("{r}-a1b2c3")));
    assert_eq!(*built.borrow(), [format!("Fix {D}/review-1.md; report to {D}/build-2.md.{}", note())]);
    // The acceptance's first turn, in round 2, gets its first prompt, not the one for a step that has judged before.
    assert_eq!(*accepted.borrow(), [format!("Accept {D}/build-2.md against add a rate limiter; write {D}/accept-2.md.")]);
    assert_eq!(verdicts(wf.state()), [(1, "review", CHANGES_REQUESTED), (2, "review", APPROVE), (2, "accept", APPROVE)]);
    assert!(!f.host.state().files.contains_key(&format!("{D}/accept-1.md")));
    assert!(summary(&f).contains("- Round 1: review CHANGES_REQUESTED\n- Round 2: review APPROVE; accept APPROVE\n"));
}

#[test]
fn max_rounds_bounds_both_and_the_last_round_ends_at_the_first_that_does_not_approve() {
    let p = accepting();
    let f = Fixture::on(Rc::new(FakeHost::default()), new_run(&p), vec![
        ("build", vec![build_turn(1), build_turn(2)]),
        ("review", vec![review_turn(1, APPROVE), review_turn(2, CHANGES_REQUESTED)]),
        ("accept", vec![accept_turn(1, CHANGES_REQUESTED)]),
    ]);
    let mut wf = f.workflow(with_pipeline(&p, 2));
    assert_eq!(wf.run(), verdict(CHANGES_REQUESTED));
    assert_eq!(names(&f, "prompt"), ["build", "review", "accept", "build", "review"].map(|r| format!("{r}-a1b2c3")));
    let s = wf.state();
    assert_eq!((s.round, s.phase.as_str()), (2, "done"));
    assert_eq!(verdicts(s), [(1, "review", APPROVE), (1, "accept", CHANGES_REQUESTED), (2, "review", CHANGES_REQUESTED)]);
    let pr = f.host.state().prs[0].clone();
    assert!(pr.draft);
    assert!(
        pr.body.contains(
            "Verdicts after 2 rounds: review **CHANGES_REQUESTED**.\n\nreview still requested changes after the last round, \
             so this is a draft."
        ),
        "{}",
        pr.body
    );
    // The last round's only verdict file, named by its file as the line names its step.
    assert!(pr.body.contains("<details open>\n<summary>Review (review-2.md)</summary>"), "{}", pr.body);
}

#[test]
fn a_verdict_step_s_earlier_files_are_those_it_wrote() {
    // Round 1: the acceptance requests changes; round 2: the review does, so the acceptance does not run; round 3:
    // its earlier file is round 1's alone.
    let mut steps = accepting().steps().to_vec();
    steps[2].again = Some("Accept again after {prev_accept_path} ({earlier_accept_paths}); write {accept_path}.".into());
    let p = Pipeline::new("accepting", accepting().roles().clone(), steps).unwrap();
    let accepted = seen();
    let f = Fixture::on(Rc::new(FakeHost::default()), new_run(&p), vec![
        ("build", vec![build_turn(1), build_turn(2), build_turn(3)]),
        ("review", vec![review_turn(1, APPROVE), review_turn(2, CHANGES_REQUESTED), review_turn(3, APPROVE)]),
        ("accept", vec![accept_turn(1, CHANGES_REQUESTED), recording(accept_turn(3, APPROVE), &accepted)]),
    ]);
    let mut wf = f.workflow(with_pipeline(&p, 3));
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(
        *accepted.borrow(),
        [format!("Accept again after {D}/accept-1.md ({D}/accept-1.md); write {D}/accept-3.md.")]
    );
}

#[test]
fn a_last_round_that_ends_early_goes_on_past_every_verdict_step() {
    let mut steps = accepting().steps().to_vec();
    steps.push(Step::new("notes", "notes", "notes.md", "Write release notes to {notes_path}."));
    let roles = roles(&[("build", "Builder"), ("review", "Reviewer"), ("accept", "Product Manager"), ("notes", "Writer")]);
    let p = Pipeline::new("noted", roles, steps).unwrap();
    let f = Fixture::on(Rc::new(FakeHost::default()), new_run(&p), vec![
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, CHANGES_REQUESTED)]),
        ("notes", vec![writes(|s| format!("{}/notes.md", s.dir()), "notes", "done")]),
    ]);
    let mut wf = f.workflow(with_pipeline(&p, 1));
    assert_eq!(wf.run(), verdict(CHANGES_REQUESTED));
    assert_eq!(names(&f, "prompt"), ["build", "review", "notes"].map(|r| format!("{r}-a1b2c3")));
}

#[test]
fn a_resume_between_the_two_verdict_turns_takes_only_the_second() {
    let p = accepting();
    let f = Fixture::on(Rc::new(FakeHost::default()), new_run(&p), vec![
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    f.play("accept", vec![then_interrupted(&f.interrupt, working())]);
    assert_eq!(f.workflow(with_pipeline(&p, 3)).run(), Err(Stopped::Interrupted));
    let saved = f.saved();
    assert_eq!((text(&saved, "phase"), saved.get("round")), (Some("accept".into()), Some(&Value::from(1))));
    let state = RunState::from_saved(&saved).unwrap();
    assert_eq!(verdicts(&state), [(1, "review", APPROVE)]);
    // The review's approval is not yet the run's: the acceptance has still to judge the round.
    assert_eq!(state.verdict, None);
    let f2 = Fixture::resume(Rc::clone(&f.host), state, vec![("accept", vec![accept_turn(1, APPROVE)])], &[], &[]);
    let mut wf = f2.workflow(RunSettings { pipeline: Some(p), ..f2.as_saved() });
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!names(&f2, "prompt").contains(&"review-a1b2c3".to_string()));
    assert_eq!(verdicts(wf.state()), [(1, "review", APPROVE), (1, "accept", APPROVE)]);
}

#[test]
fn a_run_saved_with_one_verdict_and_no_record_of_it_finishes() {
    // Saved by a release before RunState::verdicts, in the review of round 2, after round 1's changes.
    let mut state = saved_run("review", 2, FULL);
    state.verdict = Some(CHANGES_REQUESTED.into());
    let Value::Object(mut saved) = state.to_json() else { unreachable!("a state is an object") };
    saved.shift_remove("verdicts");
    let state = RunState::from_saved(&saved).unwrap();
    assert!(state.verdicts.is_empty());
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![("review", vec![review_turn(2, APPROVE)])], &[], &[]);
    f.put(&format!("{D}/build-2.md"), "report 2");
    let mut wf = f.workflow(RunSettings { pull_request: false, ..f.as_saved() });
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(verdicts(wf.state()), [(2, "review", APPROVE)]);
}

#[test]
fn a_run_saved_without_records_keeps_its_earlier_rounds_once_it_goes_on() {
    // Saved by a release before RunState::verdicts, in the review of round 3; resumed, it requests changes, and the
    // review of round 4 is to name every earlier review, rounds 1 and 2 as well as the recorded round 3.
    let default = crate::pipeline::DEFAULT_WORKFLOW.clone();
    let mut steps = default.steps().to_vec();
    steps[2].again = Some("Recheck against {earlier_review_paths}; write {review_path}.".into());
    let p = Pipeline::new("rechecking", default.roles().clone(), steps).unwrap();
    let mut state = saved_run("review", 3, FULL);
    state.verdict = Some(CHANGES_REQUESTED.into());
    let Value::Object(mut saved) = state.to_json() else { unreachable!("a state is an object") };
    saved.shift_remove("verdicts");
    let state = RunState::from_saved(&saved).unwrap();
    let reviewed = seen();
    let script = vec![
        ("build", vec![build_turn(4)]),
        ("review", vec![review_turn(3, CHANGES_REQUESTED), recording(review_turn(4, APPROVE), &reviewed)]),
    ];
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, script, &["build", "review"], &[]);
    for n in 1..=3 {
        f.put(&format!("{D}/build-{n}.md"), "report");
    }
    for n in 1..=2 {
        f.put(&format!("{D}/review-{n}.md"), "VERDICT: CHANGES_REQUESTED\n1. finding");
    }
    let mut wf = f.workflow(RunSettings { pipeline: Some(p), pull_request: false, max_rounds: 4, ..f.as_saved() });
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(verdicts(wf.state()), [(3, "review", CHANGES_REQUESTED), (4, "review", APPROVE)]);
    assert_eq!(
        *reviewed.borrow(),
        [format!("Recheck against {D}/review-1.md, {D}/review-2.md, {D}/review-3.md; write {D}/review-4.md.")]
    );
}
