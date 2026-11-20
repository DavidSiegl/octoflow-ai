//! A run picked up from its state.json, the agents found alive, resumed into their sessions or started afresh, each
//! prompted only for what it still owes.
//!
//! Not here: reading a state.json with missing, mistyped, unknown or later fields, which is crate::state's.

use std::cell::RefCell;

use super::pull_request::rounds;
use super::*;
use crate::constants::{METRICS_FILE, SUMMARY_FILE};
use crate::json::object;
use crate::pipeline::DEFAULT_WORKFLOW;
use crate::test_support::herdr_server;
use crate::workflow_file::workflow_definition;

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
    state.agents.insert("spec".to_string(), AgentRecord::new("spec-a1b2c3", "w1:p1", DEFAULT_HARNESS));
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![("build", vec![build_turn(1)]), ("review", vec![review_turn(1, APPROVE)])],
        &[],
        &[],
    );
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
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        prompted(saved_run("spec", 0, &["spec"]), "spec.md"),
        vec![("build", vec![build_turn(1)]), ("review", vec![review_turn(1, APPROVE)])],
        &["spec"],
        &[],
    );
    let (host, spec) = (Rc::clone(&f.host), f.state.spec_path());
    f.clock.hook(move |_| host.write(&spec, "# Goal\nx", false).unwrap());
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f, "spec-a1b2c3"), 0);
    assert!(!names(&f, "start").contains(&"spec-a1b2c3".to_string()));
    // The orchestrator that prompted it told the human already (run-crash-ending/03).
    assert!(!called(&f, &Call::Focus { name: "spec-a1b2c3".into() }));
    assert!(!f.told.titles().iter().any(|t| t.contains("is waiting for you")), "{:?}", f.told.titles());
}

#[test]
fn live_collector_never_prompted_is_prompted_once() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), saved_run("spec", 0, &["spec"]), approved(), &["spec"], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f, "spec-a1b2c3"), 1);
    assert!(called(&f, &Call::Focus { name: "spec-a1b2c3".into() }));
    assert_eq!(f.told.titles()[0], "Spec Collector is waiting for you");
}

#[test]
fn a_live_agent_saved_without_its_session_has_it_noted_before_its_turn_is_recorded() {
    // The orchestrator died between saving the Reviewer's agent and saving its session (Workflow::start).
    let state = prompted(saved_run("review", 1, FULL), "build-1.md");
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![("review", vec![taking(100, review_turn(1, APPROVE))])],
        &["review"],
        &[],
    );
    f.herdr.state().sessions.insert("review-a1b2c3".into(), "s-review".into());
    f.host.state().session_logs.insert(("claude".into(), "s-review".into()), claude_log(&[(50, 10, 20)]));
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!names(&f, "start").contains(&"review-a1b2c3".to_string()), "the live agent is not started again");
    let review = wf.state().turns.iter().find(|t| t.role == "review").expect("a review turn");
    assert_eq!(review.session.as_deref(), Some("s-review"));
    assert_eq!(review.tokens, serde_json::json!({"input": 10, "output": 20, "cache_read": 7, "cache_write": 1}));
}

#[test]
fn herdr_failing_to_look_up_a_known_agent_stops_the_run_without_starting_one() {
    // A skip in place of the failure would start a second Reviewer next to the live one.
    let state = prompted(saved_run("review", 1, FULL), "build-1.md");
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![("review", vec![review_turn(1, APPROVE)])],
        &["review"],
        &[],
    );
    f.herdr.state().agent_error = Some(crate::herdr::HerdrError::new("server_error", "herdr is down"));
    let e = failure(f.workflow(f.as_saved()).run());
    assert!(e.contains("herdr is down"), "{e}");
    assert!(names(&f, "start").is_empty(), "no agent is started: {:?}", names(&f, "start"));
}

