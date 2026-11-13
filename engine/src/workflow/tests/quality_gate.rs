//! TestQualityGate and TestStuckQualityLoop: the quality rounds of a gated run, on FakeJenkins, as Jenkins plays them
//! for real: the preflight, the snapshot pushed and built, the quality file the build archives or the report of a
//! build that failed, Jenkins failing on the way, the heartbeat while it builds, the refs deleted at the end, and a
//! loop that stops when a quality round fails on just what the one before it failed on.

use std::cell::RefCell;
use std::sync::Arc;

use super::*;
use crate::ci::fake::{credentials, Outcome, JENKINS, JOB, JOB_PATH};
use crate::constants::{CI_POLL_SECONDS, HEARTBEAT_SECONDS, QUALITY_TIMEOUT};
use crate::prompts::{QUALITY_PASSED_NOTE, QUALITY_STUCK_NOTE, QUALITY_UNRESOLVED_NOTE};
use crate::quality::finding_fingerprint;

/// make_workflow with the quality gate: Python's gated.
fn gated(script: Script) -> Fixture {
    Fixture::new(script).gated()
}

/// The push of snapshot sha to the ref of round n's quality round q.
fn pushed(sha: &str, n: u32, q: u32) -> Vec<String> {
    strings(&["push", "--quiet", "--force", "origin", &format!("{sha}:refs/heads/orchestrator-ci/a1b2c3-{n}-q{q}")])
}

fn names_of(f: &Fixture) -> Vec<String> {
    collapsed(&f.ci.state().names)
}

/// Nothing the run wrote, logged or saved holds the Jenkins token.
fn assert_no_token(f: &Fixture) {
    for (path, text) in f.host.state().files.iter() {
        assert!(!holds_token(text), "{path} holds the token");
    }
    for line in f.told.lines() {
        assert!(!holds_token(&line), "the log holds the token: {line}");
    }
}

#[test]
fn without_the_gate_no_request_is_made() {
    let f = Fixture::new(approved());
    let mut wf = f.workflow(RunSettings::default());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(f.ci.state().requests.is_empty());
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

    let mut expected = strings(&["parameters", "trigger", "queue"]);
    // Once quality.md is archived, each read of it is followed by one of quality.json.
    expected.extend(strings(&["build", "quality", "build", "quality", "quality.json", "build", "quality", "quality.json"]));
    assert_eq!(names_of(&f), expected);
    let triggers: Vec<Map<String>> = f
        .ci
        .state()
        .requests
        .iter()
        .filter(|r| r.url.ends_with("/buildWithParameters"))
        .map(|r| r.form.clone())
        .collect();
    assert_eq!(triggers, [[("GIT_REF", REF_1_1.to_string())].into_iter().collect::<Map<String>>()]);
    assert!(f.host.state().git_calls.contains(&pushed("snap1", 1, 1)));
    assert_eq!(f.host.state().snapshots[0].0, "abc123");
    let state = &f.state;
    assert_eq!(
        f.quality_file(1, 1),
        "GATE: OK\n\nJenkins build #41 judged snapshot `snap1` (round 1, quality round 1 of 3) and ended SUCCESS.\n\
         \n`orchestrator.py gate` judged the working tree.\n\n## Issues on changed lines\n\nNone.\n"
    );
    assert!(seen.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &state.quality_path(1, 1))));
    assert!(seen.borrow()[0].contains(&format!("The Builder's report is in {}", state.build_path(1))));
    assert!(f.host.state().prs[0].body.contains("<summary>Quality gate (round 1)</summary>"));
    // done: the refs are gone.
    assert_eq!(ci_refs(&f.host), []);
    let saved = f.saved();
    assert_eq!(text(&saved, "phase").as_deref(), Some("done"));
    assert_eq!(saved.get("ci"), Some(&Value::Object(Object::new())));
}

