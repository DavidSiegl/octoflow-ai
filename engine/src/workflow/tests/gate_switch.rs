//! TestQualityGateOnResume and TestQualityGateOffOnResume: resume's --quality-gate and --no-quality-gate, the quality
//! gate of a stopped run turned on or off (turn_quality_gate_on, turn_quality_gate_off) and the run then resumed.
//!
//! Not here: test_the_cli_refuses_with_exit_1_and_leaves_state_json_alone and
//! test_the_cli_runs_the_workflow_with_the_new_job, which drive main and are ported in engine/src/cli/tests.rs; and
//! test_show_names_the_gate, as the engine has no `show` yet.

use std::cell::RefCell;

use super::*;
use crate::host::Exec;
use crate::local_gate::LOCAL_GATE;
use crate::constants::{DEFAULT_MAX_QUALITY_ROUNDS, FINISHED};
use crate::pipeline::{Pipeline, Step, DEFAULT_WORKFLOW, PUBLISH, QUALITY};
use crate::prompts::{QUALITY_OFF_NOTE, QUALITY_PASSED_NOTE, QUALITY_STUCK_NOTE, QUALITY_UNRESOLVED_NOTE};
use crate::workflow::{turn_quality_gate_off, turn_quality_gate_on};
use crate::json::object;

/// The branch of pr_run's change.
const SWITCH_BRANCH: &str = "orchestrator/add-a-token-bucket-rate-limiter-a1b2c3";

/// The pull request's notes for a final change that resume --no-quality-gate left unjudged (pr_body).
const OFF_AFTER_ANALYSIS: &str = "> [!NOTE]\n> The quality gate was turned off (`resume --no-quality-gate`) after its \
                                  last analysis, and the Builder changed the code since, so nothing judged the final \
                                  change.";
const OFF_BEFORE_ANALYSIS: &str = "> [!NOTE]\n> The quality gate was turned off (`resume --no-quality-gate`) before \
                                   any analysis finished, so nothing judged the change.";

/// saved_run of the default workflow for a run that ends in a pull request from SWITCH_BRANCH into main.
fn pr_run(phase: &str, rnd: u32) -> RunState {
    let mut state = saved_run(phase, rnd, FULL);
    state.pull_request = true;
    state.base_branch = Some("main".into());
    state.branch = Some(SWITCH_BRANCH.into());
    state
}

/// pr_run with the quality gate's job, in quality round q.
fn gated_pr_run(phase: &str, q: u32) -> RunState {
    let mut state = pr_run(phase, 1);
    state.quality_job = Some(LOCAL_GATE.into());
    state.quality_round = q;
    state
}

/// A FakeHost on pr_run's branch, which already holds a commit of the Builder's.
fn pr_host() -> Rc<FakeHost> {
    Rc::new(FakeHost::new(Some("def456"), SWITCH_BRANCH))
}

/// turn_quality_gate_on of state with the local gate on the default workflow, its preflight on host passing unless
/// host's run_command answers it: the lines it logged, or its error.
fn turn_on(state: &mut RunState, host: &FakeHost) -> Result<Vec<String>, OrchestratorError> {
    if host.state().run_command.is_none() {
        host.state().run_command = Some(Box::new(|_, argv| {
            let exec = Exec { command: argv.join(" "), status: 0, stdout: Vec::new(), stderr: Vec::new() };
            (argv[..2] == ["sh", "-c"] && argv[2].contains("mkdir -p")).then_some(Ok(exec))
        }));
    }
    let told = Told::default();
    turn_quality_gate_on(state, &DEFAULT_WORKFLOW, host, &told)?;
    Ok(told.lines())
}

/// The job directories of the local gate's runs in f, in order.
fn judged(f: &Fixture) -> Vec<String> {
    f.gate.borrow().jobs.iter().map(|job| job[0].clone()).collect()
}

/// The job directory of quality round q of round n's first gate run in f's run.
fn job(f: &Fixture, n: u32, q: u32) -> String {
    format!("{}/job-1", f.state.gate_dir(n, q))
}

