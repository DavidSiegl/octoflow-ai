//! ci/release-latest.sh, which tells the Release stage whether a release may be GitHub's Latest: not when a release
//! above it is there already.

mod release_scripts;

use std::process::Output;

/// The script's run from the checkout with args, given the release tags released on standard input.
fn run(args: &[&str], released: &[&str]) -> Output {
    release_scripts::run(&xtask::checkout(), "ci/release-latest.sh", args, released)
}

/// What the script printed for version, given released; panics with its standard error when it did not exit 0.
fn printed(version: &str, released: &[&str]) -> String {
    release_scripts::printed(run(&[version], released))
}

#[test]
fn a_release_above_every_other_may_be_latest() {
    assert_eq!(printed("0.10.0", &["v0.9.0", "v0.9.1"]), "true\n");
    // No release yet.
    assert_eq!(printed("0.1.0", &[]), "true\n");
}

#[test]
fn a_release_below_one_already_there_may_not_be_latest() {
    // A failed main build of 0.104.9 re-run after 0.104.10 released: as versions, not as text, 0.104.10 is higher.
    assert_eq!(printed("0.104.9", &["v0.104.8", "v0.104.10"]), "false\n");
    assert_eq!(printed("0.9.2", &["v0.10.0", "v0.9.1"]), "false\n");
}

#[test]
fn only_a_plain_version_tag_is_a_release() {
    // Each would sort above 0.10.0 as sort -V orders them, were it taken for a release.
    assert_eq!(printed("0.10.0", &["v0.9.0", "nightly", "vnext", "v0.11.0-rc.1", "0.11.0"]), "true\n");
}

#[test]
fn a_call_without_the_version_is_a_usage_error() {
    let output = run(&[], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: release-latest.sh"));
}