#[test]
fn credentials_travel_only_in_the_authorization_header() {
    let f = gated(approved());
    f.workflow(RunSettings::default()).run().unwrap();

    let jenkins = format!("Basic {}", "YWdlbnRzOmplbmtpbnMtc2VjcmV0LXRva2Vu");
    let ci = f.ci.state();
    assert_eq!(ci.requests.len(), ci.auth.len());
    for (request, auth) in ci.requests.iter().zip(&ci.auth) {
        assert_eq!(auth.as_deref(), Some(jenkins.as_str()));
        let form: Vec<&str> = request.form.values().map(String::as_str).collect();
        assert!(!holds_token(&request.url) && !holds_token(&form.join(" ")), "{request:?}");
    }
    drop(ci);
    assert_no_token(&f);
}

#[test]
fn jenkins_requests_go_under_the_jobs_folder() {
    let f = gated(approved());
    f.workflow(RunSettings::default()).run().unwrap();

    let job = format!("{JENKINS}{JOB_PATH}");
    let urls: Vec<String> = f.ci.state().requests.iter().map(|r| r.url.clone()).collect();
    // The queue items Jenkins names in Location are its own, outside any job.
    let outside: Vec<&String> = urls.iter().filter(|u| !u.starts_with(&job)).collect();
    let queue: Vec<&String> = urls.iter().filter(|u| u.starts_with(&format!("{JENKINS}/queue/item/"))).collect();
    assert_eq!(outside, queue);
    assert!(urls.iter().any(|u| u.starts_with(&job)));
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
        "GATE: ERROR\n\nJenkins build #41 judged snapshot `snap1` (round 1, quality round 1 of 3) and ended UNSTABLE.\n"
    ));
    assert!(quality.contains("1. `limiter.py:3` error ruff F401: `os` imported but unused\n"));
    assert_eq!(*builds.borrow(), [quality_fix(state, 1, 1) + &note()]);
    assert!(reviews.borrow()[0].contains(&format!("The Builder's report is in {}", state.quality_build_path(1, 1))));
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &state.quality_path(1, 2))));
    assert!(f.quality_file(1, 2).starts_with("GATE: OK"));
    assert_eq!(f.ci.state().triggered(), [REF_1_1, "orchestrator-ci/a1b2c3-1-q2"]);
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

    assert!(f.host.state().git_calls.contains(&pushed("snap2", 2, 1)));
    assert_eq!(f.ci.state().triggered(), [REF_1_1, "orchestrator-ci/a1b2c3-2-q1"]);
    assert!(reviews.borrow()[0].contains(&format!("the new report is in {}", f.state.build_path(2))));
    assert!(reviews.borrow()[0].contains(&quality_note(QUALITY_PASSED_NOTE, &f.state.quality_path(2, 1))));
}

/// Run f, whose preflight is to fail with an error holding each of patterns, before any herdr call; the error, saved
/// too, and no token in it.
fn preflight_fails(f: &Fixture, settings: RunSettings, patterns: &[&str]) {
    let e = failure(f.workflow(settings).run());
    for pattern in patterns {
        assert!(e.contains(pattern), "{e}");
    }
    assert!(f.herdr.state().calls.is_empty());
    assert!(!holds_token(&e));
    let saved = text(&f.saved(), "error").unwrap_or_default();
    for pattern in patterns {
        assert!(saved.contains(pattern), "{saved}");
    }
}

fn gated_on(host: FakeHost) -> Fixture {
    Fixture::on(Rc::new(host), new_state(), vec![("spec", vec![])]).gated()
}

#[test]
fn missing_credentials_fail_before_the_interview() {
    let mut env = credentials();
    env.remove("JENKINS_USER");
    let f = Fixture::new(vec![("spec", vec![])]).gated_with(&env);
    preflight_fails(&f, RunSettings::default(), &["the quality gate needs JENKINS_USER in"]);
    assert!(f.ci.state().requests.is_empty());
}

#[test]
fn unreachable_jenkins_fails_before_the_interview() {
    let f = gated_on(FakeHost::default());
    f.ci.state().down = true;
    preflight_fails(&f, RunSettings::default(), &["Jenkins at JENKINS_URL failed the preflight: GET ", "Connection refused"]);
}

#[test]
fn job_without_the_parameter_fails_before_the_interview() {
    let f = gated_on(FakeHost::default());
    f.ci.state().parameters.clear();
    preflight_fails(&f, RunSettings::default(), &["lacks the parameters GIT_REF; build it once by hand"]);
}