/// turn_quality_gate_off of state on pipeline, on host and a fixture's other fakes: the lines it logged, or its error.
fn turn_off(state: &mut RunState, pipeline: &Pipeline, host: &Rc<FakeHost>) -> Result<Vec<String>, OrchestratorError> {
    let f = Fixture::on(Rc::clone(host), state.clone(), vec![]);
    turn_quality_gate_off(state, pipeline, &f.collaborators())?;
    Ok(f.told.lines())
}


/// The arguments of the commit the run made.
fn commit_args(f: &Fixture) -> Vec<String> {
    f.host.state().git_calls.iter().find(|c| c[0] == "commit").cloned().expect("a commit")
}

/// The script of a resume in which the Reviewer approves round 1, recording its prompt in reviews.
fn approving(reviews: &Rc<RefCell<Vec<String>>>) -> Script {
    vec![("review", vec![recording(review_turn(1, APPROVE), reviews)])]
}

/// The body of the run's pull request.
fn pr_body(f: &Fixture) -> String {
    f.host.state().prs.first().expect("a pull request").body.clone()
}

/// The one-step gated workflow gsolo, which has no verdict step.
fn gsolo() -> Pipeline {
    let mut step = Step::new("impl", "impl", "impl-{n}.md", "Implement {task}; report to {path}.");
    (step.edits, step.quality_gated) = (true, true);
    Pipeline::new("gsolo", crate::test_support::roles(&[("impl", "Implementer")]), vec![step])
        .expect("gsolo is a valid workflow")
}

/// A run of gsolo saved in phase and quality round 1 of round 1, ending in a pull request from SWITCH_BRANCH.
fn gsolo_run(phase: &str) -> RunState {
    let mut state = saved_run(phase, 1, &[]);
    state.pull_request = true;
    state.base_branch = Some("main".into());
    state.branch = Some(SWITCH_BRANCH.into());
    state.workflow = "gsolo".into();
    state.quality_job = Some(LOCAL_GATE.into());
    state.quality_round = 1;
    state
}

/// The Implementer's answer to quality round q of round n of gsolo: it changes /proj/limiter.py and reports.
fn gsolo_turn(n: u32, q: u32) -> Turn {
    Box::new(move |_, state, host| {
        host.write("/proj/limiter.py", &format!("version {n}.{q}"), false).expect("the fake host writes");
        let path = gsolo().steps()[0].path(&state.dir(), n, q);
        host.write(&path, &format!("impl {n} q{q}"), false).expect("the fake host writes");
        "done".to_string()
    })
}

// -- TestQualityGateOnResume -------------------------------------------------------------------------------------

#[test]
fn on_in_the_builders_step_gates_its_turn() {
    let mut state = pr_run("build", 1);
    let host = pr_host();
    turn_on(&mut state, &host).expect("the gate turns on");
    assert_eq!((state.phase.as_str(), state.quality_job.as_deref(), state.quality_round), ("build", Some(LOCAL_GATE), 0));

    let reviews = seen();
    let script = vec![
        ("build", vec![build_turn(1), fix_turn(1, 1)]),
        ("review", vec![recording(review_turn(1, APPROVE), &reviews)]),
    ];
    let f = Fixture::resume(host, state, script, &[], &[]).gated();
    f.judging(vec![red()]);
    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));
    assert_eq!(judged(&f), [job(&f, 1, 1), job(&f, 1, 2)]);
    let review = &reviews.borrow()[0];
    assert!(review.contains(&format!("The Builder's report is in {}", f.state.quality_build_path(1, 1))), "{review}");
    assert!(review.contains(&quality_note(QUALITY_PASSED_NOTE, &f.state.quality_path(1, 2))), "{review}");
    assert_eq!(text(&f.saved(), "quality_job").as_deref(), Some(LOCAL_GATE));
}

