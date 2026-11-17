//! The move of a project's own directory off its old name, in a real git repository with a real worktree of a run
//! under it.

use std::path::Path;

use super::*;
use crate::host::tests::{put, Repo};
use crate::state::RunState;

/// A run id as new_run_id makes them, with the key k1a2b3.
const RUN: &str = "20260101-120000-k1a2b3";

/// A run of the project at cwd that failed, so nothing drives it, closed when closed is, saved as state.json text.
fn failed_run(cwd: &str, closed: bool) -> String {
    let mut state = RunState::new(RUN, "a task", cwd);
    state.error = Some("it failed".to_string());
    state.closed = closed.then(|| "workspace closed".to_string());
    state.to_text()
}

/// The state.json of RUN under the project at cwd's old directory.
fn old_state(cwd: &str) -> String {
    format!("{cwd}/.orchestrator/runs/{RUN}/state.json")
}

#[test]
fn a_project_s_old_directory_of_closed_runs_moves_with_its_kept_worktree_registered_at_the_new_place() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    let old_tree = format!("{cwd}/.orchestrator/worktrees/k1a2b3");
    let new_tree = format!("{cwd}/.octoflow-ai/worktrees/k1a2b3");
    repo.git(&["worktree", "add", "--quiet", "-b", "run-k1a2b3", &old_tree]);
    put(&old_state(cwd), &failed_run(cwd, true));

    let moved = move_project_dir(&repo.host, cwd, false).expect("the move");

    assert_eq!(moved, Some(format!("moved the project's runs from {cwd}/.orchestrator to {cwd}/.octoflow-ai")));
    assert!(!Path::new(&format!("{cwd}/.orchestrator")).exists());
    assert!(Path::new(&format!("{cwd}/.octoflow-ai/runs/{RUN}/state.json")).exists());
    // Git finds the worktree at its new place from both ends.
    assert!(repo.git(&["worktree", "list", "--porcelain"]).contains(&format!("worktree {new_tree}\n")));
    let head = repo.host.git(&new_tree, &["rev-parse", "--abbrev-ref", "HEAD"]).expect("git in the worktree");
    assert_eq!(head, "run-k1a2b3\n");
}

#[test]
fn a_worktree_git_cannot_register_again_is_named_and_the_move_stands() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    put(&old_state(cwd), &failed_run(cwd, true));
    put(&format!("{cwd}/.orchestrator/worktrees/k1a2b3/.git"), "gitdir: /nowhere\n");

    let moved = move_project_dir(&repo.host, cwd, false).expect("the move");

    let tree = format!("{cwd}/.octoflow-ai/worktrees/k1a2b3");
    let said = format!(
        "moved the project's runs from {cwd}/.orchestrator to {cwd}/.octoflow-ai; git could not register the \
         worktree {tree} again: run `git worktree repair {tree}` in {cwd}"
    );
    assert_eq!(moved, Some(said));
    assert!(Path::new(&format!("{cwd}/.octoflow-ai/runs/{RUN}/state.json")).exists());
}

#[test]
fn a_run_not_closed_under_the_old_directory_refuses_the_move() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    put(&old_state(cwd), &failed_run(cwd, false));

    let e = move_project_dir(&repo.host, cwd, false).expect_err("refused");

    let start = format!("{cwd}/.orchestrator holds runs an older engine left open: {RUN}; this engine moves it to");
    assert!(e.0.starts_with(&start), "{}", e.0);
    assert!(Path::new(&old_state(cwd)).exists());
    assert!(!Path::new(&format!("{cwd}/.octoflow-ai")).exists());
}

#[test]
fn a_dry_run_refuses_a_due_move_and_moves_nothing() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    put(&old_state(cwd), &failed_run(cwd, true));

    let e = move_project_dir(&repo.host, cwd, true).expect_err("refused");

    let said = format!(
        "{cwd}/.orchestrator holds this project's runs, which the first command that is not a dry run moves to \
         {cwd}/.octoflow-ai"
    );
    assert_eq!(e.0, said);
    assert!(Path::new(&old_state(cwd)).exists());
    assert!(!Path::new(&format!("{cwd}/.octoflow-ai")).exists());
}

#[test]
fn a_project_whose_new_directory_exists_keeps_both() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    put(&old_state(cwd), &failed_run(cwd, true));
    put(&format!("{cwd}/.octoflow-ai/.gitignore"), "*\n");

    assert_eq!(move_project_dir(&repo.host, cwd, false).expect("no move"), None);

    assert!(Path::new(&old_state(cwd)).exists());
    assert!(!Path::new(&format!("{cwd}/.octoflow-ai/runs")).exists());
}

#[test]
fn an_old_directory_without_runs_is_another_tool_s_and_stays() {
    let repo = Repo::new(&[("a.txt", "a\n")]);
    let cwd = &repo.path;
    put(&format!("{cwd}/.orchestrator/config.toml"), "x = 1\n");

    assert_eq!(move_project_dir(&repo.host, cwd, false).expect("no move"), None);

    assert!(Path::new(&format!("{cwd}/.orchestrator/config.toml")).exists());
    assert!(!Path::new(&format!("{cwd}/.octoflow-ai")).exists());
}

#[test]
fn a_project_without_an_old_directory_has_nothing_to_move() {
    let repo = Repo::new(&[("a.txt", "a\n")]);

    assert_eq!(move_project_dir(&repo.host, &repo.path, false).expect("no move"), None);

    assert!(!Path::new(&format!("{}/.octoflow-ai", repo.path)).exists());
}