#[test]
fn missing_job_fails_before_the_interview() {
    let f = gated_on(FakeHost::default());
    f.ci.state().job_gone = true;
    preflight_fails(&f, RunSettings::default(), &[&format!("there is no Jenkins job {JOB} at JENKINS_URL: "), " HTTP 404"]);
}

#[test]
fn not_a_git_repository_is_refused() {
    for pull_request in [true, false] {
        let f = gated_on(FakeHost::new(None, "main"));
        let settings = RunSettings { pull_request, ..RunSettings::default() };
        preflight_fails(&f, settings, &["the quality gate needs a git repository"]);
        assert!(f.ci.state().requests.is_empty(), "pull_request {pull_request}");
    }
}

#[test]
fn no_pr_needs_a_clean_tree_and_an_origin() {
    let host = FakeHost::default();
    host.state().changed.insert("/proj/wip.py".into());
    preflight_fails(&gated_on(host), no_pr(), &["uncommitted changes"]);
    let host = FakeHost::default();
    host.state().has_origin = false;
    preflight_fails(&gated_on(host), no_pr(), &["pushes snapshots to origin, and /proj has no origin"]);
}

#[test]
fn no_pr_pushes_snapshots_without_a_run_branch() {
    let f = gated(approved());
    let mut wf = f.workflow(no_pr());

    assert_eq!(wf.run(), verdict(APPROVE));
    let h = f.host.state();
    assert!(h.git_calls.contains(&pushed("snap1", 1, 1)));
    assert!(!h.git_calls.iter().any(|c| ["switch", "commit", "add"].contains(&c[0].as_str())));
    assert_eq!(h.changed.iter().collect::<Vec<_>>(), ["/proj/limiter.py"]);
    assert!(h.remote.is_empty());
}

#[test]
fn job_deleted_mid_run_fails_at_once() {
    let f = gated(vec![("spec", vec![]), ("build", vec![build_turn(1)])]);
    let ci = Arc::clone(&f.ci);
    let mut spec = spec_turn();
    f.play(
        "spec",
        vec![Box::new(move |prompt, state, host| {
            ci.state().job_gone = true;
            spec(prompt, state, host)
        })],
    );

    let e = failure(f.workflow(RunSettings::default()).run());
    assert!(e.contains(&format!("Jenkins job {JOB} is gone: POST ")) && e.contains("HTTP 404"), "{e}");
    assert!(f.clock.now() < 60.0);
}

/// Make Jenkins stop answering once the change is triggered, and answer again back_after seconds later, if given.
fn jenkins_down_after_the_change_is_triggered(f: &Fixture, back_after: Option<f64>) {
    let ci = Arc::clone(&f.ci);
    let mut down_at: Option<f64> = None;
    f.clock.hook(move |now| {
        let mut ci = ci.state();
        match (down_at, back_after) {
            (None, _) if !ci.triggered().is_empty() => {
                ci.down = true;
                down_at = Some(now);
            }
            (Some(at), Some(after)) if now >= at + after => ci.down = false,
            _ => {}
        }
    });
}

#[test]
fn jenkins_unreachable_mid_round_then_back() {
    let f = gated(approved());
    jenkins_down_after_the_change_is_triggered(&f, Some(300.0));

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    assert!(f.ci.state().names.contains(&"jenkins down".to_string()));
    assert!(f.quality_file(1, 1).starts_with("GATE: OK"));
    assert!(f.told.lines().iter().any(|l| l.ends_with("answered again")), "{:?}", f.told.lines());
}

#[test]
fn jenkins_unreachable_until_the_round_times_out() {
    let f = gated(vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])]);
    jenkins_down_after_the_change_is_triggered(&f, None);

    let e = failure(f.workflow(RunSettings::default()).run());
    assert!(
        e.starts_with(&format!(
            "quality gate: waiting for Jenkins to start the build of {REF_1_1} did not finish within 1200s"
        )),
        "{e}"
    );
    assert!(e.contains("Connection refused (token ****)"), "{e}");
    let saved = f.saved();
    let ci = ci_of(&[
        ("ref", REF_1_1.into()),
        ("sha", "snap1".into()),
        ("queue_url", format!("{JENKINS}/queue/item/941/").into()),
    ]);
    assert_eq!(saved.get("ci"), Some(&Value::Object(ci)));
    assert_eq!(text(&saved, "phase").as_deref(), Some("quality"));
    assert_no_token(&f);
    // A failed run keeps the ref in flight, for a resume.
    assert!(f.host.state().remote.contains_key(REF_1_1));
}