#[test]
fn a_live_agent_in_another_session_than_saved_has_its_turn_recorded_in_that_one() {
    let state = prompted(saved_run("review", 1, FULL), "build-1.md");
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![("review", vec![review_turn(1, APPROVE)])],
        &["review"],
        &[("review", "s-old")],
    );
    f.herdr.state().sessions.insert("review-a1b2c3".into(), "s-now".into());
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    let review = wf.state().turns.iter().find(|t| t.role == "review").expect("a review turn");
    assert_eq!(review.session.as_deref(), Some("s-now"));
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
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        build_and_approve(2, &seen),
        &["review"],
        &[("build", "s-build")],
    );
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(starts(&f), [("build-a1b2c3".into(), "w1:p2".into(), strings(&["--resume", "s-build"]))]);
    assert_eq!(*seen.borrow(), [continue_prompt(&wf.state().build_path(2))]);
}

#[test]
fn resumed_builder_not_yet_prompted_gets_its_normal_prompt() {
    let seen = seen();
    let state = prompted(saved_run("build", 2, FULL), "review-1.md");
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        build_and_approve(2, &seen),
        &["review"],
        &[("build", "s-build")],
    );
    f.workflow(f.as_saved()).run().unwrap();
    assert!(seen.borrow()[0].starts_with("Changes were requested"), "{}", seen.borrow()[0]);
}

#[test]
fn a_built_in_workflow_resumes_on_the_definition_saved_with_the_run_not_on_this_engine_s_built_in() {
    // The run was started by a release whose default workflow asked the Builder again in other words.
    let mut definition = workflow_definition(&DEFAULT_WORKFLOW);
    let steps = definition["steps"].as_array_mut().unwrap();
    let build = steps.iter_mut().find(|step| step["id"] == "build").unwrap();
    build["again"] = Value::from("Mend the findings in {findings}; report to {build_path}.");
    let mut state = prompted(saved_run("build", 2, FULL), "review-1.md");
    state.workflow_definition = Some(definition.clone());
    let seen = seen();
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        build_and_approve(2, &seen),
        &["review"],
        &[("build", "s-build")],
    );

    f.workflow(f.as_saved()).run().unwrap();

    assert!(seen.borrow()[0].starts_with("Mend the findings in "), "{}", seen.borrow()[0]);
    assert_eq!(f.saved().get("workflow_definition"), Some(&definition));
}

#[test]
fn a_built_in_run_started_here_resumes_on_the_definition_it_saved_once_the_built_in_changed() {
    // An engine starts a run of the built-in default workflow, which stops in round 1 with changes requested.
    let f = Fixture::new(vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])]);
    f.play("review", vec![then_interrupted(&f.interrupt, review_turn(1, CHANGES_REQUESTED))]);
    assert_eq!(f.workflow(RunSettings::default()).run(), Err(Stopped::Interrupted));
    // A release whose default workflow asks the Builder again in other words resumes it: the saved definition is
    // that engine's built-in, edited here as the resuming engine's built-in differs from it.
    let mut saved = f.saved();
    let definition = saved.get_mut("workflow_definition").expect("a built-in run saves its definition");
    let steps = definition["steps"].as_array_mut().unwrap();
    let build = steps.iter_mut().find(|step| step["id"] == "build").unwrap();
    build["again"] = Value::from("Mend the findings in {findings}; report to {build_path}.");
    let seen = seen();
    let resumed = Fixture::resume(
        Rc::clone(&f.host),
        RunState::from_saved(&saved).unwrap(),
        build_and_approve(2, &seen),
        &[],
        &[],
    );

    resumed.workflow(resumed.as_saved()).run().unwrap();

    assert!(seen.borrow()[0].starts_with("Mend the findings in "), "{}", seen.borrow()[0]);
}

