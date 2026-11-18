//! forget, through main's command on the CLI fixture, whose FakeHost stands for the project's machine, local or
//! --machine's alike. That the real host deletes the directory and keeps the metrics is Host::forget_run's, in
//! engine/src/host/tests.rs.

use serde_json::{json, Value};

use crate::cli::fixture::Cli;
use crate::cli::Target;
use crate::constants::{BRANCH_PREFIX, EXIT_ERROR, RUNS_DIR};
use crate::json::object;
use crate::state::RunState;

const CLOSED: &str = "20261001-120000-a1b2c3";
const FINISHED: &str = "20261001-120100-d4e5f6";
const RUNNING: &str = "20261001-120200-0a0b0c";

/// The CLI fixture with no live pid.
fn cli() -> Cli {
    let mut cli = Cli::new();
    cli.f.pid_alive = Box::new(|_| false);
    cli
}

/// Save a run of /proj with id, closed when closed says how, with a metrics.json when done.
fn add(cli: &Cli, id: &str, closed: Option<&str>, done: bool) {
    add_with(cli, id, closed, done, |_| {});
}

/// add, with the state changed by edit before it is saved.
fn add_with(cli: &Cli, id: &str, closed: Option<&str>, done: bool, edit: impl FnOnce(&mut RunState)) {
    let mut state = RunState::new(id, &format!("task of {id}"), "/proj");
    state.phase = if done { "done".into() } else { "build".into() };
    state.verdict = done.then(|| "APPROVE".into());
    state.closed = closed.map(String::from);
    edit(&mut state);
    let mut host = cli.f.host.state();
    host.files.insert(format!("{}/state.json", dir(id)), format!("{}\n", state.to_json()));
    if done {
        host.files.insert(format!("{}/metrics.json", dir(id)), format!("{}\n", json!({"run_id": id})));
    }
}

fn dir(id: &str) -> String {
    format!("/proj/{RUNS_DIR}/{id}")
}

/// Whether anything of run id's directory is left.
fn kept(cli: &Cli, id: &str) -> bool {
    let under = format!("{}/", dir(id));
    cli.f.host.state().files.keys().any(|path| path.starts_with(&under))
}

/// main's command argv in /proj: its exit status and standard output.
fn run(cli: &Cli, argv: &[&str]) -> (i32, String) {
    let mut full = vec![argv[0], "--cwd", "/proj"];
    full.extend(&argv[1..]);
    let status = cli.main(&full);
    (status, cli.take_out())
}

/// The runs list --json gives, by id.
fn listed(cli: &Cli) -> Vec<String> {
    let (_, out) = run(cli, &["list", "--json"]);
    let document: Value = serde_json::from_str(&out).unwrap();
    document["runs"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_sweep_forgets_only_the_closed_run_and_metrics_stay_the_same() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    add(&cli, FINISHED, None, true);
    add(&cli, RUNNING, None, false);
    let metrics = run(&cli, &["metrics"]);

    let (status, out) = run(&cli, &["forget", "--all", "--dry-run"]);
    assert_eq!(status, 0);
    assert_eq!(
        out,
        format!("would forget run {CLOSED}: delete {}\n1 to forget, 2 not closed, 0 skipped\n", dir(CLOSED))
    );
    assert!(kept(&cli, CLOSED));

    let (status, out) = run(&cli, &["forget", "--all"]);
    assert_eq!(status, 0);
    assert_eq!(out, format!("forgot run {CLOSED}: deleted {}\n1 forgotten, 2 not closed, 0 skipped\n", dir(CLOSED)));
    assert!(!kept(&cli, CLOSED));
    assert!(kept(&cli, FINISHED) && kept(&cli, RUNNING));
    assert_eq!(listed(&cli), [FINISHED, RUNNING]);
    assert_eq!(run(&cli, &["metrics"]), metrics);
}

#[test]
fn a_run_that_is_not_closed_is_refused_and_a_closed_one_forgotten() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), false);
    add(&cli, FINISHED, None, true);
    let e = cli.error(&["forget", "--cwd", "/proj", "d4e5f6"]);
    assert_eq!(e, format!("run {FINISHED} is not closed: close it before forgetting it"));
    assert_eq!(cli.main(&["forget", "--cwd", "/proj", "d4e5f6"]), EXIT_ERROR);
    assert!(kept(&cli, FINISHED));
    cli.take_out();

    let (status, out) = run(&cli, &["forget", "a1b2c3", "--dry-run"]);
    assert_eq!((status, out), (0, format!("would forget run {CLOSED}: delete {}\n", dir(CLOSED))));
    assert!(kept(&cli, CLOSED));
    let (status, out) = run(&cli, &["forget", CLOSED]);
    assert_eq!((status, out), (0, format!("forgot run {CLOSED}: deleted {}\n", dir(CLOSED))));
    assert!(!kept(&cli, CLOSED));
}