#[test]
fn heartbeat_during_a_twenty_minute_build() {
    let f = gated_building(approved(), vec![Outcome { polls: 110, ..green() }]);
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
fn a_build_running_on_after_its_gate_ends_the_round_with_its_quality_file() {
    // quality.md archived on the second poll, and the build running long past the round's deadline after it.
    let polls = (2 * QUALITY_TIMEOUT / CI_POLL_SECONDS) as u32;
    let f = gated_building(
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), fix_turn(1, 1)]),
            ("review", vec![review_turn(1, APPROVE)]),
        ],
        vec![Outcome { polls, after_gate: polls - 2, ..red() }, green()],
    );

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));

    let quality = f.quality_file(1, 1);
    assert!(
        quality.starts_with(
            "GATE: ERROR\n\nJenkins build #41 judged snapshot `snap1` (round 1, quality round 1 of 3) and was still \
             running when its quality.md was read.\n"
        ),
        "{quality}"
    );
    assert!(quality.contains("1. `limiter.py:3` error ruff F401: `os` imported but unused\n"));
    // The result and the file once per poll, the file taken on the second read that agrees, then round 2.
    let mut expected = strings(&["parameters", "trigger", "queue"]);
    expected.extend(strings(&["build", "quality", "build", "quality", "quality.json", "build", "quality"]));
    expected.extend(strings(&["quality.json", "trigger"]));
    assert_eq!(names_of(&f)[..12], expected);
    assert_eq!(f.ci.state().builds[&41].left, polls - 3);
}

#[test]
fn a_quality_file_read_while_its_build_runs_is_taken_once_two_reads_agree() {
    let f = gated_building(approved(), vec![Outcome { polls: 5, after_gate: 4, ..green() }]);
    let cut: String = green().quality.unwrap().chars().take(12).collect();
    f.ci.state().quality_replies.insert(1, cut);

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));

    assert_eq!(
        f.quality_file(1, 1),
        "GATE: OK\n\nJenkins build #41 judged snapshot `snap1` (round 1, quality round 1 of 3) and was still running \
         when its quality.md was read.\n\n`orchestrator.py gate` judged the working tree.\n\n## Issues on changed \
         lines\n\nNone.\n"
    );
    assert_eq!(f.ci.state().quality_reads.len(), 3);
    assert_eq!(f.ci.state().builds[&41].left, 2);
}

#[test]
fn a_read_of_a_running_build_s_quality_file_outlasts_a_failed_read_after_it() {
    let f = gated_building(approved(), vec![Outcome { polls: 6, after_gate: 5, ..green() }]);
    // The read after the first fails as Jenkins's own trouble does, once.
    let path = format!("{JOB_PATH}41/artifact/quality.md");
    let ci = Arc::clone(&f.ci);
    f.clock.hook(move |_| {
        let mut ci = ci.state();
        let failed = ci.names.iter().any(|n| n.ends_with(" failing"));
        if ci.quality_reads.len() == 1 && !failed && ci.fail.is_empty() {
            ci.fail.push((("GET", path.clone()), 503));
        } else if failed {
            ci.fail.clear();
        }
    });

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));

    assert!(f.ci.state().names.iter().any(|n| n.ends_with("quality.md failing")), "{:?}", f.ci.state().names);
    // The read before the failure is the first of the two that agree: taken at the next read, the build still running.
    assert_eq!(f.ci.state().quality_reads.len(), 2);
    assert!(f.ci.state().builds[&41].left > 0);
    assert!(f.quality_file(1, 1).starts_with("GATE: OK\n\nJenkins build #41 judged snapshot `snap1`"));
    assert!(f.told.lines().iter().any(|l| l.ends_with("answered again")), "{:?}", f.told.lines());
}

