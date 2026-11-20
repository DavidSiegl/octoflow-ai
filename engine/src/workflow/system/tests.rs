//! The real run's clocks and human: SystemClocks and LocalHuman.

use crate::herdr::ProcessHerdr;
use std::time::{Duration, Instant};

use super::*;
use crate::host::tests::{environment, tool};
use crate::test_support::TempDir;

#[test]
fn the_clocks_are_the_systems() {
    let interrupt = Interrupt::new();
    let clocks = SystemClocks::new(&interrupt).unwrap();
    let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
    assert!((clocks.wall() - wall).abs() < 5.0);
    let now = clock_gettime(ClockId::CLOCK_MONOTONIC).unwrap();
    assert!((clocks.monotonic() - now.tv_sec() as f64).abs() < 5.0);
    let (start, before) = (Instant::now(), clocks.monotonic());
    clocks.sleep(0.05);
    assert!(start.elapsed() >= Duration::from_millis(50));
    assert!(clocks.monotonic() - before >= 0.05);
    // The Ctrl-C cuts a sleep short, a sleep of no time is none, and neither is one of a time that is not one.
    interrupt.raise();
    let start = Instant::now();
    clocks.sleep(600.0);
    interrupt.catch();
    clocks.sleep(-1.0);
    clocks.sleep(f64::NAN);
    assert!(start.elapsed() < Duration::from_secs(60));
}

#[test]
fn the_human_is_notified_through_the_local_herdr() {
    let d = TempDir::new();
    let args = d.join("args");
    tool(&d.path(), "herdr", &format!("#!/bin/sh\necho \"$@\" > '{args}'\necho '{{\"result\": {{}}}}'\n"));
    let herdr = ProcessHerdr::in_environment(None, environment(Some(&d.path()), &[]).into_iter(), &Interrupt::new());
    let human = LocalHuman::new(Box::new(herdr));
    human.notify("Run finished: APPROVE", "the task");
    assert_eq!(
        std::fs::read_to_string(&args).unwrap(),
        "notification show Run finished: APPROVE --body the task --sound request\n"
    );
    // A notification that fails costs only itself.
    tool(&d.path(), "herdr", "#!/bin/sh\nexit 3\n");
    human.notify("t", "b");
    human.log("a line");
    assert!(format!("{:?}", LocalHuman::new(Box::new(ProcessHerdr::new(None, &Interrupt::new()))))
        .contains("machine: None"));
}

#[test]
fn the_desktop_command_is_osascript_on_macos_and_notify_send_elsewhere_given_title_and_body_as_arguments() {
    let mac = desktop_command("macos", "Builder \"is\" waiting", "pane: it");
    let linux = desktop_command("linux", "-x", "b");

    assert_eq!(mac[0], "osascript");
    assert_eq!(mac[mac.len() - 2..], ["Builder \"is\" waiting".to_string(), "pane: it".to_string()]);
    assert!(mac.iter().any(|a| a.contains("item 1 of argv")), "{mac:?}");
    assert_eq!(linux, ["notify-send", "--app-name", "octoflow", "--", "-x", "b"]);
}

#[test]
fn a_run_over_acp_notifies_by_the_desktop_s_command_and_a_missing_one_is_an_error_once() {
    let d = TempDir::new();
    let args = d.join("args");
    for name in ["notify-send", "osascript"] {
        tool(&d.path(), name, &format!("#!/bin/sh\necho \"$@\" > '{args}'\n"));
    }
    let shown = DesktopNotify::in_environment([("PATH".into(), d.path().into())].into_iter());
    shown.notify("Builder is waiting for you", "the task").expect("shown");
    let expected = desktop_command(std::env::consts::OS, "Builder is waiting for you", "the task");
    assert_eq!(std::fs::read_to_string(&args).unwrap(), expected[1..].join(" ") + "\n");

    let empty = TempDir::new();
    let missing = DesktopNotify::in_environment([("PATH".into(), empty.path().into())].into_iter());
    let first = missing.notify("t", "b").expect_err("the command is missing");
    assert!(first.0.contains("is not installed"), "{first}");
    assert_eq!((missing.notify("t", "b"), missing.notify("t", "b")), (Ok(()), Ok(())));

    tool(&d.path(), "notify-send", "#!/bin/sh\necho no display >&2\nexit 1\n");
    tool(&d.path(), "osascript", "#!/bin/sh\necho no display >&2\nexit 1\n");
    let failed = shown.notify("t", "b").expect_err("the command fails");
    assert!(failed.0.ends_with("no display"), "{failed}");
}