#[test]
fn a_built_in_workflow_saved_without_its_definition_resumes_on_this_engine_s_built_in_and_saves_it() {
    let mut state = prompted(saved_run("build", 2, FULL), "review-1.md");
    state.workflow_definition = None;
    let seen = seen();
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        build_and_approve(2, &seen),
        &["review"],
        &[("build", "s-build")],
    );

    f.workflow(f.as_saved()).run().unwrap();

    assert!(seen.borrow()[0].starts_with("Changes were requested"), "{}", seen.borrow()[0]);
    assert_eq!(f.saved().get("workflow_definition"), Some(&workflow_definition(&DEFAULT_WORKFLOW)));
}

/// The role's saved agent, marked as started the way start says and not yet prompted.
fn unprompted(mut state: RunState, role: &str, start: AgentStart) -> RunState {
    state.agents.get_mut(role).expect("a saved agent").unprompted = Some(start);
    state
}

#[test]
fn a_new_agent_stopped_in_its_startup_dialog_is_waited_out_then_prompted() {
    // The orchestrator stopped while the Spec Collector's harness asked whether to trust the folder (Workflow::start).
    let state = unprompted(saved_run("spec", 0, &["spec"]), "spec", AgentStart::New);
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, approved(), &["spec"], &[]);
    f.herdr.state().statuses.insert("spec-a1b2c3".into(), "blocked".into());
    let order = Rc::new(RefCell::new(vec![]));
    let (herdr, seen) = (Rc::clone(&f.herdr), Rc::clone(&order));
    f.herdr.state().wait_with = Some(Box::new(move |name, timeout_ms, until| {
        if until == ["idle", "done"] {
            seen.borrow_mut().push("startup");
            herdr.state().statuses.insert(name.to_string(), "idle".into());
        }
        herdr.own_wait(name, timeout_ms, until)
    }));
    let seen = Rc::clone(&order);
    f.herdr.state().before_call = Some(Box::new(move |c| {
        if matches!(c, Call::Prompt { .. }) {
            seen.borrow_mut().push("prompt");
        }
    }));
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(order.borrow()[..2], ["startup", "prompt"], "the prompt is not typed into the dialog");
    assert!(f.told.lines().contains(&"[spec] needs your answer before it starts (pane w1:p1)".into()));
    assert!(!names(&f, "start").contains(&"spec-a1b2c3".to_string()), "the live agent is not started again");
    assert_eq!(wf.state().agents.get("spec").unwrap().unprompted, None);
}

#[test]
fn a_resumed_agent_stopped_before_its_continue_prompt_is_told_to_continue() {
    // The orchestrator stopped after relaunching the Builder into its session, before the prompt went out.
    let seen = seen();
    let state = unprompted(prompted(saved_run("build", 2, FULL), "build-2.md"), "build", AgentStart::Resumed);
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        build_and_approve(2, &seen),
        &["build", "review"],
        &[("build", "s-build")],
    );
    f.herdr.state().statuses.insert("build-a1b2c3".into(), "idle".into());
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!names(&f, "start").contains(&"build-a1b2c3".to_string()), "the live agent is not started again");
    assert_eq!(*seen.borrow(), [continue_prompt(&wf.state().build_path(2))]);
}

#[test]
fn a_restarted_agent_stopped_before_its_prompt_gets_the_fresh_one() {
    let seen = seen();
    let state = unprompted(prompted(saved_run("build", 2, FULL), "build-2.md"), "build", AgentStart::Restarted);
    let script = build_and_approve(2, &seen);
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, script, &["build", "review"], &[]);
    f.herdr.state().statuses.insert("build-a1b2c3".into(), "idle".into());
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert!(!names(&f, "start").contains(&"build-a1b2c3".to_string()), "the live agent is not started again");
    let first = &seen.borrow()[0];
    assert!(first.contains("This is round 2, and you are a fresh session"), "{first}");
}

