//! TestResume: a run picked up from its state.json, the agents found alive, resumed into their sessions or started
//! afresh, each prompted only for what it still owes.
//!
//! Not here: its two cases of RunState.from_dict, which are crate::state's: test_from_dict_incomplete
//! (a_state_without_its_required_fields_or_with_a_wrong_type_is_refused_naming_the_run, ticket 04) and
//! test_from_dict_ignores_unknown_keys_and_fills_new_ones
//! (a_state_with_a_field_it_does_not_know_and_without_later_ones_reads_with_their_defaults).

use std::cell::RefCell;

use super::*;
use crate::json::object;
use crate::pipeline::{METRICS_FILE, SUMMARY_FILE};

/// saved_run, its prompt for file counted as delivered.
fn prompted(mut state: RunState, file: &str) -> RunState {
    state.prompted = Some(file.to_string());
    state
}

/// The build and review turns of round n, the review approving, the build recorded in seen.
fn build_and_approve(n: u32, seen: &Rc<RefCell<Vec<String>>>) -> Script {
    vec![("build", vec![recording(build_turn(n), seen)]), ("review", vec![review_turn(n, APPROVE)])]
}

#[test]
fn spec_written_while_orchestrator_was_gone() {
    // Run e292fb: the collector wrote spec.md and exited after the orchestrator died.
    let mut state = RunState::new("20260929-120000-a1b2c3", "add a rate limiter", "/proj");
    state.machine = Some("remote".into());
    state.workspace_id = "w1".into();
    state.agents.insert("spec", [("name", "spec-a1b2c3".to_string()), ("pane", "w1:p1".to_string())].into_iter().collect());
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ], &[], &[]);
    f.put(&f.state.spec_path(), "# Goal\nx");
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f, "spec-a1b2c3"), 0);
    assert!(!f.herdr.state().calls.iter().any(|c| matches!(c, Call::Workspace { .. } | Call::Focus { .. })));
    assert!(called(&f, &Call::Split { pane: "w1:p1".into(), direction: Direction::Right }));
    assert_eq!(wf.state().base.as_deref(), Some("abc123"));
}

#[test]
fn live_prompted_collector_is_only_waited_on() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), prompted(saved_run("spec", 0, &["spec"]), "spec.md"), vec![
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ], &["spec"], &[]);
    let (host, spec) = (Rc::clone(&f.host), f.state.spec_path());
    f.clock.hook(move |_| host.write(&spec, "# Goal\nx", false).unwrap());
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f, "spec-a1b2c3"), 0);
    assert!(!names(&f, "start").contains(&"spec-a1b2c3".to_string()));
    assert!(called(&f, &Call::Focus { name: "spec-a1b2c3".into() }));
    assert_eq!(f.told.titles()[0], "Spec Collector is waiting for you");
}

#[test]
fn live_collector_never_prompted_is_prompted_once() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), saved_run("spec", 0, &["spec"]), approved(), &["spec"], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f, "spec-a1b2c3"), 1);
}

#[test]
fn collector_without_session_restarts_the_interview() {
    let state = prompted(saved_run("spec", 0, &["spec"]), "spec.md");
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, approved(), &[], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert!(starts(&f).contains(&("spec-a1b2c3".into(), "w1:p1".into(), vec![])));
    assert_eq!(f.told.titles()[0], "Spec Collector restarted; the interview starts over");
}

#[test]
fn exited_builder_resumes_its_session_and_continues() {
    let seen = seen();
    let state = prompted(saved_run("build", 2, FULL), "build-2.md");
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, build_and_approve(2, &seen), &["review"], &[(
        "build", "s-build",
    )]);
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(starts(&f), [("build-a1b2c3".into(), "w1:p2".into(), strings(&["--resume", "s-build"]))]);
    assert_eq!(*seen.borrow(), [continue_prompt(&wf.state().build_path(2))]);
}

#[test]
fn resumed_builder_not_yet_prompted_gets_its_normal_prompt() {
    let seen = seen();
    let state = prompted(saved_run("build", 2, FULL), "review-1.md");
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, build_and_approve(2, &seen), &["review"], &[(
        "build", "s-build",
    )]);
    f.workflow(f.as_saved()).run().unwrap();
    assert!(seen.borrow()[0].starts_with("The Reviewer requested changes"), "{}", seen.borrow()[0]);
}

#[test]
fn exited_builder_without_session_gets_a_recovery_prompt() {
    let seen = seen();
    let state = prompted(saved_run("build", 2, FULL), "build-2.md");
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, build_and_approve(2, &seen), &["review"], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert!(starts(&f).contains(&("build-a1b2c3".into(), "w1:p2".into(), vec![])));
    let first = &seen.borrow()[0];
    assert!(first.contains("You are the Builder"), "{first}");
    assert!(first.contains("This is round 2, and you are a fresh session"), "{first}");
    assert!(first.contains(&f.state.review_path(1)), "{first}");
}

