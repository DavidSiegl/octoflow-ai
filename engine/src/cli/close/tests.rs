//! close, through main's command on the CLI fixture: a FakeHost and a FakeHerdr whose workspace w1 is open, and on
//! a real repository with a worktree. That a closed run is not resumed is resumable_state's, in
//! engine/src/cli/start/tests.rs. That main connects to --machine's herdr is a check of target parsing here: the
//! fixture is the connection. A case that needs the workspace's close to fail, or the state saved meanwhile, scripts
//! the FakeHerdr: its close_error and its before_call.

use std::rc::Rc;

use indexmap::IndexMap;

use super::*;
use crate::cli::fixture::Cli;
use crate::cli::{Command, Target};
use crate::constants::{APPROVE, EXIT_ERROR, EXIT_INTERRUPTED, HEARTBEAT_SECONDS, STALE_SECONDS, WORKTREES_DIR};
use crate::herdr::fake::Call;
use crate::herdr::{HerdrError, HerdrServer, OWN_SESSION};
use crate::host::tests::Repo;
use crate::host::{Host, GIT_STATUS_ARGS};
use crate::json::object;
use crate::json::Object;
use crate::state::format_problem;
use crate::state::RunState;
use crate::test_support::resumable;
use crate::workflow::fixture::saved_run;
use serde_json::Value;

const CLOSE_ID: &str = "20261001-120000-a1b2c3";
const CLOSE_BRANCH: &str = "orchestrator/x-a1b2c3";

fn close_state() -> String {
    format!("/proj/.octoflow-ai/runs/{CLOSE_ID}/state.json")
}

fn worktree() -> String {
    format!("/proj/{WORKTREES_DIR}/a1b2c3")
}

/// The world of close's cases: the CLI fixture, workspace w1 open, no live pid.
struct Close {
    cli: Cli,
}

