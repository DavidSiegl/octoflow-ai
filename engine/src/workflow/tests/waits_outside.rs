//! An agent in a turn that reports it is waiting on something outside the run (its handoff file's waiting file, the
//! wait note of a step that can block): its turn's deadline stands still and no stall is told while the report
//! stands, a resume finds the wait and waits too, and the report goes once the turn ends. The turn's record carries
//! the wait while it stands and counts its seconds once it ends (TurnRecord::outside_wait_seconds).
//!
//! Not here: the note in the prompts of steps that can block, in tests.rs beside the BLOCKED note.

use super::*;
use crate::constants::POLL_SECONDS;
use crate::host::fake::HOST_EPOCH;
use crate::workflow::Workflow;

const D: &str = "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3";
const WHAT: &str = "the merge of pull request #526";

fn build_1() -> String {
    format!("{D}/build-1.md")
}

fn waiting_1() -> String {
    format!("{D}/build-1.waiting")
}

/// How many of the human's notices name the Builder's wait.
fn waits_told(f: &Fixture) -> usize {
    f.told.titles().iter().filter(|t| *t == &format!("Builder is waiting on {WHAT}")).count()
}

/// The record of the run's build-1.md turn.
fn build_1_turn(wf: &Workflow) -> TurnRecord {
    wf.state().turns.iter().find(|t| t.file == "build-1.md").cloned().expect("a build-1.md turn")
}

/// A record of the build-1.md turn under way, as a run that stopped while its Builder reported a wait on WHAT saved
/// it: started 200s and waiting 100s before HOST_EPOCH.
fn waiting_build_turn(wf: &Workflow) -> TurnRecord {
    let mut record = wf.turn_record("build", "build-1.md", Some(HOST_EPOCH - 200));
    let since = crate::timestamp::iso_utc(HOST_EPOCH - 100);
    record.waiting_on = Some(crate::state::StandingWait { on: WHAT.to_string(), since });
    record
}

/// How many of the human's notices name the Builder's stall.
fn stalls_told(f: &Fixture) -> usize {
    f.told.titles().iter().filter(|t| *t == "Builder is idle without writing build-1.md").count()
}

/// A run whose Builder, once prompted, reports a wait at the first poll, withdraws it once withdraw seconds have
/// passed, and writes its report after seconds more; with withdraw None it writes its report after seconds, its
/// waiting file left in place.
fn waits_then_writes(f: &Fixture, withdraw: Option<f64>, after: f64) {
    let host = Rc::clone(&f.host);
    let mut start: Option<f64> = None;
    f.clock.hook(move |now| {
        let prompted = host.state().files.contains_key(&format!("{D}/spec.md"));
        if !prompted || host.state().files.contains_key(&build_1()) {
            return;
        }
        if start.is_none() {
            start = Some(now);
            host.write(&waiting_1(), &format!("{WHAT}\n"), false).unwrap();
        }
        let since = now - start.unwrap_or(now);
        let write_at = match withdraw {
            Some(w) if since >= w => {
                host.remove(&waiting_1()).unwrap();
                w + after
            }
            Some(_) => return,
            None => after,
        };
        if since >= write_at {
            host.write(&build_1(), "report 1", false).unwrap();
        }
    });
}

#[test]
fn a_turn_that_waits_past_its_timeout_and_then_writes_its_file_succeeds() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![idle()]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    // Waiting ten times the turn's 60s, then working 30s more: 90s of the turn's clock in all.
    waits_then_writes(&f, Some(600.0), 30.0);
    let mut wf = f.workflow(RunSettings { turn_timeout: 60, ..no_pr() });
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(waits_told(&f), 1);
    assert_eq!(stalls_told(&f), 0);
    assert!(f.told.lines().iter().any(|l| l.starts_with(&format!("[build] is waiting on {WHAT} ("))));
}

#[test]
fn a_withdrawn_wait_s_seconds_are_recorded_apart_from_the_turn_s() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![idle()]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    waits_then_writes(&f, Some(600.0), 30.0);
    let mut wf = f.workflow(no_pr());
    assert_eq!(wf.run(), verdict(APPROVE));
    let turn = build_1_turn(&wf);
    // A poll's worth of slack either side: the wait is found and its withdrawal seen at polls POLL_SECONDS apart.
    let outside = turn.outside_wait_seconds.expect("the turn's wait is recorded");
    assert!((600 - POLL_SECONDS as i64..=600 + POLL_SECONDS as i64).contains(&outside), "{outside}");
    assert!(turn.seconds.is_some_and(|s| s >= outside + 30), "{turn:?}");
    assert_eq!(turn.waiting_on, None);
    let saved = f.saved();
    let turns = saved.get("turns").and_then(Value::as_array).expect("saved turns");
    let build = turns.iter().find(|t| t.get("file") == Some(&Value::from("build-1.md"))).expect("a saved build turn");
    assert_eq!(build.get("outside_wait_seconds"), Some(&Value::from(outside)));
    assert_eq!(build.get("waiting_on"), None);
}