#[test]
fn lost_session_falls_back_to_a_fresh_one() {
    let seen = seen();
    let f = Fixture::resume(Rc::new(FakeHost::default()), saved_run("build", 2, FULL), build_and_approve(2, &seen), &[
        "review",
    ], &[("build", "s-gone")]);
    f.herdr.state().lost_sessions.insert("s-gone".into());
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    let args: Vec<Vec<String>> = starts(&f).into_iter().map(|(_, _, args)| args).collect();
    assert_eq!(args, [strings(&["--resume", "s-gone"]), vec![]]);
    assert!(seen.borrow()[0].contains("You are the Builder"));
    assert_ne!(wf.state().agents.get("build").unwrap().get("session").map(String::as_str), Some("s-gone"));
}

#[test]
fn fresh_reviewer_in_a_later_round_is_told_about_earlier_reviews() {
    let seen = seen();
    let f = Fixture::resume(Rc::new(FakeHost::default()), saved_run("review", 2, FULL), vec![(
        "review",
        vec![recording(review_turn(2, APPROVE), &seen)],
    )], &[], &[]);
    f.put(&f.state.build_path(2), "report 2");
    f.workflow(f.as_saved()).run().unwrap();
    let first = &seen.borrow()[0];
    assert!(first.contains("You are the Reviewer"), "{first}");
    assert!(first.contains(&format!("earlier reviews of this change are {}", f.state.review_path(1))), "{first}");
}

#[test]
fn review_already_written_is_not_asked_for() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), saved_run("review", 1, FULL), vec![], &[], &[]);
    f.put(&f.state.review_path(1), "VERDICT: APPROVE");
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!f.herdr.state().calls.iter().any(|c| matches!(c, Call::Prompt { .. } | Call::Start { .. })));
    assert_eq!(wf.state().phase, "done");
}

#[test]
fn base_comes_from_the_saved_state() {
    let seen = seen();
    let f = Fixture::resume(Rc::new(FakeHost::new(Some("moved"), "main")), saved_run("build", 1, &["spec"]), vec![
        ("build", vec![build_turn(1)]),
        ("review", vec![recording(review_turn(1, APPROVE), &seen)]),
    ], &[], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert!(seen.borrow()[0].contains("git diff abc123"));
    // A crash before the Builder was recorded: it is started in a split of the root pane.
    assert!(called(&f, &Call::Split { pane: "w1:p1".into(), direction: Direction::Right }));
}

#[test]
fn done_is_a_no_op() {
    let mut state = saved_run("done", 2, FULL);
    state.verdict = Some(CHANGES_REQUESTED.into());
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![], &[], &[]);
    f.put(&format!("{}/{SUMMARY_FILE}", f.state.dir()), "# summary\n");
    f.put(&format!("{}/{METRICS_FILE}", f.state.dir()), "{}\n");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(CHANGES_REQUESTED));
    assert_eq!(f.herdr.state().calls, []);
    assert!(f.host.state().writes.is_empty());
    assert_eq!(f.told.titles(), Vec::<String>::new());
}

/// A run of the default workflow that ended approved in round 1 with a pull request, as the first save of its ending
/// (Workflow::end_done_run) leaves it: done, still owned by the orchestrator that ended it, its Builder's turn not
/// yet given its tokens, and neither summary.md nor metrics.json written.
fn stopped_after_saving_done() -> RunState {
    let mut state = saved_run("done", 1, FULL);
    state.verdict = Some(APPROVE.into());
    state.pull_request = true;
    state.owner = Some(object([("pid", Value::from(4242)), ("host", Value::from("gone"))]));
    state.turns = vec![record("build", "build-1.md", Some(-100), Some(-50)).map()];
    state
}

/// stopped_after_saving_done as the ending's second save leaves it: released, the Builder's turn given tokens.
fn stopped_after_its_release() -> RunState {
    let mut state = stopped_after_saving_done();
    state.owner = None;
    state.turns[0].insert("tokens".into(), Value::from(7));
    state
}

/// The run's summary.md and metrics.json, as the host holds them now.
fn ending_files(f: &Fixture) -> (Option<String>, Option<String>) {
    let files = &f.host.state().files;
    let at = |name: &str| files.get(&format!("{}/{name}", f.state.dir())).cloned();
    (at(SUMMARY_FILE), at(METRICS_FILE))
}

