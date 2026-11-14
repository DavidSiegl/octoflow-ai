//! The Ctrl-C while the run waits on herdr, and once it is done. A real ProcessHerdr, running a herdr script, is what
//! the Ctrl-C stops (TODO.md's Ctrl-C entry); the interrupted proof, the command case, is proofs.rs's.

use super::*;
use crate::herdr::{Herdr, ProcessHerdr};
use crate::host::tests::{environment, tool};
use crate::test_support::{ended, hearing_sigint, raise_once_written, TempDir};

/// A ProcessHerdr of the fixture's Ctrl-C whose herdr is script, in a directory of its own.
fn real_herdr(f: &Fixture, d: &TempDir, script: &str) -> ProcessHerdr {
    tool(&d.path(), "herdr", script);
    ProcessHerdr::in_environment(None, environment(Some(&d.path()), &[]).into_iter(), &f.interrupt)
}

/// The run as it was saved, which must say it was interrupted and that no orchestrator drives it, once the Workflow
/// caught the Ctrl-C to save that.
fn interrupted(f: &Fixture) -> RunState {
    assert!(!f.interrupt.is_raised());
    let saved = RunState::from_saved(&f.saved()).unwrap();
    assert_eq!(saved.error.as_deref(), Some("interrupted"));
    assert_eq!(saved.owner, None);
    saved
}

#[test]
fn a_ctrl_c_while_herdr_waits_stops_the_wait_and_the_run() {
    let d = TempDir::new();
    let pid = d.join("pid");
    let f = Fixture::new(vec![("spec", vec![working()])]);
    let herdr = real_herdr(&f, &d, &format!("#!/bin/sh\necho $$ > '{pid}'\nexec sleep 600\n"));
    f.herdr.state().wait_with = Some(Box::new(move |name, timeout_ms, until| herdr.wait(name, timeout_ms, until)));
    let raising = raise_once_written(&pid, &f.interrupt);
    assert_eq!(f.workflow(RunSettings::default()).run(), Err(Stopped::Interrupted));
    raising.join().unwrap();
    assert!(ended(&pid), "herdr agent wait outlived the Ctrl-C");
    // The wait was the delivery check: the prompt never counted as taken.
    let saved = interrupted(&f);
    assert_eq!((saved.prompted, saved.turns.len()), (None, 0));
}

#[test]
fn a_ctrl_c_during_a_prompt_herdr_delivered_records_no_prompt() {
    // No prompted, no turn record, whatever herdr did with the text.
    let d = TempDir::new();
    let delivered = d.join("delivered");
    let f = Fixture::new(vec![("spec", vec![spec_turn()])]);
    let script = hearing_sigint(&format!("trap 'exit 0' INT\necho \"$4\" > '{delivered}'\nwhile :; do sleep 0.05; done\n"));
    let herdr = real_herdr(&f, &d, &script);
    f.herdr.state().prompt_with = Some(Box::new(move |name, text| herdr.prompt(name, text)));
    let raising = raise_once_written(&delivered, &f.interrupt);
    assert_eq!(f.workflow(RunSettings::default()).run(), Err(Stopped::Interrupted));
    raising.join().unwrap();
    assert!(std::fs::read_to_string(&delivered).unwrap().len() > 1, "herdr was given no prompt");
    let saved = interrupted(&f);
    assert_eq!((saved.prompted, saved.turns.len()), (None, 0));
    // The Spec Writer's turn was never played.
    assert_eq!(f.herdr.state().prompts.len(), 0);
}

#[test]
fn a_ctrl_c_once_the_run_is_done_stops_what_is_left_of_its_end() {
    // After the run's last save, the run stays done. It is not released either, as its files come before its release
    // (Workflow::end_done_run), so a resume finishes its ending.
    let f = Fixture::new(approved());
    let (interrupt, summary) = (f.interrupt.clone(), format!("{}/summary.md", f.state.dir()));
    f.host.state().before_write = Some(Box::new(move |path, _| {
        if path == summary {
            interrupt.raise();
        }
        Ok(())
    }));
    assert_eq!(f.workflow(RunSettings::default()).run(), Err(Stopped::Interrupted));
    let saved = RunState::from_saved(&f.saved()).unwrap();
    assert_eq!((saved.phase.as_str(), saved.error), ("done", None));
    assert!(saved.owner.is_some(), "released before its ending was whole");
    assert!(!f.host.state().files.contains_key(&format!("{}/metrics.json", f.state.dir())));
    assert!(!f.told.titles().iter().any(|t| t.starts_with("Run finished")), "{:?}", f.told.titles());
    assert!(!f.herdr.state().calls.iter().any(|c| matches!(c, Call::Close { .. })));
}