#[test]
fn a_relaunch_is_saved_marked_and_in_its_session_before_herdr_starts_it() {
    // herdr's agent start itself waits up to a minute on a startup dialog: a stop then must find the new agent's
    // record.
    let state = prompted(saved_run("build", 2, FULL), "build-2.md");
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        build_and_approve(2, &seen()),
        &["review"],
        &[("build", "s-build")],
    );
    let saved_at_start = Rc::new(RefCell::new(vec![]));
    let (host, path, seen) = (Rc::clone(&f.host), format!("{}/state.json", f.state.dir()), Rc::clone(&saved_at_start));
    f.herdr.state().before_call = Some(Box::new(move |c| {
        if matches!(c, Call::Start { .. }) {
            let saved = RunState::from_text(&host.read(&path).unwrap().expect("a saved state")).unwrap();
            seen.borrow_mut().push(saved.agents.get("build").cloned().expect("a saved Builder"));
        }
    }));
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let record = saved_at_start.borrow()[0].clone();
    assert_eq!((record.unprompted, record.session.as_deref()), (Some(AgentStart::Resumed), Some("s-build")));
}

#[test]
fn an_agent_that_exits_in_the_startup_dialog_a_resume_waits_out_is_started_again() {
    // As when the human refuses Claude Code's folder-trust question.
    let state = unprompted(saved_run("spec", 0, &["spec"]), "spec", AgentStart::New);
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, approved(), &["spec"], &[]);
    f.herdr.state().statuses.insert("spec-a1b2c3".into(), "blocked".into());
    let herdr = Rc::clone(&f.herdr);
    f.herdr.state().wait_with = Some(Box::new(move |name, timeout_ms, until| {
        if until == ["idle", "done"] && herdr.state().statuses.get(name).is_some_and(|s| s == "blocked") {
            herdr.state().statuses.remove(name);
            return Err(crate::herdr::HerdrError::new("agent_not_found", format!("agent target {name} not found")));
        }
        herdr.own_wait(name, timeout_ms, until)
    }));
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(names(&f, "start")[0], "spec-a1b2c3");
    assert!(f.told.titles().contains(&"Spec Collector is waiting for you".to_string()), "{:?}", f.told.titles());
}

#[test]
fn a_marked_agent_found_working_took_its_prompt_and_is_only_waited_on() {
    // The orchestrator stopped after the Builder took its prompt to continue, before it saved that.
    let state = unprompted(prompted(saved_run("build", 2, FULL), "build-2.md"), "build", AgentStart::Resumed);
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![("review", vec![review_turn(2, APPROVE)])],
        &["build", "review"],
        &[("build", "s-build")],
    );
    let (host, build) = (Rc::clone(&f.host), f.state.build_path(2));
    f.clock.hook(move |_| host.write(&build, "built", false).unwrap());
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 0);
    assert!(!f.told.lines().iter().any(|l| l.contains("needs your answer")), "{:?}", f.told.lines());
    assert_eq!(wf.state().agents.get("build").unwrap().unprompted, None);
}

#[test]
fn a_marked_agent_blocked_after_its_file_was_written_is_not_said_to_be_starting() {
    // The orchestrator stopped after relaunching the Builder, before the prompt went out; the human had it write
    // build-2.md in its pane. In round 3 it is blocked, which may be on what the human typed: the human is asked, with
    // no claim that it is starting, and it is prompted once it is answered.
    let state = unprompted(saved_run("build", 2, FULL), "build", AgentStart::Resumed);
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![
            ("review", vec![review_turn(2, CHANGES_REQUESTED), review_turn(3, APPROVE)]),
            ("build", vec![build_turn(3)]),
        ],
        &["build", "review"],
        &[("build", "s-build")],
    );
    f.put(&f.state.build_path(2), "built");
    f.herdr.state().statuses.insert("build-a1b2c3".into(), "blocked".into());
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!f.told.lines().iter().any(|l| l.contains("before it starts")), "{:?}", f.told.lines());
    assert!(f.told.lines().iter().any(|l| l.starts_with("[build] needs your answer (")), "{:?}", f.told.lines());
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 1);
    assert_eq!(wf.state().agents.get("build").unwrap().unprompted, None);
}

