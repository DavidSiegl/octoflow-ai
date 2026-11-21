//! RUN's candidates from a project's saved runs, and the scan of the line's --cwd and --machine.

use super::*;
use crate::host::tests::put;
use crate::state::{run_dir, RunState};
use crate::test_support::TempDir;

/// A host on this machine whose home directory is home.
fn host(home: &str) -> ProcessHost {
    let vars = std::env::vars_os().filter(|(k, _)| k != "HOME").chain([("HOME".into(), home.into())]);
    ProcessHost::in_environment(vars, &Interrupt::new())
}

/// Save a run of the project at cwd, by run_id, in phase with task.
fn save(cwd: &str, run_id: &str, phase: &str, task: &str) {
    let mut state = RunState::new(run_id, task, cwd);
    state.phase = phase.to_string();
    put(&format!("{}/state.json", run_dir(cwd, run_id)), &state.to_text());
}

/// Each candidate as its value and its help.
fn shown(candidates: &[CompletionCandidate]) -> Vec<(String, String)> {
    candidates
        .iter()
        .map(|c| {
            let help = c.get_help().map(ToString::to_string).unwrap_or_default();
            (c.get_value().to_string_lossy().into_owned(), help)
        })
        .collect()
}

fn words(line: &[&str]) -> Vec<OsString> {
    line.iter().map(OsString::from).collect()
}

#[test]
fn a_project_s_runs_are_offered_by_key_newest_first_with_phase_and_task() {
    let tmp = TempDir::new();
    let cwd = tmp.join("proj");
    save(&cwd, "20261101-090000-aaaaaa", "done", "first task\nmore of it");
    save(&cwd, "20261102-090000-bbbbbb", "build", "second task");
    put(&format!("{}/state.json", run_dir(&cwd, "20261103-090000-cccccc")), "not json");

    let offered = shown(&run_candidates(&host(&tmp.path()), &cwd));

    let expected = [("cccccc", "unreadable: "), ("bbbbbb", "build: second task"), ("aaaaaa", "done: first task")];
    assert_eq!(offered, expected.map(|(v, h)| (v.to_string(), h.to_string())));
}

#[test]
fn runs_that_share_a_key_are_offered_by_their_full_ids() {
    let tmp = TempDir::new();
    let cwd = tmp.join("proj");
    save(&cwd, "20261101-090000-aaaaaa", "done", "one");
    save(&cwd, "20261102-090000-aaaaaa", "done", "two");
    save(&cwd, "20261103-090000-bbbbbb", "build", "three");

    let offered: Vec<String> = shown(&run_candidates(&host(&tmp.path()), &cwd)).into_iter().map(|(v, _)| v).collect();

    assert_eq!(offered, ["bbbbbb", "20261102-090000-aaaaaa", "20261101-090000-aaaaaa"]);
}

#[test]
fn a_run_whose_directory_was_copied_is_offered_once() {
    let tmp = TempDir::new();
    let cwd = tmp.join("proj");
    save(&cwd, "20261101-090000-aaaaaa", "done", "one");
    let saved = std::fs::read_to_string(format!("{}/state.json", run_dir(&cwd, "20261101-090000-aaaaaa"))).unwrap();
    put(&format!("{}/state.json", run_dir(&cwd, "20261101-090000-copy00")), &saved);

    let offered: Vec<String> = shown(&run_candidates(&host(&tmp.path()), &cwd)).into_iter().map(|(v, _)| v).collect();

    assert_eq!(offered, ["20261101-090000-aaaaaa"]);
}

#[test]
fn a_project_without_runs_or_a_directory_offers_none() {
    let tmp = TempDir::new();
    std::fs::create_dir_all(tmp.join("proj")).expect("the project");

    assert!(run_candidates(&host(&tmp.path()), &tmp.join("proj")).is_empty());
    assert!(run_candidates(&host(&tmp.path()), &tmp.join("nowhere")).is_empty());
}