#[test]
fn a_build_that_ends_with_another_quality_file_than_the_one_read_while_it_ran_is_judged_by_the_last() {
    // Read once while it runs, then again once it has ended.
    let f = gated_building(approved(), vec![Outcome { polls: 2, after_gate: 0, ..green() }]);
    let running = "GATE: FAIL\n\n`orchestrator.py gate` judged the working tree.\n\n## Issues on changed lines\n\n\
                   1. x\n";
    f.ci.state().quality_replies.insert(1, running.to_string());

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));

    assert_eq!(f.ci.state().quality_reads.len(), 2);
    let quality = f.quality_file(1, 1);
    assert!(quality.starts_with("GATE: OK\n"), "{quality}");
    assert!(!quality.contains("still running"), "{quality}");
}

#[test]
fn two_reads_of_a_running_build_without_their_gate_line_are_not_its_quality_file() {
    // archiveArtifacts can leave the file empty, or cut inside its GATE line, for longer than a poll interval.
    for cut in ["", "GATE: O"] {
        let f = gated_building(approved(), vec![Outcome { polls: 6, after_gate: 5, ..green() }]);
        f.ci.state().quality_replies.extend([(1, cut.to_string()), (2, cut.to_string())]);

        assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE), "cut {cut:?}");
        assert!(f.quality_file(1, 1).starts_with(
            "GATE: OK\n\nJenkins build #41 judged snapshot `snap1` (round 1, quality round 1 of 3) and was still \
             running when its quality.md was read."
        ));
        assert_eq!(f.ci.state().quality_reads.len(), 4, "cut {cut:?}");
    }
}

#[test]
fn two_reads_of_a_running_build_with_a_whole_line_but_no_gate_stop_the_round_at_once() {
    let f = gated_building(approved(), vec![Outcome { polls: 50, after_gate: 49, ..green() }]);
    f.ci.state().quality_replies.extend((1..50).map(|k| (k, "Not a gate line\n".to_string())));

    let e = failure(f.workflow(RunSettings::default()).run());
    assert!(e.contains("does not start with a GATE line"), "{e}");
    // Taken at the second read, not left to the build's end or the round's deadline.
    assert_eq!(f.ci.state().quality_reads.len(), 2);
}

#[test]
fn build_failed_before_the_gate_goes_back_to_the_builder() {
    let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    let console = format!("{}\nJENKINS_TOKEN={}\nSyntaxError: bad", lines.join("\n"), crate::ci::fake::JENKINS_TOKEN);
    let builds = seen();
    let f = gated_building(
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), recording(fix_turn(1, 1), &builds)]),
            ("review", vec![review_turn(1, APPROVE)]),
        ],
        vec![Outcome { console, test_report: false, ..Outcome::unarchived() }, green()],
    );

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    let quality = f.quality_file(1, 1);
    assert!(quality.starts_with("GATE: BUILD_FAILED\n"));
    assert!(quality.contains(
        "Jenkins build #41 of snapshot `snap1` (round 1, quality round 1 of 3) ended FAILURE before the quality gate ran"
    ));
    assert!(quality.contains("JENKINS_TOKEN=****\nSyntaxError: bad"));
    assert!(!quality.contains("line 41\n"));
    assert!(quality.contains("````\nline 42\n"));
    assert!(!holds_token(&quality));
    assert!(builds.borrow()[0].contains(&f.state.quality_path(1, 1)));
    assert!(f.quality_file(1, 2).starts_with("GATE: OK"));
}

#[test]
fn build_failed_with_failing_tests_has_no_console() {
    let failing = ("tests.t".to_string(), "test_a".to_string(), "AssertionError: 1 != 2".to_string());
    let f = gated_building(
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), fix_turn(1, 1)]),
            ("review", vec![review_turn(1, APPROVE)]),
        ],
        vec![Outcome { console: "x".into(), tests: vec![failing], ..Outcome::unarchived() }],
    );

    f.workflow(RunSettings::default()).run().unwrap();
    let quality = f.quality_file(1, 1);
    assert!(quality.contains("- `tests.t.test_a`: AssertionError: 1 != 2"), "{quality}");
    assert!(!quality.contains("console"));
    assert!(!f.ci.state().names.contains(&"console".to_string()));
}

