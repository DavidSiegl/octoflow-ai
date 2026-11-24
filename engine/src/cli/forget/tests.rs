//! forget, through main's command on the CLI fixture, whose FakeHost stands for the project's machine, local or
//! --machine's alike. That the real host deletes the directory and keeps the metrics is Host::forget_run's, in
//! engine/src/host/tests.rs.

use serde_json::{json, Value};

use crate::cli::fixture::Cli;
use crate::cli::Target;
use crate::constants::{BRANCH_PREFIX, EXIT_ERROR, HEARTBEAT_SECONDS, RUNS_DIR};
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
    let target = Target { machine: Some("m".into()), cwd: Some("/proj".into()), acp: false };
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

/// The project's forget lock in /proj.
fn lock() -> String {
    format!("/proj/{}", super::FORGET_LOCK)
}

/// A forget lock's text naming host and pid as its holder.
fn held_by(host: &str, pid: i64) -> String {
    format!("{}\n", json!({"host": host, "pid": pid, "nonce": "0123456789abcdef"}))
}

/// Put text at the lock's path, its heartbeat age seconds old.
fn put_lock(cli: &Cli, text: &str, age: i64) {
    let now = crate::host::Host::now(&*cli.f.host).unwrap();
    let mut host = cli.f.host.state();
    host.files.insert(lock(), text.to_string());
    host.mtimes.insert(lock(), now - age);
}

/// Every path of the fake host that names the forget lock, with its text.
fn locks(cli: &Cli) -> Vec<(String, String)> {
    let host = cli.f.host.state();
    host.files.iter().filter(|(p, _)| p.contains("forget.lock")).map(|(p, t)| (p.clone(), t.clone())).collect()
}

/// forget_all of every run in /proj from the CLI's world: what it returned, as text when an error.
fn sweep(cli: &Cli) -> Result<Vec<RunForgotten>, String> {
    cli.with(|world| forget_all(&Forgetter::of(world), "/proj", Sweep::All)).map_err(|e| e.0)
}

/// This process's id, as a forget lock it takes names it.
fn this_pid() -> i64 {
    i64::from(std::process::id())
}

use crate::cli::forget::{forget_all, Forgetter, RunForgotten, Sweep};

#[test]
fn a_forget_holds_the_lock_while_it_deletes_and_a_dry_run_takes_none() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    add(&cli, FINISHED, Some("workspace closed"), true);
    // Whether the lock was there at each write of the metrics archive, which forgetting a run makes.
    let held = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let (seen, host) = (std::rc::Rc::clone(&held), std::rc::Rc::clone(&cli.f.host));
    cli.f.host.state().after_write = Some(Box::new(move |path| {
        if path.ends_with("/metrics-archive.jsonl") {
            seen.borrow_mut().push(host.state().files.contains_key(&lock()));
        }
    }));

    assert_eq!(run(&cli, &["forget", "--all", "--dry-run"]).0, 0);
    assert!(!cli.f.host.state().writes.contains(&lock()));
    assert_eq!(run(&cli, &["forget", CLOSED]).0, 0);
    assert_eq!(run(&cli, &["forget", "--all"]).0, 0);
    assert_eq!(*held.borrow(), [true, true]);
    assert!(locks(&cli).is_empty());
}

#[test]
fn a_forget_in_a_directory_that_is_no_project_writes_nothing() {
    let cli = cli();
    assert_eq!(run(&cli, &["forget", "--all"]), (0, "0 forgotten, 0 not closed, 0 skipped\n".to_string()));
    assert!(cli.f.host.state().writes.is_empty());
}