#[test]
fn a_wait_found_at_one_poll_only_is_not_recorded() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![idle()]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    let host = Rc::clone(&f.host);
    let mut polls = 0;
    // The file stands for one poll, as one read between the agent's creating it empty and its withdrawing the wait.
    f.clock.hook(move |_| {
        if !host.state().files.contains_key(&format!("{D}/spec.md")) || host.state().files.contains_key(&build_1()) {
            return;
        }
        polls += 1;
        match polls {
            1 => host.write(&waiting_1(), "", false).unwrap(),
            2 => host.remove(&waiting_1()).unwrap(),
            _ => host.write(&build_1(), "report 1", false).unwrap(),
        }
    });
    let mut wf = f.workflow(no_pr());
    assert_eq!(wf.run(), verdict(APPROVE));
    let turn = build_1_turn(&wf);
    assert_eq!((turn.outside_wait_seconds, turn.waiting_on), (None, None));
    assert_eq!(waits_told(&f), 0);
}

#[test]
fn a_turn_that_reported_no_wait_records_none() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    let mut wf = f.workflow(no_pr());
    assert_eq!(wf.run(), verdict(APPROVE));
    let turn = build_1_turn(&wf);
    assert_eq!((turn.outside_wait_seconds, turn.waiting_on), (None, None));
}

#[test]
fn a_turn_that_works_past_its_timeout_after_a_wait_still_fails() {
    let f = Fixture::new(vec![("spec", vec![spec_turn()]), ("build", vec![idle()])]);
    waits_then_writes(&f, Some(600.0), 120.0);
    let message = failure(f.workflow(RunSettings { turn_timeout: 60, ..no_pr() }).run());
    assert!(message.starts_with(&format!("the Builder did not write {} within 60s", build_1())), "{message}");
}

#[test]
fn no_stall_is_told_while_waiting_and_one_is_once_the_wait_is_withdrawn() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![idle()]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    let stall = STALL_SECONDS as f64;
    waits_then_writes(&f, Some(3.0 * stall), stall + 10.0);
    assert_eq!(f.workflow(no_pr()).run(), verdict(APPROVE));
    assert_eq!((waits_told(&f), stalls_told(&f)), (1, 1));
    let titles = f.told.titles();
    let at = |title: &str| titles.iter().position(|t| t == title);
    assert!(at(&format!("Builder is waiting on {WHAT}")) < at("Builder is idle without writing build-1.md"));
}

#[test]
fn a_wait_report_left_when_the_file_is_written_is_deleted_with_the_turn() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![idle()]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    waits_then_writes(&f, None, 120.0);
    let mut wf = f.workflow(RunSettings { turn_timeout: 60, ..no_pr() });
    assert_eq!(wf.run(), verdict(APPROVE));
    assert!(!f.host.state().files.contains_key(&waiting_1()));
    // The wait ends with the turn, at its file's mtime.
    let turn = build_1_turn(&wf);
    let outside = turn.outside_wait_seconds.expect("the turn's wait is recorded");
    assert!((120 - POLL_SECONDS as i64..=120 + POLL_SECONDS as i64).contains(&outside), "{outside}");
    assert_eq!(turn.waiting_on, None);
}

