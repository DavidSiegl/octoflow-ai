//! A run started in a checkout left on another run's branch, as a failed in-place run leaves it, refuses to take
//! that branch as its base while that run is not closed, and once it is closed while origin does not have the branch.

use super::one_run::{host_with_other_run, OTHER_ID};
use super::worktree::worktree_run;
use super::*;
use crate::run_commands::RunCommands;

const OTHER_BRANCH: &str = "orchestrator/another-task-537935";

/// A FakeHost whose /proj holds run OTHER_ID, failed on its branch OTHER_BRANCH off main and closed when closed,
/// with OTHER_BRANCH checked out.
fn on_failed_runs_branch(closed: bool) -> Rc<FakeHost> {
    let mut saved = vec![
        ("error", Value::from("the Builder changed no files")),
        ("owner", Value::Null),
        ("pull_request", Value::from(true)),
        ("base_branch", Value::from("main")),
        ("branch", Value::from(OTHER_BRANCH)),
    ];
    if closed {
        saved.push(("closed", Value::from("workspace closed")));
    }
    let host = host_with_other_run(0, &saved);
    host.state().branch = OTHER_BRANCH.into();
    host
}

#[test]
fn a_run_on_a_failed_runs_branch_is_refused_before_fetching() {
    for worktree in [false, true] {
        let host = on_failed_runs_branch(false);
        // As origin answers for a branch that only reaches it when its run publishes.
        host.state().fetch_error = Some(format!("fatal: couldn't find remote ref {OTHER_BRANCH}"));
        let f = if worktree { worktree_run(host, None) } else { Fixture::on(host, new_state(), approved()) };
        // The hint is typed as the human invokes the program, in the project the human named.
        let commands = RunCommands::new("of", &["--cwd".into(), "~/my proj".into()]);
        let message = failure(f.workflow(RunSettings { commands, ..RunSettings::default() }).run());
        assert_eq!(
            message,
            format!(
                "/proj is on {OTHER_BRANCH}, the branch of run {OTHER_ID}, which is not closed; check out its base \
                 branch, main, and close that run with `of close 537935 --cwd '~/my proj'` once you are done with it"
            ),
            "worktree: {worktree}"
        );
        assert!(!f.host.state().git_calls.iter().any(|c| c[0] == "fetch"), "worktree: {worktree}");
        assert!(f.herdr.state().calls.is_empty(), "worktree: {worktree}");
    }
}

#[test]
fn a_run_on_a_desktop_app_runs_branch_is_told_to_close_it_from_the_app() {
    let host = on_failed_runs_branch(false);
    let path = format!("{}/state.json", super::one_run::other_dir());
    let mut saved: Value = serde_json::from_str(&host.state().files[&path]).unwrap();
    saved["backend"] = Value::from("acp");
    saved["format"] = Value::from(crate::state::ACP_STATE_FORMAT);
    host.state().files.insert(path, saved.to_string());
    let f = Fixture::on(host, new_state(), approved());
    assert_eq!(
        failure(f.workflow(RunSettings::default()).run()),
        format!(
            "/proj is on {OTHER_BRANCH}, the branch of run {OTHER_ID}, which is not closed; check out its base \
             branch, main, and close that run from the desktop app once you are done with it"
        )
    );
}

#[test]
fn a_run_on_a_closed_runs_branch_origin_lacks_is_refused_before_fetching() {
    for worktree in [false, true] {
        let host = on_failed_runs_branch(true);
        host.state().fetch_error = Some(format!("fatal: couldn't find remote ref {OTHER_BRANCH}"));
        let f = if worktree { worktree_run(host, None) } else { Fixture::on(host, new_state(), approved()) };
        let message = failure(f.workflow(RunSettings::default()).run());
        assert_eq!(
            message,
            format!(
                "/proj is on {OTHER_BRANCH}, the branch of run {OTHER_ID}, which origin does not have; check out its \
                 base branch, main"
            ),
            "worktree: {worktree}"
        );
        assert!(!f.host.state().git_calls.iter().any(|c| c[0] == "fetch"), "worktree: {worktree}");
        assert!(f.herdr.state().calls.is_empty(), "worktree: {worktree}");
    }
}

#[test]
fn a_run_on_a_closed_runs_branch_origin_has_starts() {
    for worktree in [false, true] {
        let host = on_failed_runs_branch(true);
        host.state().remote.insert(OTHER_BRANCH.into(), "c1".into());
        let f = if worktree { worktree_run(host, None) } else { Fixture::on(host, new_state(), approved()) };
        let mut wf = f.workflow(RunSettings::default());
        assert_eq!(wf.run(), verdict(APPROVE), "worktree: {worktree}");
        assert_eq!(wf.state().base_branch.as_deref(), Some(OTHER_BRANCH), "worktree: {worktree}");
    }
}

#[test]
fn a_run_on_a_closed_runs_branch_origin_cannot_be_asked_about_stops_before_fetching() {
    let host = on_failed_runs_branch(true);
    host.state().list_remote_error = Some("fatal: repository 'https://github.com/o/r/' not found".into());
    let f = Fixture::on(host, new_state(), approved());
    let message = failure(f.workflow(RunSettings::default()).run());
    assert!(
        message.starts_with(&format!(
            "could not ask origin whether it has {OTHER_BRANCH}, closed run {OTHER_ID}'s branch: "
        )) && message.contains("repository 'https://github.com/o/r/' not found"),
        "{message}"
    );
    assert!(!f.host.state().git_calls.iter().any(|c| c[0] == "fetch"));
    assert!(f.herdr.state().calls.is_empty());
}

#[test]
fn a_worktree_run_on_a_live_runs_branch_is_told_to_wait_for_it() {
    // An in-place run in its build round, its owner beating 16 s ago, on its branch in the checkout.
    let host = host_with_other_run(16, &[("base_branch", Value::from("main")), ("branch", Value::from(OTHER_BRANCH))]);
    host.state().branch = OTHER_BRANCH.into();
    let f = worktree_run(host, None);
    assert_eq!(
        failure(f.workflow(RunSettings::default()).run()),
        format!(
            "/proj is on {OTHER_BRANCH}, the branch of run {OTHER_ID}, which is running (pid 3354482 on {}, beat 16s \
             ago); wait for it to end, then check out its base branch, main",
            "here"
        )
    );
    assert!(f.herdr.state().calls.is_empty());
}

#[test]
fn a_run_with_no_base_recorded_is_named_by_where_it_started() {
    // An empty base_branch counts as none, as take_base_branch takes the run's own.
    for base in [Value::Null, Value::from("")] {
        let saved = [
            ("error", Value::from("interrupted")),
            ("owner", Value::Null),
            ("branch", Value::from(OTHER_BRANCH)),
            ("base_branch", base.clone()),
        ];
        let host = host_with_other_run(0, &saved);
        host.state().branch = OTHER_BRANCH.into();
        let f = Fixture::on(host, new_state(), approved());
        let message = failure(f.workflow(RunSettings::default()).run());
        assert!(
            message.contains("which is not closed; check out the branch it started from, and close"),
            "{base}: {message}"
        );
    }
}