#[test]
fn aborted_build_without_a_quality_file_fails_the_run() {
    let f = gated_building(
        vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])],
        vec![Outcome { result: "ABORTED".into(), ..Outcome::unarchived() }],
    );

    let e = failure(f.workflow(RunSettings::default()).run());
    assert!(e.contains(&format!("build #41 of {REF_1_1} was aborted before the quality gate")), "{e}");
    let ci = ci_of(&[("ref", REF_1_1.into()), ("sha", "snap1".into()), ("rejected", Value::Array(vec![41.into()]))]);
    assert_eq!(f.saved().get("ci"), Some(&Value::Object(ci)));
    assert!(!f.host.state().files.contains_key(&f.state.quality_path(1, 1)));
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 1);
}

#[test]
fn quality_file_is_followed_whatever_the_result() {
    for result in ["FAILURE", "ABORTED"] {
        let f = gated_building(approved(), vec![Outcome { result: result.into(), ..green() }]);
        assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE), "{result}");
        assert!(f.quality_file(1, 1).starts_with("GATE: OK"), "{result}");
    }
}

#[test]
fn queue_item_cancelled_fails_the_run() {
    let f = gated(vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])]);
    f.ci.state().cancelled = true;

    let e = failure(f.workflow(RunSettings::default()).run());
    assert!(e.contains("cancelled in Jenkins's queue"), "{e}");
    let ci = ci_of(&[("ref", REF_1_1.into()), ("sha", "snap1".into())]);
    assert_eq!(f.saved().get("ci"), Some(&Value::Object(ci)));
}

#[test]
fn done_deletes_only_this_runs_refs() {
    let f = gated(approved());
    f.host.state().remote.insert("orchestrator-ci/a1b2c3-1-q7", "left-over".into());
    f.host.state().remote.insert("orchestrator-ci/ffffff-1-q1", "another-run".into());

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    assert_eq!(text(&f.saved(), "phase").as_deref(), Some("done"));
    assert_eq!(ci_refs(&f.host), [("orchestrator-ci/ffffff-1-q1".to_string(), "another-run".to_string())]);
}

#[test]
fn refused_request_mid_round_fails_at_once_naming_the_step() {
    let f = gated(vec![("spec", vec![spec_turn()]), ("build", vec![build_turn(1)])]);
    f.ci.state().fail.push((("GET", format!("{JOB_PATH}41/artifact/quality.md")), 403));

    let e = failure(f.workflow(RunSettings::default()).run());
    let step = format!("quality gate: waiting for Jenkins build #41 of {REF_1_1} to archive quality.md or end: GET ");
    assert!(e.starts_with(&step) && e.ends_with("/41/artifact/quality.md: HTTP 403"), "{e}");
    assert!(f.clock.now() < 200.0);
    let saved = f.saved();
    assert_eq!(saved.get("ci").and_then(|ci| ci.get("build")), Some(&Value::from(41)));
}

#[test]
fn cleanup_without_origin_listing_is_only_logged() {
    let f = gated(approved());
    f.host.state().list_remote_error = Some("ssh: connection reset".into());

    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    assert!(f.told.lines().iter().any(|l| l.contains("could not list the orchestrator-ci/ branches")));
    // The round deleted its own ref once judged, which the end could not.
    assert_eq!(ci_refs(&f.host), []);
}

#[test]
fn a_build_failed_with_a_test_report_of_no_failures_shows_its_console() {
    // Not one of Python's cases: a report without failing tests says nothing, so the console's end is read instead.
    let f = gated_building(
        vec![
            ("spec", vec![spec_turn()]),
            ("build", vec![build_turn(1), fix_turn(1, 1)]),
            ("review", vec![review_turn(1, APPROVE)]),
        ],
        vec![Outcome { console: "error: cannot install".into(), ..Outcome::unarchived() }],
    );

    f.workflow(RunSettings::default()).run().unwrap();
    let quality = f.quality_file(1, 1);
    assert!(quality.contains("## Failing tests\n\nNone.\n"), "{quality}");
    assert!(quality.contains("````\nerror: cannot install\n````"), "{quality}");
}

// -- TestStuckQualityLoop ---------------------------------------------------------------------------------------