/// The saved state.json's first turn's tokens.
fn first_tokens(f: &Fixture) -> Option<Value> {
    let saved = f.saved();
    saved.get("turns").and_then(Value::as_array).and_then(|turns| turns[0].get("tokens").cloned())
}

fn closed_w1(f: &Fixture) -> bool {
    called(f, &Call::Close { workspace: "w1".into() })
}

#[test]
fn a_run_stopped_right_after_saving_done_finishes_its_whole_ending_on_resume() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), stopped_after_saving_done(), vec![], &[], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(f.saved().get("owner"), Some(&Value::Null));
    assert_eq!(first_tokens(&f), Some(Value::from("unknown")));
    let (summary, metrics) = ending_files(&f);
    assert!(summary.is_some_and(|s| s.contains(APPROVE)));
    assert!(metrics.is_some());
    assert_eq!(f.told.titles(), ["Run finished: APPROVE"]);
    assert!(closed_w1(&f));
}

#[test]
fn a_run_stopped_after_its_release_writes_its_files_tells_the_human_and_closes_its_workspace() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), stopped_after_its_release(), vec![], &[], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(f.saved().get("owner"), Some(&Value::Null));
    assert_eq!(first_tokens(&f), Some(Value::from(7)), "the tokens are not counted again");
    let (summary, metrics) = ending_files(&f);
    assert!(summary.is_some() && metrics.is_some());
    assert_eq!(f.told.titles(), ["Run finished: APPROVE"]);
    assert!(closed_w1(&f));
}

#[test]
fn a_summary_an_earlier_stop_left_is_written_again_with_the_missing_metrics() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), stopped_after_its_release(), vec![], &[], &[]);
    f.put(&format!("{}/{SUMMARY_FILE}", f.state.dir()), "stopped: the Reviewer's pane died\n");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let (summary, metrics) = ending_files(&f);
    assert!(summary.is_some_and(|s| s.contains(APPROVE)), "the earlier stop's summary is replaced");
    assert!(metrics.is_some());
}

#[test]
fn a_run_missing_only_its_summary_writes_it_and_tells_the_human() {
    let mut state = stopped_after_its_release();
    state.pull_request = false;
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![], &[], &[]);
    f.put(&format!("{}/{METRICS_FILE}", f.state.dir()), "{\"kept\": true}\n");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let (summary, metrics) = ending_files(&f);
    assert!(summary.is_some());
    assert_eq!(metrics.as_deref(), Some("{\"kept\": true}\n"));
    assert_eq!(f.told.titles(), ["Run finished: APPROVE"]);
    assert!(!closed_w1(&f), "a run without a pull request keeps its workspace");
}

#[test]
fn invalid_review_is_refused_until_deleted() {
    // As a run that stopped because its retry was used left it.
    let mut state = saved_run("review", 1, FULL);
    state.retried = vec!["review-1.md".into()];
    state.error = Some("review-1.md does not start with a VERDICT line, and its one retry was already used".into());
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![("review", vec![review_turn(1, APPROVE)])], &[
        "review",
    ], &[]);
    f.put(&f.state.review_path(1), "looks fine");
    let message = failure(f.workflow(f.as_saved()).run());
    assert!(
        message.contains("review-1.md does not start with a VERDICT line, and its one retry was already used; fix it"),
        "{message}"
    );
    let saved = f.saved();
    assert_eq!(saved.get("prompted"), Some(&Value::Null));
    assert_eq!(prompts_to(&f, "review-a1b2c3"), 0);

    f.host.state().files.remove(&f.state.review_path(1));
    let seen = seen();
    let f2 = Fixture::resume(Rc::clone(&f.host), RunState::from_saved(&saved).unwrap(), vec![(
        "review",
        vec![recording(review_turn(1, APPROVE), &seen)],
    )], &["review"], &[]);
    let mut wf2 = f2.workflow(f2.as_saved());
    assert_eq!(wf2.run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f2, "review-a1b2c3"), 1);
    assert!(seen.borrow()[0].starts_with("You are the Reviewer"));
    assert_eq!(wf2.state().error, None);
}

#[test]
fn malformed_review_found_on_resume_is_retried() {
    let saved = prompted(saved_run("review", 1, FULL), "review-1.md").to_json().as_object().unwrap().clone();
    let seen = seen();
    let f = Fixture::resume(Rc::new(FakeHost::default()), RunState::from_saved(&saved).unwrap(), vec![(
        "review",
        vec![recording(review_turn(1, APPROVE), &seen)],
    )], &["review"], &[]);
    f.put(&f.state.review_path(1), "looks fine");
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(*seen.borrow(), [retry_prompt(&f.state.review_path(1))]);
    let rejected = format!("{}/review-1.rejected.md", f.state.dir());
    assert_eq!(f.host.state().files.get(&rejected).map(String::as_str), Some("looks fine"));
    assert_eq!(wf.state().retried, ["review-1.md"]);
}

