//! Reaching origin through a short outage of it: which of git's failures say origin could not be reached, how long a
//! run waits for it to answer again, and what the human is told meanwhile and at the end.

use super::{Clocks, Human};
use crate::constants::REMOTE;
use crate::error::OrchestratorError;
use crate::interrupt::Interrupt;

/// Seconds waited before each try after the first; one try more than there are waits. About a minute and a half in
/// all: a blip of the network or of the forge, as GitHub's refusing known keys during its incident of 2026-11-04,
/// passes in that, and a longer outage is left to the human, who resumes the run.
const WAITS: [u64; 3] = [5, 20, 60];

/// What git writes, over SSH or HTTPS, when it could not reach origin or origin would not let it in: each a failure
/// that a later try may not have. A failure without any of them, such as a branch origin lacks, is final, and so is
/// one with any of FINAL too. A command the host kills for running past its time limit, as one that hangs on a network
/// that drops its packets, says none of them either: four tries of NETWORK_TIMEOUT each would hold a run for 20
/// minutes, so a hang is final.
const UNREACHABLE: &[&str] = &[
    // ssh's every failure, a refused key included, ends so.
    "Could not read from remote repository",
    // curl's every failure: DNS, connect, TLS, an HTTP error status.
    "unable to access",
    "Authentication failed",
    "Connection timed out",
    "Connection reset",
    "Connection refused",
    "the remote end hung up unexpectedly",
    "early EOF",
    "RPC failed",
];

/// What git writes, beside one of UNREACHABLE's, when origin answered and the answer will not change by asking again:
/// ssh ends each of these with "Could not read from remote repository", as HTTPS's do with "unable to access".
const FINAL: &[&str] = &[
    "does not appear to be a git repository",
    "Repository not found",
    // GitHub's over SSH: "ERROR: Permission to o/r.git denied to user".
    "denied to",
    "The requested URL returned error: 403",
    "The requested URL returned error: 404",
];

/// reach's result, reach being a call that reaches origin (a fetch, an `ls-remote`), tried again while its error says
/// origin could not be reached: up to WAITS.len() more times, each after a wait on clocks that human is told of
/// first, as `origin unreachable, trying again in 20 s (2 of 3)`.
///
/// Errors: reach's error as it is, at once, when it says something else (origin_unreachable), or when interrupt is
/// raised by the time a try fails or during the wait after it; after the last try, its error followed by a line saying
/// how many tries were made and what to check, naming GitHub's checks when the error names github.com, which
/// origin_unreachable still says of it.
pub fn reaching_origin<T>(
    clocks: &dyn Clocks,
    human: &dyn Human,
    interrupt: &Interrupt,
    mut reach: impl FnMut() -> Result<T, OrchestratorError>,
) -> Result<T, OrchestratorError> {
    let mut waits = WAITS.iter().enumerate();
    loop {
        let e = match reach() {
            Ok(reached) => return Ok(reached),
            Err(e) if !origin_unreachable(&e) || interrupt.is_raised() => return Err(e),
            Err(e) => e,
        };
        let Some((i, wait)) = waits.next() else {
            return Err(OrchestratorError(format!("{e}\n({})", gave_up(&e))));
        };
        human.log(&format!("{REMOTE} unreachable, trying again in {wait} s ({} of {})", i + 1, WAITS.len()));
        clocks.sleep(*wait as f64);
        if interrupt.is_raised() {
            return Err(e);
        }
    }
}

/// Whether e, an error of a call that reaches origin, says origin could not be reached for now: what reaching_origin
/// tries again on, and what its last error still says after every try.
pub fn origin_unreachable(e: &OrchestratorError) -> bool {
    UNREACHABLE.iter().any(|said| e.0.contains(said)) && !FINAL.iter().any(|said| e.0.contains(said))
}

/// The line reaching_origin's last error ends with, for e, the last try's error: the tries made, and what to check,
/// as far as e tells where origin is.
fn gave_up(e: &OrchestratorError) -> String {
    let mut checks = vec![];
    if e.0.contains("git@github.com") {
        checks.push("`ssh -T git@github.com`");
    }
    if e.0.contains("github.com") {
        checks.push("https://www.githubstatus.com");
    }
    let tries = WAITS.len() + 1;
    match checks.as_slice() {
        [] => format!("{REMOTE} still unreachable after {tries} tries; check that it can be reached"),
        _ => {
            let checks = checks.join(", and ");
            format!("{REMOTE} still unreachable after {tries} tries; check that it can be reached: {checks}")
        }
    }
}

#[cfg(test)]
mod tests;