/// f's saved settings, its workflow quick with a human-paced Builder, whose fresh session is told of its earlier
/// reports.
fn interviewing_builder(f: &Fixture) -> RunSettings {
    use crate::pipeline::{Pipeline, Step};
    use crate::test_support::{again, roles};
    let mut build = again(
        Step::new("build", "build", "build-{n}.md", "Build {task} in {cwd}; report to {build_path}."),
        "Fix {prev_review_path}; report to {build_path}.",
    );
    (build.edits, build.human_paced) = (true, true);
    build.fresh_note = Some("Earlier reports: {earlier_build_paths}.".into());
    let mut review = again(
        Step::new("review", "review", "review-{n}.md", "Review {change} against {task}; write {review_path}."),
        "Recheck {build_path}; write {review_path}.",
    );
    review.loop_to = Some("build".into());
    let p = Pipeline::new("quick", roles(&[("build", "Builder"), ("review", "Reviewer")]), vec![build, review]);
    RunSettings { pipeline: Some(p.expect("a valid workflow")), ..f.as_saved() }
}

/// A saved quick run in round 2 whose Builder, started as start says, was never prompted, yet wrote build-2.md: the
/// human had it do so in its pane while no orchestrator ran. It loops back to round 3, recording the Builder's prompt in
/// seen; alive names the roles herdr finds running.
fn written_by_an_unprompted_builder(start: AgentStart, alive: &[&str], seen: &Rc<RefCell<Vec<String>>>) -> Fixture {
    let state = unprompted(saved_run("build", 2, &["build", "review"]), "build", start);
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![
            ("review", vec![review_turn(2, CHANGES_REQUESTED), review_turn(3, APPROVE)]),
            ("build", vec![recording(build_turn(3), seen)]),
        ],
        alive,
        &[],
    );
    f.put(&f.state.build_path(2), "built");
    f
}

#[test]
fn a_restarted_agent_past_its_startup_gets_the_fresh_prompt_and_is_not_said_to_have_restarted() {
    let seen = seen();
    let f = written_by_an_unprompted_builder(AgentStart::Restarted, &["build", "review"], &seen);
    f.herdr.state().statuses.insert("build-a1b2c3".into(), "idle".into());
    let mut wf = f.workflow(interviewing_builder(&f));
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(seen.borrow()[0].contains("Earlier reports:"), "{}", seen.borrow()[0]);
    assert!(f.told.titles().contains(&"Builder is waiting for you".to_string()), "{:?}", f.told.titles());
    assert!(!f.told.titles().iter().any(|t| t.contains("restarted")), "{:?}", f.told.titles());
}

#[test]
fn a_new_agent_past_its_startup_that_exited_lost_a_conversation_and_is_restarted() {
    let seen = seen();
    let f = written_by_an_unprompted_builder(AgentStart::New, &["review"], &seen);
    let mut wf = f.workflow(interviewing_builder(&f));
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(
        f.told.titles().contains(&"Builder restarted; the interview starts over".to_string()),
        "{:?}",
        f.told.titles()
    );
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
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        saved_run("build", 2, FULL),
        build_and_approve(2, &seen),
        &["review"],
        &[("build", "s-gone")],
    );
    f.herdr.state().lost_sessions.insert("s-gone".into());
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    let args: Vec<Vec<String>> = starts(&f).into_iter().map(|(_, _, args)| args).collect();
    assert_eq!(args, [strings(&["--resume", "s-gone"]), vec![]]);
    assert!(seen.borrow()[0].contains("You are the Builder"));
    assert_ne!(wf.state().agents.get("build").unwrap().session.as_deref(), Some("s-gone"));
}

#[test]
fn fresh_reviewer_in_a_later_round_is_told_about_earlier_reviews() {
    let seen = seen();
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        saved_run("review", 2, FULL),
        vec![("review", vec![recording(review_turn(2, APPROVE), &seen)])],
        &[],
        &[],
    );
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
    let f = Fixture::resume(
        Rc::new(FakeHost::new(Some("moved"), "main")),
        saved_run("build", 1, &["spec"]),
        vec![("build", vec![build_turn(1)]), ("review", vec![recording(review_turn(1, APPROVE), &seen)])],
        &[],
        &[],
    );
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert!(seen.borrow()[0].contains("git diff --no-relative abc123"));
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