/// A gated run whose builds play outcomes, with builds turns of the Builder in round 1; its fixture, run to APPROVE,
/// and the reviews' prompts.
fn stuck_loop(outcomes: Vec<Outcome>, builds: u32) -> (Fixture, Rc<RefCell<Vec<String>>>) {
    let reviews = seen();
    let mut turns = vec![build_turn(1)];
    turns.extend((1..builds).map(|q| fix_turn(1, q)));
    let f = gated_building(
        vec![("spec", vec![spec_turn()]), ("build", turns), ("review", vec![recording(review_turn(1, APPROVE), &reviews)])],
        outcomes,
    );
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
fn a_build_that_failed_with_nothing_listed_is_not_stuck() {
    let failed =
        Outcome { console: "error: cannot install".into(), test_report: false, ..Outcome::unarchived() };
    let (f, _) = stuck_loop(vec![failed.clone(), failed.clone(), failed], 3);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 3);
}

#[test]
fn a_failure_holding_a_credential_is_saved_masked() {
    // As Python's _release masks the error it saves: here git's own message quotes the token.
    let host = FakeHost::default();
    host.state().fetch_error =
        Some(format!("fatal: unable to access 'https://agents:{}@git.example/r/'", crate::ci::fake::JENKINS_TOKEN));
    let f = gated_on(host);

    let e = failure(f.workflow(RunSettings::default()).run());
    assert!(holds_token(&e), "the error itself is git's, as Python raises it: {e}");
    let saved = text(&f.saved(), "error").unwrap_or_default();
    assert!(saved.contains("unable to access 'https://agents:****@git.example/r/'"), "{saved}");
    assert_no_token(&f);
}

// -- quality.json ------------------------------------------------------------------------------------------------

/// outcome with its quality.json changed by change, as a JSON object.
fn reporting(outcome: Outcome, change: impl FnOnce(&mut Object)) -> Outcome {
    let mut report: Object = serde_json::from_str(outcome.quality_json.as_deref().unwrap()).unwrap();
    change(&mut report);
    Outcome { quality_json: Some(Value::Object(report).to_string()), ..outcome }
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
fn a_build_that_archived_no_quality_json_is_read_from_its_quality_file() {
    let f = gated_building(fixed_once(), vec![red_with_more_in_its_json().markdown_only(), green().markdown_only()]);
    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    let f401 = finding_fingerprint(Some("limiter.py"), "ruff F401", "`os` imported but unused");
    assert_eq!(recorded_fingerprints(&f, 1), serde_json::json!({"ruff": [f401]}));
    assert_eq!(f.saved().get("quality_rounds").and_then(Value::as_array).map(Vec::len), Some(2));
}

#[test]
fn stuck_compares_every_failure_quality_json_lists() {
    // The quality files are the same, but quality.json fails round 2 on a finding round 1 did not have.
    let (f, _) = stuck_loop(vec![red(), red_with_more_in_its_json(), red()], 3);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 3);
    // Without quality.json, the same files are the same failures again.
    let (f, _) = stuck_loop(vec![red().markdown_only(), red_with_more_in_its_json().markdown_only()], 3);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 2);
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

/// The run saved its round's build 41 as rejected, so that a resume builds the snapshot again.
fn assert_build_is_left_to_build_again(f: &Fixture) {
    let ci = ci_of(&[("ref", REF_1_1.into()), ("sha", "snap1".into()), ("rejected", Value::Array(vec![41.into()]))]);
    assert_eq!(f.saved().get("ci"), Some(&Value::Object(ci)));
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

#[test]
fn a_running_build_s_reports_are_taken_once_two_reads_of_both_agree() {
    // archiveArtifacts has copied quality.md, but not yet quality.json, at the first read.
    let late = Outcome { polls: 50, after_gate: 49, ..red_with_more_in_its_json() };
    let f = gated_building(fixed_once(), vec![late, green()]);
    f.ci.state().quality_json_late = 1;
    assert_eq!(f.workflow(RunSettings::default()).run(), verdict(APPROVE));
    // Three reads of the running build, the first finding no quality.json, then two of the green build.
    let ci = f.ci.state();
    assert_eq!((ci.quality_reads.len(), ci.quality_json_reads), (5, 5));
    drop(ci);
    assert!(recorded_fingerprints(&f, 1).get("mypy").is_some(), "read from quality.json");
}