#[test]
fn on_in_the_reviewers_step_judges_the_builders_report_before_the_review() {
    let mut state = pr_run("review", 1);
    let host = pr_host();
    turn_on(&mut state, &host).expect("the gate turns on");
    assert_eq!((state.phase.as_str(), state.quality_round), (QUALITY, 1));

    let reviews = seen();
    let script = approving(&reviews);
    let f = Fixture::resume(host, state, script, &[], &[]).gated();
    f.put(&f.state.build_path(1), "report 1");
    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));
    assert_eq!(judged(&f), [job(&f, 1, 1)]);
    assert_eq!(f.host.state().snapshots[0].0, "abc123");
    let reviews = reviews.borrow();
    assert_eq!(reviews.len(), 1);
    assert!(reviews[0].contains(&format!("The Builder's report is in {}", f.state.build_path(1))), "{}", reviews[0]);
    assert!(reviews[0].contains(&quality_note(QUALITY_PASSED_NOTE, &f.state.quality_path(1, 1))), "{}", reviews[0]);
}

#[test]
fn on_with_the_local_gate_runs_its_preflight_and_needs_no_origin() {
    let mut state = pr_run("review", 1);
    let host = pr_host();
    host.state().has_origin = false;
    let asked = seen();
    let recorded = Rc::clone(&asked);
    host.state().run_command = Some(Box::new(move |_, argv| {
        recorded.borrow_mut().push(argv[0].to_string());
        let exec = crate::host::Exec { command: argv.join(" "), status: 0, stdout: Vec::new(), stderr: Vec::new() };
        Some(Ok(exec))
    }));
    let told = Told::default();
    turn_quality_gate_on(&mut state, &DEFAULT_WORKFLOW, &*host, &told).expect("the gate turns on");
    assert_eq!(*asked.borrow(), ["sh"]);
    assert_eq!((state.phase.as_str(), state.quality_job.as_deref(), state.quality_round), (QUALITY, Some("local"), 1));
    assert_eq!(told.lines(), [format!("turned the quality gate of run {RUN_ID} on, with local")]);
}

#[test]
fn on_with_the_reviewer_prompted_gates_the_next_builder_turn() {
    let mut state = pr_run("review", 1);
    state.prompted = Some("review-1.md".into());
    let host = pr_host();
    turn_on(&mut state, &host).expect("the gate turns on");
    assert_eq!((state.phase.as_str(), state.quality_round), ("review", 0));

    let script = vec![("build", vec![build_turn(2)]), ("review", vec![review_turn(2, APPROVE)])];
    let f = Fixture::resume(host, state, script, &["review"], &[]).gated();
    f.put(&f.state.build_path(1), "report 1");
    f.put(&f.state.review_path(1), &format!("VERDICT: {CHANGES_REQUESTED}\n1. x"));
    assert_eq!(f.workflow(f.as_saved_gated()).run(), verdict(APPROVE));
    assert_eq!(judged(&f), [job(&f, 2, 1)]);
    assert!(f.host.state().files.get(&f.state.quality_path(1, 1)).is_none());
}

#[test]
fn on_with_the_reviewer_retried_or_blocked_gates_the_next_builder_turn() {
    // Both have seen the change: one is asked again for the verdict it wrote, the other waits on its answer.
    let mut retried = pr_run("review", 1);
    retried.retrying = Some("review-1.md".into());
    let mut blocked = pr_run("review", 1);
    let question = [("file", Value::from("review-1.md")), ("question", "which?".into()), ("k", 1.into())];
    blocked.blocked = Some(object(question));
    for (name, mut state) in [("retried", retried), ("blocked", blocked)] {
        turn_on(&mut state, &pr_host()).expect("the gate turns on");
        assert_eq!(
            (state.phase.as_str(), state.quality_round, state.quality_job.as_deref()),
            ("review", 0, Some(LOCAL_GATE)),
            "{name}"
        );
    }
}