#[test]
fn an_agent_that_exits_while_it_waits_fails_the_run() {
    let f = Fixture::new(vec![("spec", vec![spec_turn()]), ("build", vec![idle()])]);
    let (host, herdr) = (Rc::clone(&f.host), Rc::clone(&f.herdr));
    f.clock.hook(move |now| {
        if !host.state().files.contains_key(&format!("{D}/spec.md")) {
            return;
        }
        if !host.state().files.contains_key(&waiting_1()) {
            host.write(&waiting_1(), WHAT, false).unwrap();
        } else if now > 600.0 {
            herdr.state().statuses.remove("build-a1b2c3");
        }
    });
    let message = failure(f.workflow(RunSettings { turn_timeout: 60, ..no_pr() }).run());
    assert_eq!(message, format!("the Builder exited without writing {}", build_1()));
    // The saved turn carries the wait that still stood, for the run's view and a resume.
    let saved = f.saved();
    let turns = saved.get("turns").and_then(Value::as_array).expect("saved turns");
    let build = turns.iter().find(|t| t.get("file") == Some(&Value::from("build-1.md"))).expect("a saved build turn");
    let wait = build.get("waiting_on").expect("a standing wait");
    assert_eq!(wait.get("on"), Some(&Value::from(WHAT)));
    assert!(wait.get("since").and_then(Value::as_str).is_some_and(|s| s.ends_with("+00:00")), "{wait}");
    assert_eq!(build.get("outside_wait_seconds"), None);
}

#[test]
fn a_resume_that_finds_the_wait_standing_waits_too() {
    let mut state = saved_run("build", 1, &["spec", "build"]);
    state.prompted = Some("build-1.md".to_string());
    state.turn_timeout = 60;
    state.pull_request = false;
    let host = Rc::new(FakeHost::default());
    host.write(&format!("{D}/spec.md"), SPEC, false).unwrap();
    host.write(&waiting_1(), WHAT, false).unwrap();
    let f = Fixture::resume(Rc::clone(&host), state, vec![("review", vec![review_turn(1, APPROVE)])], &["build"], &[]);
    f.clock.hook(move |now| {
        if now > 600.0 && !host.state().files.contains_key(&build_1()) {
            host.remove(&waiting_1()).unwrap();
            host.write(&build_1(), "report 1", false).unwrap();
        }
    });
    let mut wf = f.workflow(f.as_saved());
    let record = waiting_build_turn(&wf);
    wf.state.turns.open(record);
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(waits_told(&f), 1);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 0);
    // The wait the stopped run saved goes on from its start: 100s before the resume, and 600s and a poll after it.
    let outside = build_1_turn(&wf).outside_wait_seconds.expect("the turn's wait is recorded");
    assert!((700..=700 + POLL_SECONDS as i64).contains(&outside), "{outside}");
}

#[test]
fn a_wait_whose_reason_is_rewritten_is_told_once() {
    let f = Fixture::new(vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![idle()]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    let host = Rc::clone(&f.host);
    let mut polls = 0;
    f.clock.hook(move |_| {
        if !host.state().files.contains_key(&format!("{D}/spec.md")) || host.state().files.contains_key(&build_1()) {
            return;
        }
        polls += 1;
        if polls < 10 {
            host.write(&waiting_1(), &format!("checks of pull request #526: {polls} of 9 done"), false).unwrap();
        } else {
            host.write(&build_1(), "report 1", false).unwrap();
        }
    });
    assert_eq!(f.workflow(no_pr()).run(), verdict(APPROVE));
    let waits: Vec<String> = f.told.titles().into_iter().filter(|t| t.starts_with("Builder is waiting on ")).collect();
    assert_eq!(waits, ["Builder is waiting on checks of pull request #526: 2 of 9 done"]);
}

#[test]
fn a_wait_left_by_an_agent_that_exited_is_dropped_for_the_session_started_in_its_place() {
    let mut state = saved_run("build", 1, &["spec", "build"]);
    state.prompted = Some("build-1.md".to_string());
    state.pull_request = false;
    state.agents.get_mut("build").unwrap().session = None;
    let host = Rc::new(FakeHost::default());
    host.write(&format!("{D}/spec.md"), SPEC, false).unwrap();
    host.write(&waiting_1(), WHAT, false).unwrap();
    let f = Fixture::resume(Rc::clone(&host), state, vec![("build", vec![idle()])], &[], &[]);
    // Without the drop the old wait would stand for good, and the turn never time out.
    f.clock.hook(|now| assert!(now < 20_000.0, "the turn did not time out"));
    let mut wf = f.workflow(f.as_saved());
    let record = waiting_build_turn(&wf);
    wf.state.turns.open(record);
    let message = failure(wf.run());
    assert!(message.starts_with(&format!("the Builder did not write {} within ", build_1())), "{message}");
    assert_eq!(waits_told(&f), 0);
    assert!(!host.state().files.contains_key(&waiting_1()));
    // The saved wait ends when the replaced session is found, before the new one's turn starts.
    let turn = build_1_turn(&wf);
    assert_eq!((turn.outside_wait_seconds, turn.waiting_on), (Some(100), None));
}