#[test]
fn a_lock_its_holder_left_when_it_died_is_taken_over() {
    let mut cli = cli();
    cli.f.pid_alive = Box::new(|pid| pid == 4242);
    // This machine's process that is gone, its heartbeat late; and any lock whose heartbeat has been silent
    // LOCK_SILENCE, even one naming a live process of this machine, as a pid used again by another would be.
    let late = HEARTBEAT_SECONDS as i64 + 1;
    let dead = [(held_by("here", 1), late), (held_by("there", 1), super::LOCK_SILENCE)];
    let silent = [(held_by("here", 4242), super::LOCK_SILENCE), ("made by hand\n".to_string(), super::LOCK_SILENCE)];
    for (holder, age) in dead.into_iter().chain(silent) {
        add(&cli, CLOSED, Some("workspace closed"), true);
        put_lock(&cli, &holder, age);
        let (status, out) = run(&cli, &["forget", "--all"]);
        assert_eq!(
            (status, out),
            (0, format!("forgot run {CLOSED}: deleted {}\n1 forgotten, 0 not closed, 0 skipped\n", dir(CLOSED))),
            "{holder}"
        );
        assert!(locks(&cli).is_empty(), "{holder}");
    }
}

#[test]
fn a_forget_is_refused_while_a_live_holder_holds_the_lock() {
    let mut cli = cli();
    cli.f.pid_alive = Box::new(|pid| pid == 4242);
    add(&cli, CLOSED, Some("workspace closed"), true);
    // A live process of this machine; a dead one whose heartbeat is not late, as another machine of this one's name
    // may hold it; another machine's; and a lock naming none, its heartbeat younger than LOCK_SILENCE.
    let young = super::LOCK_SILENCE - 1;
    let holders = [
        (held_by("here", 4242), young, "process 4242 on here"),
        (held_by("here", 1), HEARTBEAT_SECONDS as i64, "process 1 on here"),
        (held_by("there", 1), young, "process 1 on there"),
        ("made by hand\n".to_string(), young, "a holder its text does not name"),
    ];
    for (holder, age, named) in holders {
        put_lock(&cli, &holder, age);
        assert_eq!(
            sweep(&cli),
            Err(format!(
                "another forget of this project is running: {named} holds {}; forget again once it ends",
                lock()
            ))
        );
        let e = cli.error(&["forget", "--cwd", "/proj", CLOSED]);
        assert!(e.starts_with("another forget of this project is running"), "{e}");
        assert!(kept(&cli, CLOSED), "{holder}");
        assert_eq!(locks(&cli), [(lock(), holder)]);
    }
    // A dry run changes nothing, so the lock does not hold it back.
    assert_eq!(run(&cli, &["forget", "--all", "--dry-run"]).0, 0);
}

#[test]
fn something_other_than_a_file_at_the_lock_is_an_error() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    cli.f.host.state().files.insert(format!("{}/x", lock()), String::new());
    assert_eq!(
        sweep(&cli),
        Err(format!("{} is not a file: no forget of this project can run until it is removed", lock()))
    );
    assert!(kept(&cli, CLOSED));
}

#[test]
fn two_sweeps_of_one_project_at_once_forget_a_run_once() {
    let mut cli = cli();
    cli.f.pid_alive = Box::new(|pid| pid == this_pid());
    add(&cli, CLOSED, Some("workspace closed"), true);
    let metrics = run(&cli, &["metrics"]);
    // A second sweep, as another process's, while the first appends the run's metrics to the archive.
    let second = std::rc::Rc::new(std::cell::RefCell::new(None));
    let (result, host) = (std::rc::Rc::clone(&second), std::rc::Rc::clone(&cli.f.host));
    cli.f.host.state().after_write = Some(Box::new(move |path| {
        if path.ends_with("/metrics-archive.jsonl") && result.borrow().is_none() {
            let other = Forgetter {
                host: &*host,
                interrupt: &crate::interrupt::Interrupt::new(),
                stdout: &|_| {},
                scope: crate::host::RunScope::Every,
                acp: false,
                pid_alive: &|pid| pid == this_pid(),
                local_host: "here",
            };
            *result.borrow_mut() = Some(forget_all(&other, "/proj", Sweep::All).map_err(|e| e.0));
        }
    }));
    let first = sweep(&cli).unwrap();
    assert_eq!(first, [RunForgotten { run_id: CLOSED.into(), outcome: super::Outcome::Forgotten }]);
    let refused = second.borrow_mut().take().unwrap().unwrap_err();
    assert!(refused.contains(&format!("process {} on here holds", this_pid())), "{refused}");
    cli.take_out();
    // One line of the run's metrics, as before it was forgotten.
    assert_eq!(run(&cli, &["metrics"]), metrics);
}

