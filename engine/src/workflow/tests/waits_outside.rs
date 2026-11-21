//! An agent in a turn that reports it is waiting on something outside the run (its handoff file's waiting file, the
//! wait note of a step that can block): its turn's deadline stands still and no stall is told while the report
//! stands, a resume finds the wait and waits too, and the report goes once the turn ends.
//!
//! Not here: the note in the prompts of steps that can block, in tests.rs beside the BLOCKED note.

use super::*;

const D: &str = "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3";
const WHAT: &str = "the merge of pull request #526";

fn build_1() -> String {
    format!("{D}/build-1.md")
}

fn waiting_1() -> String {
    format!("{D}/build-1.waiting")
}

/// The titles of the human's notices that name the Builder's waits.
fn waits_told(f: &Fixture) -> usize {
    f.told.titles().iter().filter(|t| *t == &format!("Builder is waiting on {WHAT}")).count()
}

/// The titles of the human's notices of the Builder's stalls.
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
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(waits_told(&f), 1);
    assert_eq!(prompts_to(&f, "build-a1b2c3"), 0);
}
