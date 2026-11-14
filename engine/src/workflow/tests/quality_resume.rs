//! A run resumed in a quality round, or in the gated step's turn around one, picks the round up
//! from its quality file, if written, and a resumed Builder or Reviewer hears of the quality rounds before it; on the
//! local gate.

use super::*;
use crate::prompts::QUALITY_UNRESOLVED_NOTE;

/// resume_gated of state on a default FakeHost, the review approving.
fn resumed(state: RunState) -> Fixture {
    Fixture::resume_gated(Rc::new(FakeHost::default()), state, vec![("review", vec![review_turn(1, APPROVE)])], &[])
}

#[test]
fn resume_with_the_quality_file_makes_no_request() {
    let f = resumed(quality_state(1));
    f.put(&f.state.quality_path(1, 1), "GATE: OK\n");

    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));
    assert!(f.gate.borrow().jobs.is_empty());
}

#[test]
fn quality_file_without_a_gate_is_refused() {
    let f = Fixture::resume_gated(Rc::new(FakeHost::default()), quality_state(1), vec![], &[]);
    f.put(&f.state.quality_path(1, 1), "all good\n");

    let e = failure(f.workflow(f.as_saved_gated()).run());
    assert!(e.contains("quality-1-1.md does not start with a GATE line; fix it"), "{e}");
    assert!(f.gate.borrow().jobs.is_empty());
}

#[test]
fn fresh_builder_answering_a_quality_round_is_briefed() {
    let seen = seen();
    let mut state = saved_run("build", 1, FULL);
    state.quality_job = Some("local".to_string());
    state.quality_round = 1;
    state.prompted = Some("build-1-q1.md".to_string());
    let f = Fixture::resume_gated(
        Rc::new(FakeHost::default()),
        state,
        vec![("build", vec![recording(fix_turn(1, 1), &seen)]), ("review", vec![review_turn(1, APPROVE)])],
        &["review"],
    );
    f.put(&f.state.quality_path(1, 1), "GATE: ERROR\n");

    let mut wf = f.workflow(f.as_saved_gated());
    assert_eq!(wf.run(), verdict(APPROVE));
    let first = seen.borrow()[0].clone();
    assert!(first.contains("You are the Builder"));
    assert!(first.contains(&format!(
        "you are a fresh session. An earlier Builder session did the earlier turns; its changes are already in the \
         working tree, and its reports are {}.",
        f.state.build_path(1)
    )));
    assert!(first.ends_with(&(quality_fix(&f.state, 1, 1) + &note())), "{first}");
    assert_eq!(wf.state().quality_round, 2);
}

#[test]
fn fresh_builder_in_a_later_round_hears_of_every_earlier_report() {
    let seen = seen();
    let mut state = saved_run("build", 2, FULL);
    state.quality_job = Some("local".to_string());
    let f = Fixture::resume_gated(
        Rc::new(FakeHost::default()),
        state,
        vec![("build", vec![recording(build_turn(2), &seen)]), ("review", vec![review_turn(2, APPROVE)])],
        &["review"],
    );
    let s = &f.state;
    f.put(&s.quality_build_path(1, 1), "r");
    f.put(&s.quality_build_path(1, 2), "r");

    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));
    let reports = [s.build_path(1), s.quality_build_path(1, 1), s.quality_build_path(1, 2)].join(", ");
    assert!(seen.borrow()[0].contains(&format!("its reports are {reports}.")));
}

#[test]
fn the_last_allowed_quality_round_goes_to_review() {
    let reviews = seen();
    let mut state = quality_state(2);
    state.max_quality_rounds = 2;
    let f = Fixture::resume_gated(
        Rc::new(FakeHost::default()),
        state,
        vec![("review", vec![recording(review_turn(1, APPROVE), &reviews)])],
        &[],
    );
    f.judging(vec![red()]);
    f.put(&f.state.quality_build_path(1, 1), "report 1 q1");

    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 0);
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_UNRESOLVED_NOTE, &f.state.quality_path(1, 2))));
}

#[test]
fn a_change_the_gate_was_turned_off_before_judging_is_said_to_be_unjudged() {
    // resume --no-quality-gate turned the gate off after round 1's first quality round and the Builder's answer to
    // it, so the Reviewer hears that nothing judged the final change (turning the gate off is gate_switch.rs's).
    let mut state = saved_run("review", 1, FULL);
    (state.quality_gate_disabled, state.quality_round) = (true, 2);
    let reviews = seen();
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![("review", vec![recording(review_turn(1, APPROVE), &reviews)])],
        &[],
        &[],
    );
    f.put(&f.state.build_path(1), "report 1");
    f.put(&f.state.quality_path(1, 1), "GATE: ERROR\n");
    f.put(&f.state.quality_build_path(1, 1), "report 1 q1");

    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let off = filled(crate::prompts::QUALITY_OFF_NOTE, &[("quality_paths", &f.state.quality_path(1, 1))]);
    assert!(reviews.borrow()[0].ends_with(&format!("\n\n{off}")), "{}", reviews.borrow()[0]);
}

#[test]
fn a_quality_file_judged_again_replaces_its_round_s_record() {
    let mut state = quality_state(1);
    let stale = serde_json::json!({
        "round": 1, "quality_round": 1, "gate": "ERROR", "conditions": ["old"], "fingerprints_by_check": {}
    });
    state.quality_rounds.push(stale.as_object().cloned().unwrap());
    let f = resumed(state);

    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));
    let rounds = f.saved().get("quality_rounds").cloned().unwrap();
    assert_eq!(rounds.as_array().map(Vec::len), Some(1), "{rounds}");
    assert_eq!(rounds[0].get("gate"), Some(&Value::from("OK")), "{rounds}");
}