#[test]
fn a_directory_that_cannot_be_deleted_fails_its_run_and_not_the_others() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    add(&cli, FINISHED, Some("workspace closed"), true);
    cli.f.host.state().undeletable_run_dirs.insert(dir(CLOSED));
    let (status, out) = run(&cli, &["forget", "--all"]);
    assert_eq!(status, EXIT_ERROR);
    assert_eq!(
        out,
        format!(
            "could not forget run {CLOSED}: could not delete {}: rm: Permission denied\n\
             forgot run {FINISHED}: deleted {}\n1 forgotten, 0 not closed, 0 skipped, 1 failed\n",
            dir(CLOSED),
            dir(FINISHED)
        )
    );
    assert!(kept(&cli, CLOSED) && !kept(&cli, FINISHED));

    // Its metrics went to the archive before the delete failed; forgetting it again does not add them twice.
    cli.f.host.state().undeletable_run_dirs.clear();
    let metrics = run(&cli, &["metrics"]);
    assert_eq!(run(&cli, &["forget", CLOSED]).0, 0);
    assert_eq!(run(&cli, &["metrics"]), metrics);
    assert_eq!(metrics.1.lines().count(), 2);
}

#[test]
fn a_run_whose_state_cannot_be_read_is_skipped_and_named() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), false);
    cli.f
        .host
        .state()
        .files
        .insert(format!("{}/state.json", dir(FINISHED)), format!("{}\n", json!({"run_id": FINISHED, "format": 999})));
    let (status, out) = run(&cli, &["forget", "--all"]);
    assert_eq!(status, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with(&format!("forgot run {CLOSED}")), "{out}");
    assert!(lines[1].starts_with("skipped: ") && lines[1].contains(FINISHED), "{out}");
    assert_eq!(lines[2], "1 forgotten, 0 not closed, 1 skipped");
    assert!(kept(&cli, FINISHED));
}

/// A finished --worktree run whose pull request close left open: its branch and worktree wait for prune.
fn awaiting_prune(state: &mut RunState) {
    state.branch = Some(format!("{BRANCH_PREFIX}add-a-rate-limiter-a1b2c3"));
    state.pr_url = Some("https://github.com/o/r/pull/7".into());
}

#[test]
fn a_closed_run_whose_branch_waits_for_prune_is_kept_until_prune_records_it() {
    let cli = cli();
    add_with(&cli, CLOSED, Some("workspace closed"), true, awaiting_prune);
    let e = cli.error(&["forget", "--cwd", "/proj", CLOSED]);
    assert_eq!(
        e,
        format!(
            "run {CLOSED} is closed, but prune deletes its branch and worktree once its pull request is merged or \
             closed: forget it after that"
        )
    );
    let (status, out) = run(&cli, &["forget", "--all"]);
    assert_eq!((status, out.as_str()), (0, "0 forgotten, 0 not closed, 1 waiting for prune, 0 skipped\n"));
    assert!(kept(&cli, CLOSED));

    // Once prune has recorded what it did with the branch, deleted or kept, the run may go.
    for cleanup in ["deleted", "kept: the pull request is open"] {
        add_with(&cli, CLOSED, Some("workspace closed"), true, |s| {
            awaiting_prune(s);
            s.branch_cleanup = Some(cleanup.into());
        });
        assert_eq!(run(&cli, &["forget", CLOSED]).0, 0, "{cleanup}");
        assert!(!kept(&cli, CLOSED), "{cleanup}");
    }
}

#[test]
fn a_live_run_is_refused_and_left() {
    let mut cli = cli();
    cli.f.pid_alive = Box::new(|pid| pid == 4242);
    add_with(&cli, RUNNING, None, false, |s| {
        s.owner = Some(object([
            ("host", Value::from("here")),
            ("pid", Value::from(4242)),
            ("started_at", Value::from("2026-10-01T12:02:00+00:00")),
        ]));
    });
    assert!(run(&cli, &["list"]).1.contains("running: pid 4242 on here"));
    let e = cli.error(&["forget", "--cwd", "/proj", RUNNING]);
    assert_eq!(e, format!("run {RUNNING} is not closed: close it before forgetting it"));
    assert_eq!(run(&cli, &["forget", "--all"]), (0, "0 forgotten, 1 not closed, 0 skipped\n".to_string()));
    assert!(kept(&cli, RUNNING));
}

#[test]
fn on_a_machine() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    let (status, out) = run(&cli, &["forget", "--all", "--machine", "m"]);
    assert_eq!(
        (status, out),
        (0, format!("forgot run {CLOSED}: deleted {}\n1 forgotten, 0 not closed, 0 skipped\n", dir(CLOSED)))
    );
    assert!(!kept(&cli, CLOSED));
    let target = Target { machine: Some("m".into()), cwd: Some("/proj".into()) };
    assert_eq!(cli.command(&["forget", "--cwd", "/proj", CLOSED, "--machine", "m"]).target(), Some(&target));
}

#[test]
fn a_listed_run_is_read_at_its_turn_so_one_forgotten_since_is_gone() {
    use crate::cli::forget::{forget_all, Forgetter, Outcome, RunForgotten, Sweep};
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    // The second CLOSED is read after the first forgot it, as a run another request forgot meanwhile would be.
    let ids = [CLOSED, CLOSED, "20261001-130000-ffffff"].map(String::from);
    let runs = cli.with(|world| forget_all(&Forgetter::of(world), "/proj", Sweep::Listed(&ids))).unwrap();
    let outcome = |id: &str, outcome| RunForgotten { run_id: id.to_string(), outcome };
    assert_eq!(
        runs,
        [outcome(CLOSED, Outcome::Forgotten), outcome(CLOSED, Outcome::Gone), outcome(&ids[2], Outcome::Gone)]
    );
    assert_eq!(
        cli.take_out(),
        format!("forgot run {CLOSED}: deleted {}\n1 forgotten, 0 not closed, 2 skipped\n", dir(CLOSED))
    );
}