#[test]
fn on_in_round_0_is_run_quality_gate() {
    let mut state = saved_run("spec", 0, &[]);
    state.pull_request = true;
    // In round 0 the preflight waits for the run, before the interview.
    let unasked = FakeHost::default();
    unasked.state().run_command = Some(Box::new(|_, argv| panic!("ran {argv:?}")));
    turn_on(&mut state, &unasked).expect("the gate turns on");
    assert_eq!(state.quality_job.as_deref(), Some(LOCAL_GATE));

    let script = vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ];
    let f = Fixture::on(Rc::new(FakeHost::default()), state.clone(), script).gated();
    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    assert_eq!(judged(&f), [job(&f, 1, 1)]);

    // And its preflight, before the interview.
    let host = Rc::new(FakeHost::default());
    host.state().run_command = Some(Box::new(|_, argv| {
        let (command, stderr) = (argv.join(" "), b"curl is not on PATH\n".to_vec());
        Some(Ok(Exec { command, status: 1, stdout: Vec::new(), stderr }))
    }));
    let mut f = Fixture::on(host, state, vec![("spec", vec![spec_turn()])]);
    f.quality_gate_on = true;
    let message = failure(f.workflow(RunSettings::default()).run());
    assert!(message.contains("curl is not on PATH"), "{message}");
    assert!(!called(&f, &Call::Prompt { name: "spec-a1b2c3".into() }));
}

#[test]
fn an_in_place_no_pr_run_can_turn_it_on_in_round_0_only() {
    let mut state = saved_run("spec", 0, &[]);
    turn_on(&mut state, &pr_host()).expect("the gate turns on");
    assert_eq!(state.quality_job.as_deref(), Some(LOCAL_GATE));
    // Round 0 checks the tree is clean before the interview, as run --quality-gate does.
    let host = Rc::new(FakeHost::default());
    host.write("/proj/mine.py", "my own change", false).expect("the fake host writes");
    let f = Fixture::on(host, state, vec![("spec", vec![spec_turn()])]).gated();
    let message = failure(f.workflow(no_pr()).run());
    assert!(message.contains("uncommitted changes"), "{message}");
}

#[test]
fn refusals_leave_the_state_as_it_was() {
    let mut done = pr_run("done", 1);
    done.verdict = Some(APPROVE.into());
    let mut done_same_job = done.clone();
    done_same_job.quality_job = Some(LOCAL_GATE.into());
    let mut turned_off = pr_run("build", 1);
    turned_off.quality_gate_disabled = true;
    let cases = [
        ("done", done, "is in phase done, past its last turn"),
        ("publish", pr_run("publish", 1), "is in phase publish, past its last turn"),
        ("done, the same job", done_same_job, "is in phase done, past its last turn"),
        ("turned off", turned_off, "turned off with --no-quality-gate, which cannot be undone"),
        ("in place", saved_run("build", 1, &["spec", "build"]), "is a --no-pr run in /proj itself"),
    ];
    for (name, mut state, why) in cases {
        let before = state.to_json();
        let unasked = pr_host();
        unasked.state().run_command = Some(Box::new(|_, argv| panic!("ran {argv:?}")));
        let refused = turn_on(&mut state, &unasked).expect_err(name);
        assert!(refused.0.contains(why), "{name}: {}", refused.0);
        assert_eq!(state.to_json(), before, "{name}");
    }

    let mut state = saved_run("build", 1, &["build"]);
    (state.pull_request, state.workflow) = (true, "solo".into());
    let before = state.to_json();
    let told = Told::default();
    let refused = turn_quality_gate_on(&mut state, &crate::test_support::solo(), &*pr_host(), &told)
        .expect_err("solo has no gated step");
    assert!(
        refused.0.contains(
            "is of workflow solo, which has no quality-gated step, so its quality gate cannot be turned on"
        ),
        "{}",
        refused.0
    );
    assert_eq!(state.to_json(), before);
}

#[test]
fn the_same_job_again_changes_nothing() {
    let mut state = gated_pr_run("review", 1);
    let before = state.to_json();
    let unasked = pr_host();
    unasked.state().run_command = Some(Box::new(|_, argv| panic!("ran {argv:?}")));
    let logged = turn_on(&mut state, &unasked).expect("the same gate is no error");
    assert_eq!(state.to_json(), before);
    assert!(logged.iter().any(|l| l.contains(&format!("already has quality gate {LOCAL_GATE}"))), "{logged:?}");
}