/// Fields put over add's defaults.
type Fields<'a> = &'a [(&'a str, Value)];

impl Close {
    fn new() -> Close {
        let mut cli = Cli::new();
        cli.f.herdr.state().workspaces.insert("w1".into());
        cli.f.pid_alive = Box::new(|_| false);
        Close { cli }
    }

    /// A run in workspace w1 that failed, a --worktree run unless worktree is false, whose worktree holds a.py
    /// uncommitted and whose branch is at its base, saved age seconds ago with fields; returns its saved state.
    fn add(&self, age: u64, worktree: bool, fields: Fields) -> Object {
        let mut state = RunState::new(CLOSE_ID, "a task", "/proj");
        state.phase = "build".into();
        state.round = 1;
        state.workspace_id = "w1".into();
        state.base = Some("base1".into());
        state.base_branch = Some("main".into());
        state.branch = Some(CLOSE_BRANCH.into());
        state.error = Some("interrupted".into());
        state.worktree = worktree.then(self::worktree);
        let Value::Object(mut saved) = state.to_json() else { unreachable!("a state is saved as an object") };
        for (key, value) in fields {
            saved.insert(key.to_string(), value.clone());
        }
        let mut host = self.cli.f.host.state();
        host.files.insert(close_state(), format!("{}\n", Value::Object(saved.clone())));
        host.ages.insert(CLOSE_ID.into(), age);
        if worktree {
            host.worktrees.insert(self::worktree(), CLOSE_BRANCH.into());
            host.changed.insert(format!("{}/a.py", self::worktree()));
            host.branches.insert(CLOSE_BRANCH.to_string(), "base1".into());
        } else {
            host.branch = CLOSE_BRANCH.into();
            host.changed.insert("/proj/a.py".into());
        }
        host.ahead.insert(CLOSE_BRANCH.into(), 0);
        saved
    }

    /// main's close of run_ref (a1b2c3 by default) and more flags in /proj: its exit status and standard output.
    fn close(&self, more: &[&str]) -> (i32, String) {
        let mut argv = vec!["close", "--cwd", "/proj"];
        argv.extend(if more.is_empty() { &["a1b2c3"][..] } else { more });
        let status = self.cli.main(&argv);
        (status, self.cli.take_out())
    }

    /// The error close gives.
    fn error(&self) -> String {
        self.cli.error(&["close", "--cwd", "/proj", "a1b2c3"])
    }

    fn saved(&self) -> Object {
        self.cli.saved(CLOSE_ID).as_object().cloned().expect("an object")
    }

    fn closed(&self) -> Value {
        self.saved().get("closed").cloned().unwrap_or(Value::Null)
    }

    /// Everything of the project close might change: files, worktrees, branches and uncommitted changes.
    fn project(&self) -> String {
        let h = self.cli.f.host.state();
        format!("{:?}", (h.files.clone(), h.worktrees.clone(), h.branches.clone(), h.changed.clone()))
    }

    /// The git commands close ran that change a repository, as all but what it reads to decide do.
    fn changing_git_calls(&self) -> Vec<Vec<String>> {
        let reads = |args: &&Vec<String>| {
            args.as_slice() == GIT_STATUS_ARGS
                || matches!(args.first().map(String::as_str), Some("rev-parse" | "rev-list"))
        };
        self.cli.f.host.state().git_calls.iter().filter(|args| !reads(args)).cloned().collect()
    }

    fn calls(&self) -> Vec<Call> {
        self.cli.f.herdr.state().calls.clone()
    }

    fn assert_closed(&self, line: &str) {
        assert_eq!(self.closed(), Value::from(line));
        assert_eq!(self.cli.f.host.state().mtime_kept, [close_state()]);
    }
}

fn closing_w1() -> Vec<Call> {
    vec![Call::Close { workspace: "w1".into() }]
}

fn me() -> Value {
    Value::Object(object([
        ("host", Value::from("here")),
        ("pid", Value::from(4242)),
        ("started_at", Value::from("2026-09-30T07:00:00+00:00")),
    ]))
}

#[test]
fn the_workspace_is_told_and_closed_in_the_server_the_run_saved() {
    let c = Close::new();
    let pane: HerdrServer = serde_json::from_str(r#"{"HERDR_SESSION": "work"}"#).unwrap();
    c.cli.f.herdr.state().server = Some(pane);
    let saved = serde_json::json!({"HERDR_SOCKET_PATH": null, "HERDR_SESSION": OWN_SESSION});
    c.add(STALE_SECONDS + 1, true, &[("herdr_server", saved.clone())]);
    let run_s = serde_json::from_value::<HerdrServer>(saved).ok();
    // The run's workspace is open in its own server, not in the one close's environment reaches.
    c.cli.f.herdr.state().foreign_workspaces.insert((run_s.clone().unwrap(), "w1".into()));
    assert_eq!(c.close(&[]).0, 0);
    let reached = c.cli.f.herdr.state().reached.clone();
    assert_eq!(reached, [("w1".to_string(), run_s.clone()), ("w1".to_string(), run_s)]);
    assert_eq!(c.calls(), closing_w1());
}

#[test]
fn a_run_that_saved_no_server_is_closed_in_the_environment_s() {
    let c = Close::new();
    let pane: HerdrServer = serde_json::from_str(r#"{"HERDR_SESSION": "work"}"#).unwrap();
    c.cli.f.herdr.state().server = Some(pane.clone());
    c.add(STALE_SECONDS + 1, true, &[]);
    assert_eq!(c.close(&[]).0, 0);
    let reached = c.cli.f.herdr.state().reached.clone();
    assert_eq!(reached, [("w1".to_string(), Some(pane.clone())), ("w1".to_string(), Some(pane))]);
}

#[test]
fn the_sweep_gives_each_run_s_outcome() {
    let c = Close::new();
    c.add(STALE_SECONDS + 1, true, &[]);
    let out = |sweep| c.cli.with(|world| close_all(&Closer::of(world), "/proj", sweep).unwrap());
    // FakeHost's git status names a change by its whole path.
    let discarded = vec![format!("?? {}/a.py", worktree())];
    let preview = format!("close workspace w1; delete worktree {}; delete branch {CLOSE_BRANCH}", worktree());
    let closed = |done: String| {
        vec![RunClosed { run_id: CLOSE_ID.into(), outcome: Outcome::Closed { done, discarded: discarded.clone() } }]
    };
    assert_eq!(out(Sweep::DryRun), closed(preview));
    let done = "workspace closed; worktree deleted; branch deleted".to_string();
    assert_eq!(out(Sweep::Close), closed(done.clone()));
    assert_eq!(out(Sweep::Close), [RunClosed { run_id: CLOSE_ID.into(), outcome: Outcome::AlreadyClosed(done) }]);
}

#[test]
fn a_sweep_of_a_preview_closes_only_what_it_showed() {
    let c = Close::new();
    c.add(STALE_SECONDS + 1, true, &[]);
    let sweep = |sweep| c.cli.with(|world| close_all(&Closer::of(world), "/proj", sweep).unwrap());
    let preview = sweep(Sweep::DryRun);

    // Not in the preview, as a run the preview found running: not looked at.
    assert_eq!(sweep(Sweep::Previewed(&[])), []);
    assert_eq!(c.closed(), Value::Null);

    // Changed since: its worktree holds a change the preview did not show.
    let wt = worktree();
    c.cli.f.host.state().changed.insert(format!("{wt}/b.py"));
    let now = format!("close workspace w1; delete worktree {wt}; delete branch {CLOSE_BRANCH}");
    let skipped = format!("run {CLOSE_ID} changed since the preview: closing it would now {now}");
    assert_eq!(
        sweep(Sweep::Previewed(&preview)),
        [RunClosed { run_id: CLOSE_ID.into(), outcome: Outcome::Skipped(skipped) }]
    );
    assert_eq!(c.closed(), Value::Null);
    assert!(c.cli.f.host.state().worktrees.contains_key(&wt));

    // As previewed: closed.
    let preview = sweep(Sweep::DryRun);
    let Outcome::Closed { discarded, .. } = &preview[0].outcome else { panic!("{preview:?}") };
    let closed = sweep(Sweep::Previewed(&preview));
    let done = "workspace closed; worktree deleted; branch deleted".to_string();
    assert_eq!(
        closed,
        [RunClosed { run_id: CLOSE_ID.into(), outcome: Outcome::Closed { done, discarded: discarded.clone() } }]
    );
}

#[test]
fn a_sweep_s_close_that_fails_after_the_worktree_went_gives_what_went_with_it() {
    let c = Close::new();
    c.add(STALE_SECONDS + 1, true, &[locked()]);
    c.cli.f.host.state().delete_refs_error = Some("fatal: cannot lock ref".into());
    let runs = c.cli.with(|world| close_all(&Closer::of(world), "/proj", Sweep::Close).unwrap());
    let error = "could not delete the refs under refs/orchestrator/a1b2c3/: fatal: cannot lock ref".to_string();
    let discarded = vec![format!("?? {}/a.py", worktree())];
    assert_eq!(runs, [RunClosed { run_id: CLOSE_ID.into(), outcome: Outcome::Failed { error, discarded } }]);
}

#[test]
fn a_sweep_s_close_that_fails_with_the_worktree_still_there_discarded_nothing() {
    let c = Close::new();
    c.add(STALE_SECONDS + 1, true, &[]);
    c.cli.f.host.state().remove_error = Some("fatal: cannot remove a locked working tree".into());
    let runs = c.cli.with(|world| close_all(&Closer::of(world), "/proj", Sweep::Close).unwrap());
    let Outcome::Failed { error, discarded } = &runs[0].outcome else { panic!("{runs:?}") };
    assert!(error.contains("cannot remove a locked working tree"), "{error}");
    assert_eq!(discarded, &Vec::<String>::new());
}

#[test]
fn running_run_is_refused_untouched() {
    let c = Close::new();
    c.add(0, true, &[("error", Value::Null), ("owner", me())]);
    let before = c.project();
    assert_eq!(c.close(&[]), (EXIT_ERROR, String::new()));
    assert_eq!(
        c.error(),
        format!("run {CLOSE_ID} is still running (pid 4242 on here, beat 0s ago); stop it before closing it")
    );
    assert!(c.calls().is_empty());
    assert!(c.cli.f.host.state().git_calls.is_empty() && c.cli.f.host.state().writes.is_empty());
    assert_eq!(c.project(), before);
}

#[test]
fn failed_worktree_run_loses_workspace_worktree_and_branch() {
    let c = Close::new();
    c.add(0, true, &[]);
    let (status, out) = c.close(&[]);
    assert_eq!(status, 0);
    let wt = worktree();
    assert_eq!(
        out,
        format!(
            "closed workspace w1\ndiscarding in {wt}:\n    ?? {wt}/a.py\ndeleted worktree {wt}\n\
             deleted branch {CLOSE_BRANCH}\nclosed run {CLOSE_ID}: workspace closed; worktree deleted; branch deleted\n"
        )
    );
    assert_eq!(c.calls(), closing_w1());
    {
        let h = c.cli.f.host.state();
        let removal = ("/proj".to_string(), ["worktree", "remove", "--force", &wt].map(String::from).to_vec());
        assert!(h.git_log.contains(&removal), "{:?}", h.git_log);
        assert!(h.worktrees.is_empty() && h.changed.is_empty() && h.branches.is_empty());
    }
    c.assert_closed("workspace closed; worktree deleted; branch deleted");
}

#[test]
fn a_clean_worktree_lists_nothing_to_discard() {
    let c = Close::new();
    c.add(0, true, &[]);
    c.cli.f.host.state().changed.clear();
    assert!(!c.close(&[]).1.contains("discarding"));
}

#[test]
fn branch_with_commits_beyond_base_is_kept() {
    for (n, why) in [(1, "1 commit beyond base"), (3, "3 commits beyond base")] {
        let c = Close::new();
        c.add(0, true, &[]);
        c.cli.f.host.state().ahead.insert(CLOSE_BRANCH.into(), n);
        let (status, out) = c.close(&[]);
        assert_eq!(status, 0);
        assert!(out.contains(&format!("kept branch {CLOSE_BRANCH}: {why}\n")), "{out}");
        assert!(c.cli.f.host.state().branches.contains_key(CLOSE_BRANCH));
        assert!(c.cli.f.host.state().worktrees.is_empty());
        c.assert_closed(&format!("workspace closed; worktree deleted; branch kept: {why}"));
    }
}

#[test]
fn branch_of_a_run_without_a_base_is_kept() {
    let c = Close::new();
    c.add(0, true, &[("base", Value::Null)]);
    c.close(&[]);
    assert!(c.cli.f.host.state().branches.contains_key(CLOSE_BRANCH));
    c.assert_closed("workspace closed; worktree deleted; branch kept: the run has no base to compare it with");
}

#[test]
fn stale_runs_are_cleaned_up_as_failed_ones() {
    let cases: [(u64, Vec<(&str, Value)>); 2] = [
        (STALE_SECONDS + 1, vec![("error", Value::Null)]),
        (HEARTBEAT_SECONDS + 1, vec![("error", Value::Null), ("owner", me())]),
    ];
    for (age, fields) in cases {
        let c = Close::new();
        c.add(age, true, &fields);
        assert_eq!(c.close(&[]).0, 0);
        assert_eq!(c.calls(), closing_w1());
        assert!(c.cli.f.host.state().worktrees.is_empty() && c.cli.f.host.state().branches.is_empty());
        c.assert_closed("workspace closed; worktree deleted; branch deleted");
    }
}

#[test]
fn failed_in_place_run_only_loses_its_workspace() {
    let c = Close::new();
    c.add(0, false, &[]);
    assert_eq!(c.close(&[]), (0, format!("closed workspace w1\nclosed run {CLOSE_ID}: workspace closed\n")));
    assert_eq!(c.calls(), closing_w1());
    {
        let h = c.cli.f.host.state();
        assert!(h.git_calls.is_empty());
        assert_eq!(h.branch, CLOSE_BRANCH);
        assert_eq!(h.changed.iter().collect::<Vec<_>>(), ["/proj/a.py"]);
    }
    c.assert_closed("workspace closed");
}

#[test]
fn finished_no_pr_worktree_run_keeps_its_worktree() {
    let c = Close::new();
    c.add(0, true, &[("phase", Value::from("done")), ("error", Value::Null), ("verdict", Value::from(APPROVE))]);
    let (status, out) = c.close(&[]);
    assert_eq!(status, 0);
    let wt = worktree();
    assert!(out.contains(&format!("worktree kept: {wt}")), "{out}");
    assert_eq!(c.calls(), closing_w1());
    {
        let h = c.cli.f.host.state();
        assert_eq!(h.worktrees.get(&wt).map(String::as_str), Some(CLOSE_BRANCH));
        assert_eq!(h.changed.iter().cloned().collect::<Vec<_>>(), [format!("{wt}/a.py")]);
        assert!(h.branches.contains_key(CLOSE_BRANCH));
    }
    c.assert_closed(&format!("workspace closed; worktree kept: {wt}"));
}

#[test]
fn finished_worktree_run_whose_worktree_went_with_its_pull_request() {
    let c = Close::new();
    let pr = ("pr_url", Value::from("https://github.com/o/r/pull/7"));
    c.add(0, true, &[("phase", Value::from("done")), ("error", Value::Null), ("verdict", Value::from(APPROVE)), pr]);
    c.cli.f.host.state().worktrees = IndexMap::new();
    c.close(&[]);
    c.assert_closed("workspace closed");
}

#[test]
fn a_workspace_herdr_lacks_is_already_gone() {
    for workspace in ["w1", ""] {
        let c = Close::new();
        c.cli.f.herdr.state().workspaces.clear();
        c.add(0, true, &[("workspace_id", Value::from(workspace))]);
        let (status, out) = c.close(&[]);
        assert_eq!(status, 0);
        assert!(!out.contains("closed workspace"));
        assert!(c.calls().is_empty());
        c.assert_closed("workspace already gone; worktree deleted; branch deleted");
    }
}

#[test]
fn a_worktree_already_gone_still_has_its_branch_deleted() {
    let c = Close::new();
    c.add(0, true, &[]);
    c.cli.f.host.state().worktrees = IndexMap::new();
    c.close(&[]);
    assert!(c.cli.f.host.state().branches.is_empty());
    c.assert_closed("workspace closed; worktree already gone; branch deleted");

    let c = Close::new();
    c.add(0, true, &[]);
    c.cli.f.host.state().worktrees = IndexMap::new();
    c.cli.f.host.state().branches = IndexMap::new();
    c.close(&[]);
    c.assert_closed("workspace closed; worktree already gone");
}

#[test]
fn second_close_only_prints_the_record() {
    let c = Close::new();
    c.add(0, true, &[]);
    c.close(&[]);
    c.cli.f.herdr.state().calls.clear();
    c.cli.f.host.state().git_calls.clear();
    let writes = c.cli.f.host.state().writes.clone();
    assert_eq!(
        c.close(&[]),
        (0, format!("run {CLOSE_ID} is already closed: workspace closed; worktree deleted; branch deleted\n"))
    );
    assert!(c.calls().is_empty() && c.cli.f.host.state().git_calls.is_empty());
    assert_eq!(c.cli.f.host.state().writes, writes);
}

#[test]
fn resume_of_a_closed_run_exits_1() {
    let c = Close::new();
    c.add(0, true, &[]);
    c.close(&[]);
    for flags in [&[][..], &["--force"][..]] {
        let argv: Vec<&str> = ["resume", "--cwd", "/proj", "a1b2c3"].iter().chain(flags).copied().collect();
        assert_eq!(c.cli.main(&argv), EXIT_ERROR);
        assert!(c.cli.error(&argv).contains(&format!("run {CLOSE_ID} was closed; start a new run")));
    }
}

#[test]
fn state_saved_before_close_existed_loads_and_resumes() {
    let Value::Object(mut saved) = saved_run("build", 1, &["spec", "build"]).to_json() else { unreachable!() };
    saved.remove("closed");
    assert_eq!(resumable(&[], &saved).unwrap().closed, None);
}

#[test]
fn state_saved_meanwhile_is_not_overwritten() {
    let c = Close::new();
    c.add(0, true, &[]);
    // As the workspace closes.
    let host = Rc::clone(&c.cli.f.host);
    c.cli.f.herdr.state().before_call = Some(Box::new(move |call| {
        if matches!(call, Call::Close { .. }) {
            let text = host.state().files.get(&close_state()).cloned().unwrap();
            let Ok(Value::Object(mut saved)) = serde_json::from_str::<Value>(&text) else { unreachable!() };
            saved.insert("round".into(), Value::from(2));
            host.state().files.insert(close_state(), Value::Object(saved).to_string());
        }
    }));
    assert_eq!(c.close(&[]).0, EXIT_ERROR);
    let saved = c.saved();
    assert_eq!((saved.get("round"), saved.get("closed")), (Some(&Value::from(2)), Some(&Value::Null)));
}

#[test]
fn a_run_an_earlier_release_saved_is_left_to_that_release() {
    // Its state.json has no format: close says why (crate::state::format_problem) and touches nothing.
    let c = Close::new();
    let mut saved = c.add(0, true, &[]);
    saved.shift_remove("format");
    let text = format!("{}\n", Value::Object(saved));
    c.cli.f.host.state().files.insert(close_state(), text.clone());
    let problem = format_problem(&c.saved()).unwrap();
    assert_eq!(c.error(), problem);
    assert!(c.cli.f.herdr.state().calls.is_empty() && c.cli.f.host.state().git_calls.is_empty());
    assert_eq!(c.cli.f.host.state().files.get(&close_state()), Some(&text));
}

#[test]
fn the_error_of_a_state_saved_meanwhile_names_the_file() {
    let c = Close::new();
    c.add(0, true, &[]);
    let host = Rc::clone(&c.cli.f.host);
    c.cli.f.herdr.state().before_call = Some(Box::new(move |_| {
        let path = close_state();
        let text = host.state().files.get(&path).cloned().unwrap();
        host.state()
            .files
            .insert(path, text.replace("\"round\": 1", "\"round\": 2").replace("\"round\":1", "\"round\":2"));
    }));
    assert_eq!(c.error(), format!("{} changed while the run was closed; closed is not recorded", close_state()));
}

#[test]
fn herdr_failure_records_nothing() {
    let c = Close::new();
    c.add(0, true, &[]);
    c.cli.f.herdr.state().close_error = Some(HerdrError::new("server_error", "herdr is down"));
    assert_eq!(c.close(&[]).0, EXIT_ERROR);
    assert!(c.error().contains("herdr is down"));
    assert_eq!(c.changing_git_calls(), Vec::<Vec<String>>::new());
    assert!(c.cli.f.host.state().writes.is_empty() && c.cli.f.host.state().worktrees.contains_key(&worktree()));
    assert_eq!(c.closed(), Value::Null);
}

#[test]
fn git_failure_records_nothing_and_a_second_close_finishes() {
    let c = Close::new();
    c.add(0, true, &[]);
    c.cli.f.host.state().remove_error = Some("fatal: cannot remove a locked working tree".into());
    assert_eq!(c.close(&[]).0, EXIT_ERROR);
    // A second close, which fails as the first did, the workspace gone with the first.
    assert!(c.error().contains("cannot remove a locked working tree"));
    assert!(c.cli.f.host.state().writes.is_empty());
    assert_eq!(c.closed(), Value::Null);
    assert!(c.cli.f.host.state().branches.contains_key(CLOSE_BRANCH));

    c.cli.f.host.state().remove_error = None;
    assert_eq!(c.close(&[]).0, 0);
    c.assert_closed("workspace already gone; worktree deleted; branch deleted");
}

const OTHER_REF: &str = "refs/orchestrator/d4e5f6/locked/tests";

/// What a run whose steps tests and build locked a file each saves as its locked.
fn locked() -> (&'static str, Value) {
    ("locked", serde_json::json!({"tests": ["tests/test_a.py"], "build": ["a.py"]}))
}

impl Close {
    /// Give the run two locked snapshots, and a run of another key one (OTHER_REF); the run's state must say it
    /// locked (locked()).
    fn add_refs(&self) {
        let refs = &mut self.cli.f.host.state().refs;
        for git_ref in ["refs/orchestrator/a1b2c3/locked/tests", "refs/orchestrator/a1b2c3/locked/build", OTHER_REF] {
            refs.insert(git_ref.to_string(), "snap1".to_string());
        }
    }

    fn refs(&self) -> Vec<String> {
        self.cli.f.host.state().refs.keys().map(String::from).collect()
    }
}

#[test]
fn every_closed_run_loses_its_refs_and_only_its() {
    let wt = worktree();
    let failed = [locked()];
    let not_failed = [("error", Value::Null), locked()];
    let finished =
        [("phase", Value::from("done")), ("error", Value::Null), ("verdict", Value::from(APPROVE)), locked()];
    let discarded = "workspace closed; worktree deleted; branch deleted; refs deleted";
    let cases: [(u64, bool, Fields, String); 4] = [
        (0, true, &failed, discarded.into()),
        (0, false, &failed, "workspace closed; refs deleted".into()),
        (STALE_SECONDS + 1, true, &not_failed, discarded.into()),
        (0, true, &finished, format!("workspace closed; worktree kept: {wt}; refs deleted")),
    ];
    for (age, in_worktree, fields, line) in cases {
        let c = Close::new();
        c.add(age, in_worktree, fields);
        c.add_refs();
        let (status, out) = c.close(&[]);
        assert_eq!(status, 0);
        assert!(out.contains("deleted the refs under refs/orchestrator/a1b2c3/\n"), "{out}");
        assert!(out.ends_with(&format!("closed run {CLOSE_ID}: {line}\n")), "{out}");
        assert_eq!(c.refs(), [OTHER_REF]);
        c.assert_closed(&line);
    }
}

#[test]
fn failing_to_delete_the_refs_records_nothing_and_a_second_close_finishes() {
    let c = Close::new();
    c.add(0, false, &[locked()]);
    c.add_refs();
    c.cli.f.host.state().delete_refs_error = Some("fatal: cannot lock ref".into());
    assert_eq!(c.close(&[]).0, EXIT_ERROR);
    assert_eq!(c.error(), "could not delete the refs under refs/orchestrator/a1b2c3/: fatal: cannot lock ref");
    assert_eq!(c.closed(), Value::Null);
    assert_eq!(c.refs().len(), 3);

    c.cli.f.host.state().delete_refs_error = None;
    assert_eq!(c.close(&[]).0, 0);
    assert_eq!(c.refs(), [OTHER_REF]);
    c.assert_closed("workspace already gone; refs deleted");
}

#[test]
fn refs_a_failed_close_deleted_are_still_recorded_as_deleted() {
    // As after a close that deleted some or all of them and then failed.
    let c = Close::new();
    c.add(0, false, &[locked()]);
    assert_eq!(c.close(&[]).0, 0);
    c.assert_closed("workspace closed; refs deleted");
}

#[test]
fn refs_of_a_run_whose_first_lock_failed_are_deleted_too() {
    // Host::lock makes the ref before it can fail, and the run saves locked only once it has not.
    let c = Close::new();
    c.add(0, false, &[]);
    c.add_refs();
    assert_eq!(c.close(&[]).0, 0);
    assert_eq!(c.refs(), [OTHER_REF]);
    c.assert_closed("workspace closed; refs deleted");
}

#[test]
fn a_done_run_still_ending_is_refused_untouched() {
    // Its orchestrator may be deleting its refs (Workflow::clean_up_locks) as close would.
    let c = Close::new();
    c.add(0, true, &[("phase", Value::from("done")), ("error", Value::Null), ("owner", me()), locked()]);
    c.add_refs();
    let before = c.project();
    assert_eq!(c.close(&[]), (EXIT_ERROR, String::new()));
    assert_eq!(
        c.error(),
        format!("run {CLOSE_ID} is still running (pid 4242 on here, beat 0s ago); stop it before closing it")
    );
    assert!(c.calls().is_empty());
    assert_eq!((c.project(), c.refs().len()), (before, 3));
}

#[test]
fn a_done_run_that_stopped_while_ending_closes_as_a_finished_one() {
    // RunState::health calls it stale; its worktree is still a --no-pr run's change.
    let c = Close::new();
    let ended = [("phase", Value::from("done")), ("error", Value::Null), ("owner", me()), locked()];
    c.add(STALE_SECONDS + 1, true, &ended);
    c.add_refs();
    assert_eq!(c.close(&[]).0, 0);
    c.assert_closed(&format!("workspace closed; worktree kept: {}; refs deleted", worktree()));
}

/// close_run of a1b2c3 on this machine's host, in a temporary directory that is no git repository, of a failed run
/// that locked what locked says: what close gives, and the run's saved closed after it, null when not recorded.
fn close_outside_a_repository(locked: &[(&str, Vec<String>)]) -> (Result<String, OrchestratorError>, Value) {
    let tmp = crate::test_support::TempDir::new();
    let host = crate::host::ProcessHost::local(&crate::interrupt::Interrupt::new());
    let mut state = RunState::new(CLOSE_ID, "x", &tmp.path());
    (state.phase, state.round, state.error) = ("build".into(), 1, Some("interrupted".into()));
    for (step, paths) in locked {
        state.locked.insert(step.to_string(), paths.clone());
    }
    let path = format!("{}/state.json", state.dir());
    host.write(&path, &state.to_text(), false).unwrap();
    let mut cli = Cli::new();
    cli.f.pid_alive = Box::new(|_| false);
    let closed =
        cli.with(|world| close_run(&Closer { host: &host, ..Closer::of(world) }, &tmp.path(), "a1b2c3", false));
    let saved = serde_json::from_str::<Value>(&host.read(&path).unwrap().unwrap()).unwrap();
    (closed, saved.get("closed").cloned().unwrap_or(Value::Null))
}

#[test]
fn a_run_outside_a_git_repository_closes_without_git() {
    // A --no-pr run is how a directory that is no repository is run (Workflow's start check); it locked nothing.
    let (closed, saved) = close_outside_a_repository(&[]);
    assert_eq!(closed.unwrap(), "workspace already gone");
    assert_eq!(saved, Value::from("workspace already gone"));
}

#[test]
fn a_run_that_locked_in_a_directory_git_now_refuses_is_not_closed() {
    // Its refs are somewhere git cannot reach now, as in a checkout git calls of dubious ownership: closing it
    // would record refs deleted while they stay. Fixing git and closing again finishes.
    let (closed, saved) = close_outside_a_repository(&[("tests", vec!["tests/test_a.py".to_string()])]);
    let error = closed.unwrap_err().0;
    assert!(error.starts_with("could not delete the refs under refs/orchestrator/a1b2c3/: "), "{error}");
    assert_eq!(saved, Value::Null);
}

#[test]
fn on_a_machine() {
    let c = Close::new();
    c.add(0, true, &[]);
    assert_eq!(c.close(&["a1b2c3", "--machine", "m"]).0, 0);
    let target = Target { machine: Some("m".into()), cwd: Some("/proj".into()) };
    assert_eq!(c.cli.command(&["close", "--cwd", "/proj", "a1b2c3", "--machine", "m"]).target(), Some(&target));
}

#[test]
fn args() {
    let c = Cli::new();
    let target = Target { machine: Some("m".into()), cwd: Some("~/proj".into()) };
    assert_eq!(
        c.command(&["close", "a1b2c3", "--machine", "m", "--cwd", "~/proj"]),
        Command::Close { target, run_ref: Some("a1b2c3".into()), dry_run: false }
    );
    for argv in [&["close", "a1b2c3", "--machine", "m"][..], &["close"][..]] {
        let refused = crate::cli::parse_args(argv.iter().map(std::ffi::OsString::from), &c.config).unwrap_err();
        assert_eq!(refused.exit_code(), 2, "{argv:?}");
    }
}

// -- close in a real repository --------------------------------------------------------------------------------------

#[test]
fn close_on_real_git() {
    // A failed --worktree run: its dirty worktree is removed by force, and its branch, at base, deleted. The user's
    // status.showUntrackedFiles=no does not hide the new file from the list of what goes with the worktree.
    let repo = Repo::new(&[("app.py", "one\n"), (".octoflow-ai/.gitignore", "*\n")]);
    repo.git(&["config", "status.showUntrackedFiles", "no"]);
    let base = repo.base().to_string();
    let wt = format!("{}/{WORKTREES_DIR}/a1b2c3", repo.path);
    repo.host.add_worktree(&repo.path, &wt, &base).unwrap();
    repo.host.git(&wt, &["switch", "--quiet", "-c", CLOSE_BRANCH]).unwrap();
    crate::host::tests::put(&format!("{wt}/app.py"), "changed\n");
    crate::host::tests::put(&format!("{wt}/new.py"), "new\n");
    let mut state = RunState::new(CLOSE_ID, "x", &repo.path);
    state.phase = "build".into();
    state.round = 1;
    state.workspace_id = "w1".into();
    state.base = Some(base);
    state.base_branch = Some("main".into());
    state.branch = Some(CLOSE_BRANCH.into());
    state.error = Some("interrupted".into());
    state.worktree = Some(wt.clone());
    state.locked.insert("tests".to_string(), vec!["app.py".to_string()]);
    let path = format!("{}/state.json", state.dir());
    repo.host.write(&path, &state.to_text(), false).unwrap();
    crate::host::tests::set_mtime(&path, 1_000_000_000);
    let other = "refs/orchestrator/d4e5f6/locked/tests";
    for git_ref in [state.lock_ref("tests").as_str(), other] {
        repo.git(&["update-ref", git_ref, state.base.as_deref().unwrap()]);
    }
    let mut cli = Cli::new();
    cli.f.pid_alive = Box::new(|_| false);
    let closed = cli
        .with(|world| close_run(&Closer { host: &repo.host, ..Closer::of(world) }, &repo.path, "a1b2c3", false))
        .unwrap();
    assert_eq!(closed, "workspace already gone; worktree deleted; branch deleted; refs deleted");
    assert_eq!(repo.git(&["for-each-ref", "--format=%(refname)", "refs/orchestrator/"]), format!("{other}\n"));
    let out = cli.take_out();
    assert!(out.contains(&format!("discarding in {wt}:\n     M app.py\n    ?? new.py\n")), "{out}");
    assert!(!std::path::Path::new(&wt).exists());
    assert!(!repo.git(&["worktree", "list", "--porcelain"]).contains(&wt));
    assert_eq!(repo.host.branch_commit(&repo.path, CLOSE_BRANCH).unwrap(), None);
    let saved = serde_json::from_str::<Value>(&repo.host.read(&path).unwrap().unwrap()).unwrap();
    assert_eq!(saved.get("closed"), Some(&Value::from(closed.as_str())));
    assert_eq!(repo.host.mtime(&path).unwrap(), 1_000_000_000);
    assert!(cli.f.herdr.state().calls.is_empty());
}

// -- close --all ----------------------------------------------------------------------------------------------------

/// The five runs of the spec's project, in run id order: finished in place, failed in a worktree (CLOSE_ID's, with
/// a.py uncommitted), stale in place, running, and closed. Each but the closed one has a workspace of its own.
const FINISHED: &str = "20261001-110000-aaaaa1";
const STALE: &str = "20261001-130000-ccccc3";
const LIVE: &str = "20261001-140000-ddddd4";
const CLOSED: &str = "20261001-150000-eeeee5";

impl Close {
    /// Save an in-place run of id in workspace w-<id>, open in herdr, saved age seconds ago with fields over a failed
    /// run's.
    fn add_in_place(&self, id: &str, age: u64, fields: Fields) {
        let mut state = RunState::new(id, "a task", "/proj");
        (state.phase, state.round, state.error) = ("build".into(), 1, Some("interrupted".into()));
        state.workspace_id = format!("w-{id}");
        self.cli.f.herdr.state().workspaces.insert(state.workspace_id.clone());
        let Value::Object(mut saved) = state.to_json() else { unreachable!("a state is saved as an object") };
        for (key, value) in fields {
            saved.insert(key.to_string(), value.clone());
        }
        self.cli.save(&Value::Object(saved), age);
    }

    /// The five runs.
    fn project_of_five() -> Close {
        let c = Close::new();
        let finished = [("phase", Value::from("done")), ("error", Value::Null), ("verdict", Value::from(APPROVE))];
        c.add_in_place(FINISHED, 0, &finished);
        c.add(0, true, &[]);
        c.add_in_place(STALE, STALE_SECONDS + 1, &[("error", Value::Null)]);
        c.add_in_place(LIVE, 0, &[("error", Value::Null), ("owner", me())]);
        c.add_in_place(CLOSED, 0, &[("closed", Value::from("workspace closed"))]);
        c
    }

    fn closed_of(&self, id: &str) -> Value {
        self.cli.saved(id).get("closed").cloned().unwrap_or(Value::Null)
    }
}

fn running_skipped() -> String {
    format!("skipped: run {LIVE} is still running (pid 4242 on here, beat 0s ago); stop it before closing it")
}

#[test]
fn close_all_closes_what_close_would_and_skips_the_running_run() {
    let c = Close::project_of_five();
    let (status, out) = c.close(&["--all"]);
    assert_eq!(status, 0);
    let wt = worktree();
    assert_eq!(
        out,
        format!(
            "closed workspace w-{FINISHED}\nclosed run {FINISHED}: workspace closed\n\
             closed workspace w1\ndiscarding in {wt}:\n    ?? {wt}/a.py\ndeleted worktree {wt}\n\
             deleted branch {CLOSE_BRANCH}\nclosed run {CLOSE_ID}: workspace closed; worktree deleted; branch deleted\n\
             closed workspace w-{STALE}\nclosed run {STALE}: workspace closed\n\
             {}\n\
             3 closed, 1 skipped, 1 already closed\n",
            running_skipped()
        )
    );
    for id in [FINISHED, STALE] {
        assert_eq!(c.closed_of(id), Value::from("workspace closed"));
    }
    assert_eq!(c.closed_of(CLOSE_ID), Value::from("workspace closed; worktree deleted; branch deleted"));
    assert_eq!((c.closed_of(LIVE), c.closed_of(CLOSED)), (Value::Null, Value::from("workspace closed")));
    assert!(c.cli.f.herdr.state().workspaces.contains(&format!("w-{LIVE}")));
}

#[test]
fn close_all_dry_run_says_what_each_would_lose_and_changes_nothing() {
    let c = Close::project_of_five();
    c.add_refs();
    let before = (c.project(), c.refs());
    let (status, out) = c.close(&["--all", "--dry-run"]);
    assert_eq!(status, 0);
    let wt = worktree();
    assert_eq!(
        out,
        format!(
            "would close run {FINISHED}: close workspace w-{FINISHED}\n\
             would discard in {wt}:\n    ?? {wt}/a.py\n\
             would close run {CLOSE_ID}: close workspace w1; delete worktree {wt}; delete branch {CLOSE_BRANCH}; \
             delete the refs under refs/orchestrator/a1b2c3/\n\
             would close run {STALE}: close workspace w-{STALE}\n\
             {}\n\
             3 to close, 1 skipped, 1 already closed\n",
            running_skipped()
        )
    );
    assert_eq!((c.project(), c.refs()), before);
    assert!(c.calls().is_empty() && c.cli.f.host.state().writes.is_empty());
    assert_eq!(c.changing_git_calls(), Vec::<Vec<String>>::new());
}

#[test]
fn a_dry_run_of_one_run_changes_nothing() {
    let c = Close::new();
    c.add(0, true, &[]);
    let before = c.project();
    let wt = worktree();
    assert_eq!(
        c.close(&["a1b2c3", "--dry-run"]),
        (
            0,
            format!(
                "would discard in {wt}:\n    ?? {wt}/a.py\n\
                 would close run {CLOSE_ID}: close workspace w1; delete worktree {wt}; delete branch {CLOSE_BRANCH}\n"
            )
        )
    );
    assert_eq!(c.project(), before);
    assert!(c.calls().is_empty() && c.cli.f.host.state().writes.is_empty());
    assert_eq!(c.changing_git_calls(), Vec::<Vec<String>>::new());
}

#[test]
fn one_failing_close_does_not_stop_the_others_and_fails_the_sweep() {
    let c = Close::project_of_five();
    c.cli.f.host.state().remove_error = Some("fatal: cannot remove a locked working tree".into());
    let (status, out) = c.close(&["--all"]);
    assert_eq!(status, EXIT_ERROR);
    assert!(out.contains(&format!("could not close run {CLOSE_ID}: ")), "{out}");
    assert!(out.contains("cannot remove a locked working tree"), "{out}");
    assert!(out.ends_with("2 closed, 1 skipped, 1 already closed, 1 failed\n"), "{out}");
    assert_eq!(c.closed_of(CLOSE_ID), Value::Null);
    for id in [FINISHED, STALE] {
        assert_eq!(c.closed_of(id), Value::from("workspace closed"));
    }
}

#[test]
fn a_run_that_started_running_since_the_sweep_began_is_skipped() {
    // As the first run's workspace closes, the stale run is resumed: its owner is this process, its beat now.
    let c = Close::project_of_five();
    let host = Rc::clone(&c.cli.f.host);
    let path = format!("/proj/.octoflow-ai/runs/{STALE}/state.json");
    c.cli.f.herdr.state().before_call = Some(Box::new(move |call| {
        if matches!(call, Call::Close { .. }) {
            let mut s = host.state();
            let text = s.files.get(&path).cloned().unwrap();
            let Ok(Value::Object(mut saved)) = serde_json::from_str::<Value>(&text) else { unreachable!() };
            saved.insert("owner".into(), me());
            s.files.insert(path.clone(), Value::Object(saved).to_string());
            s.ages.insert(STALE.into(), 0);
        }
    }));
    let (status, out) = c.close(&["--all"]);
    assert_eq!(status, 0);
    assert!(out.contains(&format!("skipped: run {STALE} is still running")), "{out}");
    assert!(out.ends_with("2 closed, 2 skipped, 1 already closed\n"), "{out}");
    assert_eq!(c.closed_of(STALE), Value::Null);
    assert!(c.cli.f.herdr.state().workspaces.contains(&format!("w-{STALE}")));
}

#[test]
fn a_run_removed_since_the_sweep_began_is_skipped_not_failed() {
    // As the first run's workspace closes, the stale run's directory is removed, as prune or a human would.
    let c = Close::project_of_five();
    let host = Rc::clone(&c.cli.f.host);
    let path = format!("/proj/.octoflow-ai/runs/{STALE}/state.json");
    c.cli.f.herdr.state().before_call = Some(Box::new(move |call| {
        if matches!(call, Call::Close { .. }) {
            host.state().files.shift_remove(&path);
        }
    }));
    let (status, out) = c.close(&["--all"]);
    assert_eq!(status, 0, "{out}");
    assert!(
        out.contains(&format!(
            "skipped: run {STALE} is gone: its state.json was removed from /proj/.octoflow-ai/runs since the sweep \
             began\n"
        )),
        "{out}"
    );
    assert!(out.ends_with("2 closed, 2 skipped, 1 already closed\n"), "{out}");
    assert!(c.cli.f.herdr.state().workspaces.contains(&format!("w-{STALE}")));
}

#[test]
fn an_unreadable_run_is_skipped_and_named_not_failed() {
    let c = Close::new();
    let mut saved = c.add(0, true, &[]);
    saved.shift_remove("format");
    c.cli.f.host.state().files.insert(close_state(), format!("{}\n", Value::Object(saved)));
    let problem = format_problem(&c.saved()).unwrap();
    assert_eq!(c.close(&["--all"]), (0, format!("skipped: {problem}\n0 closed, 1 skipped, 0 already closed\n")));
    assert!(c.calls().is_empty() && c.cli.f.host.state().writes.is_empty());
}

#[test]
fn a_state_json_that_is_not_json_or_names_no_run_is_skipped_and_named_beside_a_run_that_closes() {
    // Either stopped every command that lists the runs, close --all closing nothing (cross-review 194 F1).
    let c = Close::new();
    c.add(1_000_000, false, &[]);
    {
        let files = &mut c.cli.f.host.state().files;
        files.insert("/proj/.octoflow-ai/runs/broken/state.json".into(), "{\"run_id\": \n".into());
        files.insert("/proj/.octoflow-ai/runs/nameless/state.json".into(), "{\"phase\": \"build\"}\n".into());
    }
    let (status, out) = c.close(&["--all"]);
    assert_eq!(status, 0, "{out}");
    assert!(out.contains("skipped: /proj/.octoflow-ai/runs/broken/state.json is not JSON ("), "{out}");
    let nameless = "skipped: /proj/.octoflow-ai/runs/nameless/state.json is not a JSON object with a run_id";
    assert!(out.contains(nameless), "{out}");
    assert!(out.ends_with("1 closed, 2 skipped, 0 already closed\n"), "{out}");
    // close names one by its directory, and refuses it with the same reason.
    let e = c.cli.error(&["close", "--cwd", "/proj", "broken"]);
    assert!(e.contains("/proj/.octoflow-ai/runs/broken/state.json is not JSON ("), "{e}");
}

#[test]
fn an_unreadable_run_that_says_closed_is_skipped_not_counted_as_closed() {
    // As a release after this one could save it: closed is only acted on in a state this engine reads.
    let c = Close::new();
    let mut saved = c.add(0, true, &[("closed", Value::from("workspace closed"))]);
    saved.shift_remove("format");
    c.cli.f.host.state().files.insert(close_state(), format!("{}\n", Value::Object(saved)));
    let problem = format_problem(&c.saved()).unwrap();
    assert_eq!(c.close(&["--all"]), (0, format!("skipped: {problem}\n0 closed, 1 skipped, 0 already closed\n")));
}

#[test]
fn ctrl_c_stops_the_sweep_between_runs() {
    // Raised as the first run's workspace closes: that run is closed in full, the next is not touched.
    let c = Close::project_of_five();
    let interrupt = c.cli.f.interrupt.clone();
    c.cli.f.herdr.state().before_call = Some(Box::new(move |_| interrupt.raise()));
    assert_eq!(c.cli.main(&["close", "--cwd", "/proj", "--all"]), EXIT_INTERRUPTED);
    assert_eq!(c.closed_of(FINISHED), Value::from("workspace closed"));
    assert_eq!(c.closed_of(CLOSE_ID), Value::Null);
    assert!(c.cli.f.host.state().worktrees.contains_key(&worktree()));
}

#[test]
fn all_and_a_run_exclude_each_other() {
    let c = Cli::new();
    let target = Target { machine: None, cwd: Some("/proj".into()) };
    assert_eq!(
        c.command(&["close", "--cwd", "/proj", "--all", "--dry-run"]),
        Command::Close { target, run_ref: None, dry_run: true }
    );
    for argv in [&["close", "--all", "a1b2c3"][..], &["close", "--dry-run"][..]] {
        let refused = crate::cli::parse_args(argv.iter().map(std::ffi::OsString::from), &c.config).unwrap_err();
        assert_eq!(refused.exit_code(), 2, "{argv:?}");
    }
}