/// stopped_after_saving_done with its tokens counted, as its ending leaves it from then until its release.
fn counted() -> RunState {
    let mut state = stopped_after_saving_done();
    state.turns[0].tokens = Value::from(7);
    state
}

/// counted, released: as its ending leaves it once whole, or an older engine's, which released first, before its
/// files.
fn released() -> RunState {
    let mut state = counted();
    state.owner = None;
    state
}

fn path(f: &Fixture, name: &str) -> String {
    format!("{}/{name}", f.state.dir())
}

/// The run's summary.md and metrics.json, as the host holds them now.
fn ending_files(f: &Fixture) -> (Option<String>, Option<String>) {
    let files = &f.host.state().files;
    (files.get(&path(f, SUMMARY_FILE)).cloned(), files.get(&path(f, METRICS_FILE)).cloned())
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
fn an_ending_writes_the_files_before_it_releases_the_run() {
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        saved_run("review", 1, FULL),
        vec![("review", vec![review_turn(1, APPROVE)])],
        &[],
        &[],
    );
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let writes = f.host.state().writes.clone();
    let last = |p: &str| writes.iter().rposition(|w| *w == p).unwrap_or_else(|| panic!("no write of {p}: {writes:?}"));
    let release = last(&path(&f, "state.json"));
    assert!(last(&path(&f, SUMMARY_FILE)) < release && last(&path(&f, METRICS_FILE)) < release, "{writes:?}");
    assert_eq!(f.saved().get("owner"), Some(&Value::Null));
}

#[test]
fn a_done_run_saved_without_its_definition_ends_without_one() {
    // Its record keeps what it ran with; this engine's built-in may have other steps.
    let mut state = stopped_after_saving_done();
    state.workflow_definition = None;
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![], &[], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(f.saved().get("workflow_definition").filter(|d| !d.is_null()), None);
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
fn a_run_done_without_a_verdict_tells_the_human_so() {
    let mut state = stopped_after_saving_done();
    state.verdict = None;
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, vec![], &[], &[]);
    assert_eq!(f.workflow(f.as_saved()).run(), Ok(None));
    assert_eq!(f.told.titles(), ["Run finished without a verdict"]);
}

#[test]
fn a_run_stopped_before_its_release_writes_its_files_again_and_tells_the_human() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), counted(), vec![], &[], &[]);
    // Its summary.md may be an earlier stop's (Workflow::release), as its metrics.json may be half written.
    f.put(&path(&f, SUMMARY_FILE), "stopped: the Reviewer's pane died\n");
    f.put(&path(&f, METRICS_FILE), "{\"run\"");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert_eq!(f.saved().get("owner"), Some(&Value::Null));
    assert_eq!(first_tokens(&f), Some(Value::from(7)), "the tokens are not counted again");
    let (summary, metrics) = ending_files(&f);
    assert!(summary.is_some_and(|s| s.contains(APPROVE)), "the earlier stop's summary is replaced");
    assert!(metrics.is_some_and(|m| serde_json::from_str::<Value>(&m).is_ok()));
    assert_eq!(f.told.titles(), ["Run finished: APPROVE"]);
    assert!(closed_w1(&f));
}

#[test]
fn a_released_run_missing_a_file_has_it_written_and_nothing_else() {
    let f = Fixture::resume(Rc::new(FakeHost::default()), released(), vec![], &[], &[]);
    f.put(&path(&f, METRICS_FILE), "{\"kept\": true}\n");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let (summary, metrics) = ending_files(&f);
    assert!(summary.is_some_and(|s| s.contains(APPROVE)));
    assert_eq!(metrics.as_deref(), Some("{\"kept\": true}\n"));
    assert_eq!(f.host.state().writes, [path(&f, SUMMARY_FILE)], "neither saved nor its metrics written");
    assert_eq!(f.told.titles(), Vec::<String>::new(), "the human was told when it was released");
    assert!(!closed_w1(&f));
}