#[test]
fn a_sweep_whose_lock_another_took_over_stops_before_its_next_run() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    add(&cli, FINISHED, Some("workspace closed"), true);
    let taker = held_by("there", 1);
    let (text, host) = (taker.clone(), std::rc::Rc::clone(&cli.f.host));
    cli.f.host.state().after_write = Some(Box::new(move |path| {
        if path.ends_with("/metrics-archive.jsonl") {
            host.state().files.insert(lock(), text.clone());
        }
    }));
    assert_eq!(
        sweep(&cli),
        Err(format!(
            "the forget lock {} was taken over by another forget, which took this one for dead: stopped",
            lock()
        ))
    );
    assert!(!kept(&cli, CLOSED) && kept(&cli, FINISHED));
    // Its taker's lock is left to its taker.
    assert_eq!(locks(&cli), [(lock(), taker)]);
}

#[test]
fn a_sweep_renews_its_locks_heartbeat_before_each_run() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    add(&cli, FINISHED, Some("workspace closed"), true);
    // At each archive write, let the forget take LOCK_SILENCE and note the lock's age then.
    let ages = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let (seen, host) = (std::rc::Rc::clone(&ages), std::rc::Rc::clone(&cli.f.host));
    cli.f.host.state().after_write = Some(Box::new(move |path| {
        if path.ends_with("/metrics-archive.jsonl") {
            let mut s = host.state();
            let lock_mtime = s.mtimes[&lock()];
            s.mtimes.insert(lock(), lock_mtime - super::LOCK_SILENCE);
            drop(s);
            let now = crate::host::Host::now(&*host).unwrap();
            seen.borrow_mut().push(now - crate::host::Host::mtime(&*host, &lock()).unwrap());
        }
    }));
    sweep(&cli).unwrap();
    // Aged past LOCK_SILENCE after the first run, renewed before the second.
    assert_eq!(*ages.borrow(), [super::LOCK_SILENCE, super::LOCK_SILENCE]);
}

#[test]
fn a_dead_holders_lock_another_took_over_first_is_put_back() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    put_lock(&cli, &held_by("here", 4242), HEARTBEAT_SECONDS as i64 + 1);
    // Between reading the dead holder's lock and moving it aside, another forget takes it over and holds it.
    let live = held_by("there", 1);
    let (taken, host) = (live.clone(), std::rc::Rc::clone(&cli.f.host));
    cli.f.host.state().before_rename = Some(Box::new(move |path, _| {
        if path == lock() {
            host.state().files.insert(lock(), taken.clone());
        }
    }));
    let refused = sweep(&cli).unwrap_err();
    assert!(refused.contains("process 1 on there holds"), "{refused}");
    assert!(kept(&cli, CLOSED));
    assert_eq!(locks(&cli), [(lock(), live)]);
}