#[test]
fn a_failed_preflight_refuses_before_any_turn() {
    let no_gitleaks = pr_host();
    no_gitleaks.state().gitleaks_missing = true;
    let no_curl = pr_host();
    no_curl.state().run_command = Some(Box::new(|_, argv| {
        let (command, stderr) = (argv.join(" "), b"curl is not on PATH\n".to_vec());
        Some(Ok(Exec { command, status: 1, stdout: Vec::new(), stderr }))
    }));
    let cases =
        [("no gitleaks", no_gitleaks, "every push is scanned for secrets first"), ("no curl", no_curl, "curl is not on PATH")];
    for (name, host, why) in cases {
        let mut state = pr_run("build", 1);
        let before = state.to_json();
        let refused = turn_on(&mut state, &host).expect_err(name);
        assert!(refused.0.contains(why), "{name}: {}", refused.0);
        assert_eq!(state.to_json(), before, "{name}");
    }
}

// -- TestQualityGateOffOnResume ----------------------------------------------------------------------------------

#[test]
fn off_in_quality_drops_the_analysis_and_goes_to_the_reviewer() {
    let mut state = gated_pr_run("quality", 2);
    let analysis = record(QUALITY, "quality-1-2.md", Some(0), None).with("quality_round", 2).with("agent", Value::Null);
    state.turns = vec![analysis.map()];
    let host = pr_host();
    turn_off(&mut state, &DEFAULT_WORKFLOW, &host).expect("the gate turns off");

    assert_eq!(state.quality_job, None);
    assert!(state.quality_gate_disabled);
    assert_eq!(state.phase, "review");
    assert!(state.turns.is_empty());

    let reviews = seen();
    let f = Fixture::resume(host, state, approving(&reviews), &[], &[]);
    f.put(&f.state.build_path(1), "report 1");
    f.put(&f.state.quality_path(1, 1), "GATE: ERROR\n\n1. x\n");
    f.put(&f.state.quality_build_path(1, 1), "report 1 q1");
    f.host.state().changed.insert("/proj/limiter.py".into());
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let review = &reviews.borrow()[0];
    assert!(review.contains(&format!("The Builder's report is in {}", f.state.quality_build_path(1, 1))), "{review}");
    assert!(review.contains(&filled(QUALITY_OFF_NOTE, &[("quality_paths", &f.state.quality_path(1, 1))])), "{review}");
    assert!(!review.contains(&QUALITY_UNRESOLVED_NOTE[..30]), "{review}");
    assert!(f.host.state().snapshots.is_empty());
    let body = pr_body(&f);
    assert!(body.contains(&format!("{OFF_AFTER_ANALYSIS}\n\nOpened by")), "{body}");
    assert!(
        body.contains("<summary>Quality gate (round 1, before the last change)</summary>\n\nGATE: ERROR\n\n1. x\n"),
        "{body}"
    );
    let commit = commit_args(&f);
    let trailer = ["--trailer", "Orchestrator-Gate: ERROR after 1 quality round, then turned off"];
    assert_eq!(commit[commit.len() - 2..], trailer);
}

/// Whether body's Orchestrator-Gate row gives a gate result after noted quality rounds: Python's regex
/// `\| Orchestrator-Gate \| `\w+ after {noted} quality rounds?` \|`.
fn gate_row_after(body: &str, noted: u32) -> bool {
    body.lines().any(|line| {
        let Some(cell) = line.strip_prefix("| Orchestrator-Gate | `").and_then(|l| l.strip_suffix("` |")) else {
            return false;
        };
        let Some((gate, rest)) = cell.split_once(' ') else { return false };
        let word = !gate.is_empty() && gate.chars().all(|c| c.is_alphanumeric() || c == '_');
        let rounds = format!("after {noted} quality round");
        word && (rest == rounds || rest == format!("{rounds}s"))
    })
}

/// The quality files a case puts in the run's directory: each one's path, for the run's state, and its text.
type QualityFiles = Vec<(fn(&RunState) -> String, String)>;