#[test]
fn a_ctrl_c_that_fails_the_done_runs_save_is_the_interrupt_not_a_failure() {
    // On a ProcessHost the save is a command, which the Ctrl-C fails: that is the interrupt, not an error the CLI
    // would print.
    let f = Fixture::new(approved());
    let (interrupt, state) = (f.interrupt.clone(), format!("{}/state.json", f.state.dir()));
    f.host.state().before_write = Some(Box::new(move |path, text| {
        if path == state && text.contains("\"phase\":\"done\"") {
            interrupt.raise();
            return Err(crate::error::OrchestratorError(format!("{path}: interrupted")));
        }
        Ok(())
    }));
    assert_eq!(f.workflow(RunSettings::default()).run(), Err(Stopped::Interrupted));
    assert_eq!(RunState::from_saved(&f.saved()).unwrap().error, None);
}

/// A run resumed at publish, with locked files, whose human presses Ctrl-C at its first git_command; the fixture, once
/// its run has stopped as interrupted.
fn interrupted_at_the_end(git_command: &'static str) -> Fixture {
    let mut state = saved_run("publish", 1, &["build"]);
    state.pull_request = true;
    (state.base_branch, state.branch) = (Some("main".into()), Some("orchestrator/x-a1b2c3".into()));
    (state.verdict, state.workflow) = (Some(crate::constants::FINISHED.into()), "solo".into());
    state.locked = [("build", vec!["tests/test_x.py".to_string()])].into_iter().collect();
    let host = Rc::new(FakeHost::new(Some("def456"), "orchestrator/x-a1b2c3"));
    host.state().refs.insert(format!("{}build/1", crate::state::refs_prefix(&state.run_id)), "abc123".to_string());
    let f = Fixture::resume(Rc::clone(&host), state, vec![], &[], &[]);
    let interrupt = f.interrupt.clone();
    host.state().before_git = Some(Box::new(move |_, args| {
        if args.first() == Some(&git_command) {
            interrupt.raise();
        }
    }));
    let settings = RunSettings { pipeline: Some(crate::test_support::solo()), ..f.as_saved() };
    assert_eq!(f.workflow(settings).run(), Err(Stopped::Interrupted));
    assert_eq!(interrupted(&f).phase, "publish");
    f
}

#[test]
fn a_ctrl_c_at_publish_s_last_command_releases_the_run() {
    // A FakeHost lets the command succeed where a ProcessHost fails it: the run is released as interrupted all the
    // same.
    interrupted_at_the_end("switch");
}

#[test]
fn a_ctrl_c_that_fails_a_heartbeat_is_the_interrupt_not_a_missed_beat() {
    let f = Fixture::new(vec![("spec", vec![working()])]);
    let (interrupt, clock, state) = (f.interrupt.clone(), Rc::clone(&f.clock), format!("{}/state.json", f.state.dir()));
    f.host.state().before_write = Some(Box::new(move |path, _| {
        if path == state && clock.now() >= crate::constants::HEARTBEAT_SECONDS as f64 {
            interrupt.raise();
            return Err(crate::error::OrchestratorError(format!("{path}: interrupted")));
        }
        Ok(())
    }));
    assert_eq!(f.workflow(RunSettings::default()).run(), Err(Stopped::Interrupted));
    assert!(!f.told.lines().iter().any(|line| line.starts_with("heartbeat failed")), "{:?}", f.told.lines());
}

#[test]
fn a_publish_without_a_branch_fails_saying_so() {
    // Only a hand-edited state.json lacks one: the branch is recorded before the first editing step.
    let mut state = saved_run("publish", 1, &["build"]);
    state.pull_request = true;
    (state.base_branch, state.verdict) = (Some("main".into()), Some(crate::constants::FINISHED.into()));
    state.workflow = "solo".into();
    let f = Fixture::resume(Rc::new(FakeHost::new(Some("def456"), "main")), state, vec![], &[], &[]);
    let settings = RunSettings { pipeline: Some(crate::test_support::solo()), ..f.as_saved() };
    match f.workflow(settings).run() {
        Err(Stopped::Failed(e)) => assert_eq!(e.0, "a publish before the run has a branch"),
        other => panic!("not failed: {other:?}"),
    }
}

#[test]
fn a_publish_without_a_verdict_fails_saying_so() {
    // Only a hand-edited state.json lacks one: a run reaches publish once its last verdict step has given one.
    let mut state = saved_run("publish", 1, &["spec", "build", "review"]);
    state.pull_request = true;
    (state.base_branch, state.branch) = (Some("main".into()), Some("octoflow/x-a1b2c3".into()));
    let f = Fixture::resume(Rc::new(FakeHost::new(Some("def456"), "octoflow/x-a1b2c3")), state, vec![], &[], &[]);
    match f.workflow(f.as_saved()).run() {
        Err(Stopped::Failed(e)) => assert_eq!(e.0, "a publish before the run has a verdict"),
        other => panic!("not failed: {other:?}"),
    }
}