#[test]
fn a_lock_moved_aside_for_an_instant_is_made_again_and_the_sweep_goes_on() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    add(&cli, FINISHED, Some("workspace closed"), true);
    // While the first run is forgotten, another's take_over has the lock moved aside, to put it back after.
    let aside = std::rc::Rc::new(std::cell::RefCell::new(None));
    let (moved, host) = (std::rc::Rc::clone(&aside), std::rc::Rc::clone(&cli.f.host));
    let remade = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = std::rc::Rc::clone(&remade);
    cli.f.host.state().after_write = Some(Box::new(move |path| {
        if path.ends_with("/metrics-archive.jsonl") {
            if moved.borrow().is_none() {
                *moved.borrow_mut() = host.state().files.shift_remove(&lock());
            } else {
                seen.set(host.state().files.contains_key(&lock()));
            }
        }
    }));
    let runs = sweep(&cli).unwrap();
    assert!(runs.iter().all(|run| run.outcome == super::Outcome::Forgotten), "{runs:?}");
    assert!(!kept(&cli, CLOSED) && !kept(&cli, FINISHED));
    // The lock was made again with the same text before the second run, and released at the end.
    assert!(aside.borrow().as_deref().is_some_and(|text| text.contains(&format!("\"pid\":{}", this_pid()))));
    assert!(remade.get());
    assert!(locks(&cli).is_empty());
}

#[test]
fn a_run_another_forget_forgot_after_taking_this_ones_lock_over_is_gone_not_forgotten_twice() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    add(&cli, FINISHED, Some("workspace closed"), true);
    // While the first run is forgotten, this sweep falls silent past LOCK_SILENCE: another forget takes its lock
    // over, forgets the second run and releases the lock.
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let (once, host) = (std::rc::Rc::clone(&done), std::rc::Rc::clone(&cli.f.host));
    cli.f.host.state().after_write = Some(Box::new(move |path| {
        if path.ends_with("/metrics-archive.jsonl") && !once.replace(true) {
            let mut s = host.state();
            s.files.shift_remove(&lock());
            s.files.retain(|path, _| !path.starts_with(&format!("{}/", dir(FINISHED))));
        }
    }));
    let runs = sweep(&cli).unwrap();
    let outcome = |id: &str, outcome| RunForgotten { run_id: id.to_string(), outcome };
    assert_eq!(runs, [outcome(CLOSED, super::Outcome::Forgotten), outcome(FINISHED, super::Outcome::Gone)]);
    assert!(!cli.take_out().contains(&format!("forgot run {FINISHED}")));
    assert!(locks(&cli).is_empty());
}

#[test]
fn a_lock_read_while_another_writes_it_back_is_read_again_and_the_sweep_goes_on() {
    let cli = cli();
    add(&cli, CLOSED, Some("workspace closed"), true);
    add(&cli, FINISHED, Some("workspace closed"), true);
    // After the first run, another's take_over puts this sweep's lock back: the renew's first read finds it partly
    // written, the next finds it whole. Steps: 0 not begun, 1 partly written, 2 read so once, 3 whole again.
    let step = std::rc::Rc::new(std::cell::Cell::new(0));
    let whole = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    let (at, text, host) = (std::rc::Rc::clone(&step), std::rc::Rc::clone(&whole), std::rc::Rc::clone(&cli.f.host));
    cli.f.host.state().after_write = Some(Box::new(move |path| {
        if path.ends_with("/metrics-archive.jsonl") && at.get() == 0 {
            let mut s = host.state();
            let held = s.files[&lock()].clone();
            s.files.insert(lock(), held[..held.len() / 2].to_string());
            *text.borrow_mut() = held;
            at.set(1);
        }
    }));
    let (at, text, host) = (std::rc::Rc::clone(&step), std::rc::Rc::clone(&whole), std::rc::Rc::clone(&cli.f.host));
    cli.f.host.state().before_read = Some(Box::new(move |path| {
        if path == lock() {
            match at.get() {
                1 => at.set(2),
                2 => {
                    host.state().files.insert(lock(), text.borrow().clone());
                    at.set(3);
                }
                _ => {}
            }
        }
    }));
    let runs = sweep(&cli).unwrap();
    assert!(runs.iter().all(|run| run.outcome == super::Outcome::Forgotten), "{runs:?}");
    assert_eq!(step.get(), 3);
    assert!(locks(&cli).is_empty());
}