#[test]
fn off_in_quality_tells_the_reviewer_what_the_round_last_found() {
    let red_quality = red().quality;
    let cases: Vec<(&str, u32, QualityFiles, Option<&str>, u32)> = vec![
        ("first analysis unfinished", 1, vec![], None, 0),
        (
            "analysis finished, passed",
            1,
            vec![(|s| s.quality_path(1, 1), "GATE: OK\n".into())],
            Some(QUALITY_PASSED_NOTE),
            1,
        ),
        (
            "analysis finished, failed",
            1,
            vec![(|s| s.quality_path(1, 1), "GATE: ERROR\n\n1. x\n".into())],
            Some(QUALITY_UNRESOLVED_NOTE),
            1,
        ),
        (
            "stuck",
            2,
            vec![
                (|s| s.quality_path(1, 1), red_quality.clone()),
                (|s| s.quality_build_path(1, 1), "report 1 q1".into()),
                (|s| s.quality_path(1, 2), red_quality.clone()),
            ],
            Some(QUALITY_STUCK_NOTE),
            2,
        ),
    ];
    for (name, q, files, note, noted) in cases {
        let mut state = gated_pr_run("quality", q);
        let host = pr_host();
        turn_off(&mut state, &DEFAULT_WORKFLOW, &host).expect("the gate turns off");
        let reviews = seen();
        let f = Fixture::resume(host, state, approving(&reviews), &[], &[]);
        f.put(&f.state.build_path(1), "report 1");
        for (path_of, text) in &files {
            f.put(&path_of(&f.state), text);
        }
        f.host.state().changed.insert("/proj/limiter.py".into());
        assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE), "{name}");
        let review = &reviews.borrow()[0];
        assert!(review.contains(&format!("The Builder's report is in {}", f.state.last_report_path(1))), "{name}");
        let body = pr_body(&f);
        let commit_and_body = format!("{} {body}", commit_args(&f).join(" "));
        match note {
            None => {
                assert!(!review.to_lowercase().contains("quality gate"), "{name}: {review}");
                assert!(!body.contains("Quality gate"), "{name}: {body}");
                assert!(body.contains(&format!("{OFF_BEFORE_ANALYSIS}\n\nOpened by")), "{name}: {body}");
                assert!(!commit_and_body.contains("Orchestrator-Gate"), "{name}: {commit_and_body}");
            }
            Some(note) => {
                assert!(review.contains(&quality_note(note, &f.state.quality_path(1, noted))), "{name}: {review}");
                assert!(body.contains("<summary>Quality gate (round 1)</summary>"), "{name}: {body}");
                assert!(!body.contains("[!NOTE]"), "{name}: {body}");
                assert!(!commit_and_body.contains("then turned off"), "{name}: {commit_and_body}");
                assert!(gate_row_after(&body, noted), "{name}: {body}");
            }
        }
    }
}

#[test]
fn off_in_round_1_and_published_in_round_2_cites_round_1() {
    let mut state = gated_pr_run("review", 2);
    let host = pr_host();
    turn_off(&mut state, &DEFAULT_WORKFLOW, &host).expect("the gate turns off");
    let script = vec![
        ("review", vec![review_turn(1, CHANGES_REQUESTED), review_turn(2, APPROVE)]),
        ("build", vec![build_turn(2)]),
    ];
    let f = Fixture::resume(host, state, script, &[], &[]);
    f.put(&f.state.build_path(1), "report 1");
    f.put(&f.state.quality_path(1, 1), "GATE: ERROR\n\n1. x\n");
    f.put(&f.state.quality_build_path(1, 1), "report 1 q1");
    f.put(&f.state.quality_path(1, 2), "GATE: OK\n");
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!((wf.state().round, wf.state().quality_round), (2, 0));
    let body = pr_body(&f);
    assert!(body.contains(&format!("{OFF_AFTER_ANALYSIS}\n\nOpened by")), "{body}");
    assert!(body.contains("| Orchestrator-Gate | `OK after 2 quality rounds, then turned off` |"), "{body}");
    assert!(body.contains("<summary>Quality gate (round 1, before the last change)</summary>\n\nGATE: OK"), "{body}");
    let trailer = "Orchestrator-Gate: OK after 2 quality rounds, then turned off";
    assert_eq!(commit_args(&f).last().map(String::as_str), Some(trailer));
}

