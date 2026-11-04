//! reaching_origin's tries, waits and errors, on a FakeClock and the human as Told.

use std::cell::Cell;

use super::*;
use crate::host::fake::FakeClock;
use crate::workflow::fixture::Told;

/// git's error at a fetch GitHub refused the key of, as during its outage of 2026-11-04.
const REFUSED: &str = "could not fetch main from origin: git -C /proj fetch --quiet origin main failed: \
                       git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.";

/// A reach that fails with error its first failures times and then gives "done", counting its tries in tries.
fn failing<'t>(
    error: &'t str,
    failures: u32,
    tries: &'t Cell<u32>,
) -> impl FnMut() -> Result<&'static str, OrchestratorError> + 't {
    move || {
        tries.set(tries.get() + 1);
        if tries.get() <= failures {
            Err(OrchestratorError(error.to_string()))
        } else {
            Ok("done")
        }
    }
}

#[test]
fn an_origin_that_answers_again_is_waited_for() {
    let (clock, told, interrupt, tries) = (FakeClock::default(), Told::default(), Interrupt::new(), Cell::new(0));
    assert_eq!(reaching_origin(&clock, &told, &interrupt, failing(REFUSED, 2, &tries)).unwrap(), "done");
    assert_eq!(tries.get(), 3);
    assert_eq!(clock.now(), 25.0);
    assert_eq!(
        told.lines(),
        ["origin unreachable, trying again in 5 s (1 of 3)", "origin unreachable, trying again in 20 s (2 of 3)"]
    );
}

#[test]
fn an_origin_that_never_answers_fails_after_every_try_saying_what_to_check() {
    let (clock, told, interrupt, tries) = (FakeClock::default(), Told::default(), Interrupt::new(), Cell::new(0));
    let e = reaching_origin(&clock, &told, &interrupt, failing(REFUSED, u32::MAX, &tries)).unwrap_err();
    assert_eq!(tries.get(), 4);
    assert_eq!(clock.now(), 85.0);
    assert_eq!(told.lines().len(), 3);
    assert_eq!(
        e.0,
        format!(
            "{REFUSED}\n(tried 4 times; check that origin can be reached: `ssh -T git@github.com`, and \
             https://www.githubstatus.com)"
        )
    );
}

#[test]
fn an_origin_elsewhere_gets_no_github_hint() {
    let (clock, told, interrupt, tries) = (FakeClock::default(), Told::default(), Interrupt::new(), Cell::new(0));
    let error = "fatal: unable to access 'https://git.example/r/': Could not resolve host: git.example";
    let e = reaching_origin(&clock, &told, &interrupt, failing(error, u32::MAX, &tries)).unwrap_err();
    assert_eq!(e.0, format!("{error}\n(tried 4 times; check that origin can be reached)"));
}

#[test]
fn a_failure_a_retry_cannot_change_is_tried_once() {
    let (clock, told, interrupt, tries) = (FakeClock::default(), Told::default(), Interrupt::new(), Cell::new(0));
    let missing = "could not fetch main from origin: git -C /proj fetch --quiet origin main failed: fatal: couldn't \
                   find remote ref main";
    let e = reaching_origin(&clock, &told, &interrupt, failing(missing, u32::MAX, &tries)).unwrap_err();
    assert_eq!((tries.get(), e.0.as_str()), (1, missing));
    assert_eq!(clock.now(), 0.0);
    assert!(told.lines().is_empty());
}

#[test]
fn each_way_git_says_it_could_not_reach_origin_is_retried() {
    for error in [
        "fatal: Could not read from remote repository.",
        "fatal: unable to access 'https://github.com/o/r/': Failed to connect to github.com port 443",
        "ssh: connect to host github.com port 22: Connection timed out",
        "kex_exchange_identification: read: Connection reset by peer",
        "fatal: the remote end hung up unexpectedly",
        "fatal: early EOF",
        "error: RPC failed; HTTP 502 curl 22 The requested URL returned error: 502",
        "fatal: Authentication failed for 'https://github.com/o/r/'",
    ] {
        let (clock, told, interrupt, tries) = (FakeClock::default(), Told::default(), Interrupt::new(), Cell::new(0));
        reaching_origin(&clock, &told, &interrupt, failing(error, 1, &tries)).unwrap();
        assert_eq!(tries.get(), 2, "{error}");
    }
}

#[test]
fn a_ctrl_c_during_a_wait_ends_the_tries_with_the_last_error() {
    let (clock, told, interrupt, tries) = (FakeClock::default(), Told::default(), Interrupt::new(), Cell::new(0));
    let raised = interrupt.clone();
    clock.hook(move |_| raised.raise());
    let e = reaching_origin(&clock, &told, &interrupt, failing(REFUSED, u32::MAX, &tries)).unwrap_err();
    assert_eq!((tries.get(), e.0.as_str()), (1, REFUSED));
    assert_eq!(told.lines().len(), 1);
}

#[test]
fn a_ctrl_c_before_the_first_try_waits_for_nothing() {
    let (clock, told, interrupt, tries) = (FakeClock::default(), Told::default(), Interrupt::new(), Cell::new(0));
    interrupt.raise();
    let e = reaching_origin(&clock, &told, &interrupt, failing(REFUSED, u32::MAX, &tries)).unwrap_err();
    assert_eq!((tries.get(), e.0.as_str(), clock.now()), (1, REFUSED, 0.0));
    assert!(told.lines().is_empty());
}