/// A run that stopped after the Reviewer was given the retry prompt, before it wrote review-1.md again, on a host
/// holding the rejected review; the roles alive and in sessions as given, the Reviewer playing script.
fn retrying_run(state_with: impl FnOnce(&mut RunState), script: Vec<Turn>, alive: &[&str], sessions: &[(&str, &str)]) -> Fixture {
    let mut state = prompted(saved_run("review", 1, FULL), "review-1.md");
    state.retried = vec!["review-1.md".into()];
    state.retrying = Some("review-1.md".into());
    state_with(&mut state);
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![("review", script)], alive, sessions);
    f.put(&format!("{}/review-1.rejected.md", f.state.dir()), "looks fine");
    f
}

#[test]
fn live_reviewer_mid_retry_is_only_waited_for() {
    let f = retrying_run(|_| {}, vec![], &["review"], &[]);
    let (host, review) = (Rc::clone(&f.host), f.state.review_path(1));
    f.clock.hook(move |_| host.write(&review, &format!("VERDICT: {APPROVE}\n"), false).unwrap());
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f, "review-a1b2c3"), 0);
    assert_eq!(wf.state().retrying, None);
}

#[test]
fn resumed_reviewer_mid_retry_continues_it() {
    let seen = seen();
    let f = retrying_run(|_| {}, vec![recording(review_turn(1, APPROVE), &seen)], &[], &[("review", "s-review")]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(*seen.borrow(), [continue_prompt(&f.state.review_path(1))]);
}

#[test]
fn fresh_reviewer_mid_retry_gets_the_retry_prompt_and_no_second_retry() {
    let seen = seen();
    let f = retrying_run(|_| {}, vec![recording(malformed_review(1, "again"), &seen)], &[], &[]);
    let message = failure(f.workflow(f.as_saved()).run());
    assert!(message.contains("its one retry was already used"), "{message}");
    assert_eq!(*seen.borrow(), [retry_prompt(&f.state.review_path(1))]);
    let files = f.host.state().files.clone();
    assert_eq!(files.get(&format!("{}/review-1.rejected.md", f.state.dir())).map(String::as_str), Some("looks fine"));
    assert_eq!(files.get(&f.state.review_path(1)).map(String::as_str), Some("again"));
}

#[test]
fn reviewer_not_yet_given_the_retry_prompt_gets_it() {
    // Stopped between saving the retry and delivering its prompt.
    let seen = seen();
    let f = retrying_run(|s| s.prompted = None, vec![recording(review_turn(1, APPROVE), &seen)], &["review"], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(*seen.borrow(), [retry_prompt(&f.state.review_path(1))]);
}

#[test]
fn workspace_gone_opens_a_new_one() {
    let mut state = saved_run("build", 1, &["spec", "build"]);
    state.workspace_id = "w0".into();
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ], &[], &[]);
    {
        let mut h = f.herdr.state();
        h.workspaces.clear();
        h.live_panes.clear();
        h.panes = 0;
    }
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let workspaces = f.herdr.state().calls.iter().filter(|c| matches!(c, Call::Workspace { .. })).count();
    assert_eq!(workspaces, 1);
    let saved = f.saved();
    assert_eq!((text(&saved, "workspace_id"), text(&saved, "root_pane")), (Some("w1".into()), Some("w1:p1".into())));
    assert!(called(&f, &Call::Split { pane: "w1:p1".into(), direction: Direction::Right }));
    assert_eq!(names(&f, "start"), ["build-a1b2c3", "review-a1b2c3"]);
}

#[test]
fn pane_gone_splits_a_surviving_one() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), saved_run("review", 1, FULL), vec![(
        "review",
        vec![review_turn(1, APPROVE)],
    )], &[], &[]);
    f.put(&f.state.build_path(1), "report");
    {
        let mut h = f.herdr.state();
        h.live_panes.remove("w1:p2");
        h.live_panes.remove("w1:p3");
    }
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert!(called(&f, &Call::Split { pane: "w1:p1".into(), direction: Direction::Down }));
}

#[test]
fn recorded_reviewer_is_not_started_twice() {
    // start("review") saves before the review loop does, so round 1 can be saved as build with a reviewer.
    let f = Fixture::resume(Rc::new(FakeHost::default()), saved_run("build", 1, FULL), vec![(
        "review",
        vec![review_turn(1, APPROVE)],
    )], &["build", "review"], &[]);
    f.put(&f.state.build_path(1), "report");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert!(starts(&f).is_empty());
    assert_eq!(prompts_to(&f, "review-a1b2c3"), 1);
}