#[test]
fn the_note_follows_a_conflict_warning() {
    let mut state = gated_pr_run("quality", 1);
    let host = pr_host();
    turn_off(&mut state, &DEFAULT_WORKFLOW, &host).expect("the gate turns off");
    (host.state().upstream, host.state().conflicts) = (Some("u3".into()), vec!["limiter.py".into()]);
    let f = Fixture::resume(host, state, vec![("review", vec![review_turn(1, APPROVE)])], &[], &[]);
    f.put(&f.state.build_path(1), "report 1");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let body = pr_body(&f);
    let parts: Vec<&str> = body.splitn(4, "\n\n").collect();
    assert!(parts[0].starts_with("> [!WARNING]"), "{body}");
    assert_eq!(parts[1], OFF_BEFORE_ANALYSIS);
    assert!(parts[2].starts_with("Opened by"), "{body}");
}

#[test]
fn off_in_the_builders_quality_fix_goes_straight_to_the_reviewer() {
    let mut state = gated_pr_run("build", 1);
    state.prompted = Some("build-1-q1.md".into());
    let host = pr_host();
    turn_off(&mut state, &DEFAULT_WORKFLOW, &host).expect("the gate turns off");
    assert_eq!((state.phase.as_str(), state.quality_round), ("build", 1));

    let reviews = seen();
    let f = Fixture::resume(host, state, approving(&reviews), &["build"], &[]);
    f.put(&f.state.build_path(1), "report 1");
    f.put(&f.state.quality_path(1, 1), "GATE: ERROR\n\n1. x\n");
    f.put(&f.state.quality_build_path(1, 1), "report 1 q1");
    let mut wf = f.workflow(f.as_saved());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(f.host.state().snapshots.is_empty());
    let review = &reviews.borrow()[0];
    assert!(review.contains(&format!("The Builder's report is in {}", f.state.quality_build_path(1, 1))), "{review}");
    assert!(review.contains(&filled(QUALITY_OFF_NOTE, &[("quality_paths", &f.state.quality_path(1, 1))])), "{review}");
    assert_eq!(wf.state().quality_round, 2);
}

#[test]
fn metrics_count_a_gate_turned_off_as_a_gate() {
    let mut state = gated_pr_run("review", 1);
    turn_off(&mut state, &DEFAULT_WORKFLOW, &pr_host()).expect("the gate turns off");
    let metrics = crate::summary::run_metrics(&state, 1_000_000_000);
    let metrics = metrics.as_object().expect("metrics are an object");
    assert_eq!(
        (metrics.get("quality_gate"), metrics.get("max_quality_rounds")),
        (Some(&Value::Bool(true)), Some(&Value::from(i64::from(DEFAULT_MAX_QUALITY_ROUNDS))))
    );
}

#[test]
fn refused_without_a_gate_or_once_done() {
    let mut done = pr_run("done", 1);
    done.quality_job = Some(LOCAL_GATE.into());
    for (name, mut state) in [("no gate", pr_run("build", 1)), ("done", done)] {
        let before = state.to_json();
        let refused = turn_off(&mut state, &DEFAULT_WORKFLOW, &pr_host()).expect_err(name);
        assert!(refused.0.contains("so --no-quality-gate does not apply"), "{name}: {}", refused.0);
        assert_eq!(state.to_json(), before, "{name}");
    }
}

