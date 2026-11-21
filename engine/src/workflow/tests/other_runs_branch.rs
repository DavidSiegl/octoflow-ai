//! A run started in a checkout left on another run's branch, as a failed in-place run leaves it, refuses to take
//! that branch as its base until that run is closed.

use super::one_run::{host_with_other_run, OTHER_ID};
use super::worktree::worktree_run;
use super::*;

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
        let message = failure(f.workflow(RunSettings::default()).run());
        assert_eq!(
            message,
            format!(
                "/proj is on {OTHER_BRANCH}, the branch of run {OTHER_ID}, which is not closed; check out its base \
                 branch, main, or close that run with `octoflow close {OTHER_ID}`"
            ),
            "worktree: {worktree}"
        );
        assert!(!f.host.state().git_calls.iter().any(|c| c[0] == "fetch"), "worktree: {worktree}");
        assert!(f.herdr.state().calls.is_empty(), "worktree: {worktree}");
    }
}

#[test]
fn a_run_on_a_closed_runs_branch_starts() {
    let f = Fixture::on(on_failed_runs_branch(true), new_state(), approved());
    let mut wf = f.workflow(RunSettings::default());
    assert_eq!(wf.run(), verdict(APPROVE));
    assert_eq!(wf.state().base_branch.as_deref(), Some(OTHER_BRANCH));
}