#[test]
fn invalid_review_is_refused_until_deleted() {
    // As a run that stopped because its retry was used left it.
    let mut state = saved_run("review", 1, FULL);
    state.retried = vec!["review-1.md".into()];
    state.error = Some("review-1.md does not start with a VERDICT line, and its one retry was already used".into());
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![("review", vec![review_turn(1, APPROVE)])],
        &["review"],
        &[],
    );
    f.put(&f.state.review_path(1), "looks fine");
    let message = failure(f.workflow(f.as_saved()).run());
    assert!(
        message.contains("review-1.md does not start with a VERDICT line, and its one retry was already used; fix it"),
        "{message}"
    );
    let saved = f.saved();
    assert_eq!(saved.get("prompted"), Some(&Value::Null));
    assert_eq!(prompts_to(&f, "review-a1b2c3"), 0);

    f.host.state().files.shift_remove(&f.state.review_path(1));
    let seen = seen();
    let f2 = Fixture::resume(
        Rc::clone(&f.host),
        RunState::from_saved(&saved).unwrap(),
        vec![("review", vec![recording(review_turn(1, APPROVE), &seen)])],
        &["review"],
        &[],
    );
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
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        RunState::from_saved(&saved).unwrap(),
        vec![("review", vec![recording(review_turn(1, APPROVE), &seen)])],
        &["review"],
        &[],
    );
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
fn retrying_run(
    state_with: impl FnOnce(&mut RunState),
    script: Vec<Turn>,
    alive: &[&str],
    sessions: &[(&str, &str)],
) -> Fixture {
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
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        state,
        vec![("build", vec![build_turn(1)]), ("review", vec![review_turn(1, APPROVE)])],
        &[],
        &[],
    );
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
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        saved_run("review", 1, FULL),
        vec![("review", vec![review_turn(1, APPROVE)])],
        &[],
        &[],
    );
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
    let f = Fixture::resume(
        Rc::new(FakeHost::default()),
        saved_run("build", 1, FULL),
        vec![("review", vec![review_turn(1, APPROVE)])],
        &["build", "review"],
        &[],
    );
    f.put(&f.state.build_path(1), "report");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    assert!(starts(&f).is_empty());
    assert_eq!(prompts_to(&f, "review-a1b2c3"), 1);
}

/// The herdr server whose session is session, its socket path unset.
fn session(session: &str) -> HerdrServer {
    herdr_server(None, Some(session))
}

/// A run saved in phase spec in herdr server saved, its spec approved, resumed by a fake herdr reaching current.
fn resumed_from(saved: Option<HerdrServer>, current: Option<HerdrServer>) -> Fixture {
    let mut state = saved_run("spec", 0, &["spec"]);
    state.herdr_server = saved;
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, approved(), &[], &[]);
    f.herdr.state().server = current;
    f
}

/// Whether f's herdr was asked to open or close a workspace.
fn opened_or_closed(f: &Fixture) -> bool {
    f.herdr.state().calls.iter().any(|c| matches!(c, Call::Workspace { .. } | Call::Close { .. }))
}

#[test]
fn a_new_run_saves_the_herdr_server_it_opens_its_workspace_in() {
    let f = Fixture::new(rounds(&[APPROVE]));
    f.herdr.state().server = Some(session("work"));
    let mut wf = f.workflow(RunSettings::default());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(wf.state().herdr_server, Some(session("work")));
    // A herdr forwarding to a machine reaches none of this machine's.
    let f = Fixture::new(rounds(&[APPROVE]));
    let mut wf = f.workflow(RunSettings::default());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(wf.state().herdr_server, None);
}