/// Python's last line, that finish exits 0 for the run, is the CLI's (cli::finish) and is not checked here.
#[test]
fn turned_off_after_a_failed_round_a_workflow_without_a_verdict_finishes() {
    let solo = gsolo();
    let mut state = gsolo_run("quality");
    let host = pr_host();
    turn_off(&mut state, &solo, &host).expect("the gate turns off");
    assert_eq!(state.phase, PUBLISH);

    let f = Fixture::resume(host, state, vec![], &[], &[]);
    f.put(&format!("{}/impl-1.md", f.state.dir()), "impl 1");
    f.put(&f.state.quality_path(1, 1), "GATE: ERROR\n\n1. x\n");
    assert_eq!(f.workflow(RunSettings { pipeline: Some(solo), ..f.as_saved() }).run(), verdict(FINISHED));
    let pr = f.host.state().prs[0].clone();
    assert!(!pr.draft);
    assert!(!pr.body.contains("[!NOTE]"), "{}", pr.body);
}

#[test]
fn turned_off_in_its_quality_fix_a_workflow_without_a_verdict_finishes_unjudged() {
    let solo = gsolo();
    let mut state = gsolo_run("impl");
    let host = pr_host();
    turn_off(&mut state, &solo, &host).expect("the gate turns off");
    let f = Fixture::resume(host, state, vec![("impl", vec![gsolo_turn(1, 1)])], &[], &[]);
    f.put(&format!("{}/impl-1.md", f.state.dir()), "impl 1");
    f.put(&f.state.quality_path(1, 1), "GATE: ERROR\n\n1. x\n");
    let mut wf = f.workflow(RunSettings { pipeline: Some(solo), ..f.as_saved() });
    assert_eq!(wf.run(), verdict(FINISHED));
    assert_eq!(wf.state().quality_round, 2);
    let pr = f.host.state().prs[0].clone();
    assert!(!pr.draft);
    assert!(pr.body.contains(&format!("{OFF_AFTER_ANALYSIS}\n\nOpened by")), "{}", pr.body);
    assert!(pr.body.contains("| Orchestrator-Gate | `ERROR after 1 quality round, then turned off` |"), "{}", pr.body);
}

#[test]
fn round_results_keep_the_quality_files() {
    let mut state = gated_pr_run("review", 1);
    let host = pr_host();
    turn_off(&mut state, &DEFAULT_WORKFLOW, &host).expect("the gate turns off");
    let f = Fixture::resume(host, state, vec![("review", vec![review_turn(1, APPROVE)])], &[], &[]);
    f.put(&f.state.build_path(1), "report 1");
    f.put(&f.state.quality_path(1, 1), "GATE: OK\n");
    assert_eq!(f.workflow(f.as_saved()).run(), verdict(APPROVE));
    let summary = f.host.state().files.get(&format!("{}/summary.md", f.state.dir())).cloned().expect("a summary.md");
    assert!(summary.contains("Round 1: quality gate OK; review APPROVE"), "{summary}");
}

// -- Not Python's: a failure after the checks ---------------------------------------------------------------------

/// A host whose every read fails, and which runs no command.
struct Unreadable;

impl Host for Unreadable {
    fn ssh_target(&self) -> Option<&str> {
        None
    }

    fn exec(&self, argv: &[&str], _: Option<&[u8]>, _: u64) -> Result<crate::host::Exec, crate::host::ExecError> {
        panic!("Unreadable ran {argv:?}")
    }

    fn read(&self, path: &str) -> Result<Option<String>, OrchestratorError> {
        Err(OrchestratorError(format!("cannot read {path}")))
    }
}

#[test]
fn a_failed_read_of_the_next_steps_file_leaves_the_state_as_it_was() {
    // In round 0 no check of turn_quality_gate_on's runs a command, so the read of review-0.md is the only call.
    // Also once the Reviewer was prompted, whose file then decides nothing.
    for prompted in [None, Some("review-0.md")] {
        let mut state = pr_run("review", 0);
        state.prompted = prompted.map(String::from);
        let before = state.to_json();
        let e = turn_quality_gate_on(&mut state, &DEFAULT_WORKFLOW, &Unreadable, &Told::default());
        assert_eq!(e.unwrap_err().0, format!("cannot read {}/review-0.md", state.dir()), "{prompted:?}");
        assert_eq!(state.to_json(), before);
    }
}