#[test]
fn a_cwd_under_home_is_read_with_its_tilde_expanded() {
    let tmp = TempDir::new();
    save(&tmp.join("proj"), "20261101-090000-aaaaaa", "build", "t");

    let offered = shown(&run_candidates(&host(&tmp.path()), "~/proj"));

    assert_eq!(offered, [("aaaaaa".to_string(), "build: t".to_string())]);
}

/// The target of the line words, the last one the word being completed, as parser's tree parses it.
fn scanned(line: &[&str]) -> LineTarget {
    LineTarget::scan(&parser(&ConfigDir::at("/nowhere")), &words(line), None)
}

#[test]
fn the_words_after_the_cursor_count_and_only_the_one_being_completed_is_left_out() {
    let at = |cursor, line: &[&str]| LineTarget::scan(&parser(&ConfigDir::at("/nowhere")), &words(line), Some(cursor));

    let before_cwd = at(2, &["octoflow", "resume", "", "--cwd", "/a/dir"]);
    let before_machine = at(2, &["octoflow", "resume", "", "--machine", "box"]);
    let split_after = at(2, &["octoflow", "resume", "ab", "--cwd", "=", "/a/dir"]);

    assert_eq!(before_cwd, LineTarget { cwd: Some("/a/dir".to_string()), machine: false });
    assert!(before_machine.machine);
    assert_eq!(split_after.cwd, Some("/a/dir".to_string()));
}

#[test]
fn the_line_s_cwd_is_read_in_each_form_a_shell_passes_and_the_last_one_wins() {
    let cwd = |line: &[&str]| scanned(line).cwd;

    assert_eq!(cwd(&["octoflow", "resume", "--cwd", "~/dir", ""]), Some("~/dir".to_string()));
    assert_eq!(cwd(&["octoflow", "resume", "--cwd=/a/dir", ""]), Some("/a/dir".to_string()));
    assert_eq!(cwd(&["octoflow", "resume", "--cwd", "=", "/a/dir", ""]), Some("/a/dir".to_string()));
    assert_eq!(cwd(&["octoflow", "resume", "--cwd", "/a", "--cwd=/b", ""]), Some("/b".to_string()));
    assert_eq!(cwd(&["octoflow", "show", "--cw", "/a/dir", "ab"]), Some("/a/dir".to_string()));
    assert_eq!(cwd(&["octoflow", "resume", "--cw", "=", "/a/dir", ""]), Some("/a/dir".to_string()));
    assert_eq!(cwd(&["octoflow", "resume", ""]), None);
    assert_eq!(cwd(&["octoflow", "resume", "--cwd", "/being-typed"]), None);
}

#[test]
fn a_line_with_machine_is_offered_no_runs() {
    let tmp = TempDir::new();
    let cwd = tmp.join("proj");
    save(&cwd, "20261101-090000-aaaaaa", "build", "t");
    let target = scanned(&["octoflow", "resume", "--machine", "box", "--cwd", &cwd, ""]);
    assert_eq!(target, LineTarget { cwd: Some(cwd.clone()), machine: true });
    assert!(scanned(&["octoflow", "resume", "--machine=box", ""]).machine);
    assert!(scanned(&["octoflow", "close", "--mach", "box", ""]).machine);
    assert!(!scanned(&["octoflow", "resume", "--machine", "", ""]).machine);

    let command = with_run_candidates(parser(&ConfigDir::at("/nowhere")), target);
    let resume = command.find_subcommand("resume").expect("resume");
    let run_ref = resume.get_arguments().find(|a| a.get_id() == "run_ref").expect("RUN");
    let candidates = run_ref.get::<ArgValueCandidates>().expect("RUN's candidates");

    assert!(candidates.candidates().is_empty());
}

#[test]
fn every_command_with_a_run_gets_the_run_candidates() {
    let command = with_run_candidates(parser(&ConfigDir::at("/nowhere")), LineTarget { cwd: None, machine: true });
    let with_candidates: Vec<&str> = command
        .get_subcommands()
        .filter(|sub| sub.get_arguments().any(|a| a.get_id() == "run_ref" && a.get::<ArgValueCandidates>().is_some()))
        .map(clap::Command::get_name)
        .collect();

    assert_eq!(with_candidates, ["resume", "show", "close", "forget", "refresh"]);
}