#[test]
fn a_resume_from_another_herdr_server_closes_the_old_workspace_there_and_opens_one_here() {
    // Started in the app's session, resumed from a shell in another, which has a w1 of its own that must not be
    // taken for the run's.
    let f = resumed_from(Some(session("octoflow")), Some(session("work")));
    f.herdr.state().foreign_workspaces.insert((session("octoflow"), "w1".into()));
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    let reached = f.herdr.state().reached.clone();
    assert_eq!(
        reached[..2],
        [("w1".to_string(), Some(session("octoflow"))), ("w1".to_string(), Some(session("octoflow")))]
    );
    assert!(called(&f, &Call::Close { workspace: "w1".into() }));
    assert!(f.herdr.state().foreign_workspaces.is_empty());
    assert!(f.herdr.state().calls.iter().any(|c| matches!(c, Call::Workspace { .. })));
    assert_eq!(wf.state().herdr_server, Some(session("work")));
}

#[test]
fn a_workspace_gone_from_the_run_s_herdr_server_is_not_found_in_another() {
    // w1 is open only in the resuming server: another run's, which the resume must not take for its own.
    let f = resumed_from(Some(session("octoflow")), Some(session("work")));
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(f.herdr.state().reached[0], ("w1".to_string(), Some(session("octoflow"))));
    assert!(!called(&f, &Call::Close { workspace: "w1".into() }));
    assert!(f.told.lines().contains(&"herdr workspace w1 is gone; opening a new one".into()));
    assert!(f.herdr.state().calls.iter().any(|c| matches!(c, Call::Workspace { .. })));
    assert_eq!(wf.state().herdr_server, Some(session("work")));
}

#[test]
fn the_run_s_herdr_server_named_another_way_is_the_same_server() {
    // Started from a plain terminal (default server), resumed from a pane of that server, whose HERDR_SOCKET_PATH
    // names it by its socket.
    let pane = herdr_server(Some("/h/herdr.sock"), None);
    let f = resumed_from(Some(herdr_server(None, None)), Some(pane.clone()));
    f.herdr.state().aliases.push(herdr_server(None, None));
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!opened_or_closed(&f), "{:?}", f.herdr.state().calls);
    assert_eq!(wf.state().herdr_server, Some(herdr_server(None, None)));
}

#[test]
fn a_resume_forwarded_to_the_run_s_machine_goes_on_in_its_workspace() {
    // Started on box, resumed from a laptop with --machine box: the forwarded herdr reaches no server of this
    // machine's, and box's own picks the one the run is in.
    let f = resumed_from(Some(session("work")), None);
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!opened_or_closed(&f), "{:?}", f.herdr.state().calls);
}

#[test]
fn a_run_that_saved_no_herdr_server_saves_the_one_it_resumes_in() {
    let f = resumed_from(None, Some(session("work")));
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!opened_or_closed(&f), "{:?}", f.herdr.state().calls);
    assert_eq!(wf.state().herdr_server, Some(session("work")));
}

#[test]
fn a_workspace_that_cannot_be_closed_in_another_server_is_left_there() {
    let f = resumed_from(Some(session("octoflow")), Some(session("work")));
    f.herdr.state().foreign_workspaces.insert((session("octoflow"), "w1".into()));
    f.herdr.state().close_error = Some(crate::herdr::HerdrError::new("server_not_running", "down"));
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    let line = "cannot close herdr workspace w1: herdr server_not_running: down";
    assert!(f.told.lines().contains(&line.into()), "{:?}", f.told.lines());
    assert_eq!(wf.state().herdr_server, Some(session("work")));
}

#[test]
fn a_resume_from_the_run_s_own_herdr_server_goes_on_in_its_workspace() {
    let mut state = saved_run("spec", 0, &["spec"]);
    state.herdr_server = Some(session("work"));
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, approved(), &["spec"], &[]);
    f.herdr.state().server = Some(session("work"));
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!opened_or_closed(&f), "{:?}", f.herdr.state().calls);
    assert_eq!(wf.state().herdr_server, Some(session("work")));
}
