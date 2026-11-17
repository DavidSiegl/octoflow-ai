//! The move of a project's own directory off its old name, in a real git repository with a real worktree of a run
//! under it.

use std::path::Path;

use super::*;
use crate::host::tests::{put, read, set_mtime, Repo};
use crate::state::RunState;

/// A run id as new_run_id makes them, with the key k1a2b3.
const RUN: &str = "20260101-120000-k1a2b3";

/// No process is alive: what pid_alive says of every owner.
fn none_alive(_: i64) -> bool {
    false
}

/// A run of the project at cwd that failed, so nothing drives it (RunState::health None), in the worktree at
/// worktree unless it is None, saved as state.json text.
fn failed_run(cwd: &str, worktree: Option<&str>) -> String {
    let mut state = RunState::new(RUN, "a task", cwd);
    state.error = Some("it failed".to_string());
    state.worktree = worktree.map(str::to_string);
    state.to_text()
}

/// A run of the project at cwd that an engine on another machine drives, its heartbeat fresh, saved as state.json
/// text.
fn live_run(cwd: &str) -> String {
    let mut state = RunState::new(RUN, "a task", cwd);
    let owner = crate::json::object([("host", "elsewhere".into()), ("pid", 4242.into()), ("started_at", "".into())]);
    state.owner = Some(owner);
    state.to_text()
}

#[test]
fn a_project_s_old_directory_moves_with_its_worktree_registered_and_saved_at_the_new_place() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    let old_tree = format!("{cwd}/.orchestrator/worktrees/k1a2b3");
    let new_tree = format!("{cwd}/.octoflow-ai/worktrees/k1a2b3");
    repo.git(&["worktree", "add", "--quiet", "-b", "run-k1a2b3", &old_tree]);
    let state = format!("{cwd}/.orchestrator/runs/{RUN}/state.json");
    put(&state, &failed_run(cwd, Some(&old_tree)));
    set_mtime(&state, 1_700_000_000);

    let moved = move_project_dir(&repo.host, cwd, "here", &none_alive).expect("the move");

    assert_eq!(moved, Some(format!("moved the project's runs from {cwd}/.orchestrator to {cwd}/.octoflow-ai")));
    assert!(!Path::new(&format!("{cwd}/.orchestrator")).exists());
    let new_state = format!("{cwd}/.octoflow-ai/runs/{RUN}/state.json");
    let saved = RunState::from_text(&read(&new_state)).expect("a state");
    assert_eq!(saved.worktree.as_deref(), Some(new_tree.as_str()));
    // The heartbeat stays as old as it was, so a stale run does not look live.
    assert_eq!(repo.host.mtime(&new_state).expect("its mtime"), 1_700_000_000);
    // Git finds the worktree at its new place from both ends.
    assert!(repo.git(&["worktree", "list", "--porcelain"]).contains(&format!("worktree {new_tree}\n")));
    let head = repo.host.git(&new_tree, &["rev-parse", "--abbrev-ref", "HEAD"]).expect("git in the worktree");
    assert_eq!(head, "run-k1a2b3\n");
}

#[test]
fn a_project_whose_new_directory_exists_keeps_both() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    put(&format!("{cwd}/.orchestrator/runs/{RUN}/state.json"), &failed_run(cwd, None));
    put(&format!("{cwd}/.octoflow-ai/.gitignore"), "*\n");

    assert_eq!(move_project_dir(&repo.host, cwd, "here", &none_alive).expect("no move"), None);

    assert!(Path::new(&format!("{cwd}/.orchestrator/runs/{RUN}/state.json")).exists());
    assert!(!Path::new(&format!("{cwd}/.octoflow-ai/runs")).exists());
}

#[test]
fn a_project_without_an_old_directory_has_nothing_to_move() {
    let repo = Repo::new(&[("a.txt", "a\n")]);

    assert_eq!(move_project_dir(&repo.host, &repo.path, "here", &none_alive).expect("no move"), None);

    assert!(!Path::new(&format!("{}/.octoflow-ai", repo.path)).exists());
}

#[test]
fn a_live_run_under_the_old_directory_refuses_the_move() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    put(&format!("{cwd}/.orchestrator/runs/{RUN}/state.json"), &live_run(cwd));

    let e = move_project_dir(&repo.host, cwd, "here", &none_alive).expect_err("refused");

    let start = format!("{cwd}/.orchestrator holds run {RUN}, which an older engine drives (pid 4242 on elsewhere");
    assert!(e.0.starts_with(&start), "{}", e.0);
    assert!(e.0.ends_with("let that run end, or stop it, then start again"), "{}", e.0);
    assert!(Path::new(&format!("{cwd}/.orchestrator/runs/{RUN}/state.json")).exists());
    assert!(!Path::new(&format!("{cwd}/.octoflow-ai")).exists());
}
