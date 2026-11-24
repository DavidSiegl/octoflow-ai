//! ci/release-notes.sh, which gives the Release stage the notes of a release: its own version's docs/releases/ file,
//! and those of the versions since the latest release that main reached without a release of their own.

use std::fs;
use std::process::Output;

/// The script's run on version and latest, from a copy of it in a tree whose docs/releases/ holds a notes file
/// `## <version>\n` for each of notes.
fn release_notes(notes: &[&str], version: &str, latest: &str) -> Output {
    let dir = tempfile::Builder::new().prefix("xtask-release-notes-test-").tempdir().expect("a temporary directory");
    fs::create_dir(dir.path().join("ci")).expect("made");
    fs::create_dir_all(dir.path().join("docs/releases")).expect("made");
    fs::copy(xtask::checkout().join("ci/release-notes.sh"), dir.path().join("ci/release-notes.sh")).expect("copied");
    for v in notes {
        fs::write(dir.path().join(format!("docs/releases/v{v}.md")), format!("## {v}\n")).expect("written");
    }
    std::process::Command::new("sh")
        .args(["ci/release-notes.sh", version, latest])
        .current_dir(dir.path())
        .output()
        .expect("sh runs")
}

/// What the script printed on standard output; panics with its standard error when it did not exit 0.
fn printed(output: Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_release_right_after_the_latest_has_its_own_notes_as_they_are() {
    let notes = ["0.9.0", "0.9.1", "0.10.0"];
    assert_eq!(printed(release_notes(&notes, "0.10.0", "0.9.1")), "## 0.10.0\n");
    // No release yet: nothing before the version counts as skipped.
    assert_eq!(printed(release_notes(&notes, "0.10.0", "")), "## 0.10.0\n");
}

#[test]
fn a_skipped_version_s_notes_come_after_the_release_s_under_headings() {
    // 0.10.0 sorts after 0.9.1 as a version, not as text; 0.8.0 is older than the latest release, 0.11.0 newer than
    // the one released.
    let notes = ["0.8.0", "0.9.0", "0.9.1", "0.10.0", "0.11.0"];
    assert_eq!(printed(release_notes(&notes, "0.10.0", "0.9.0")), "# v0.10.0\n\n## 0.10.0\n\n# v0.9.1\n\n## 0.9.1\n");
}

#[test]
fn a_skipped_version_without_notes_adds_nothing() {
    // 0.9.1 was skipped, and had no notes file.
    assert_eq!(printed(release_notes(&["0.9.0", "0.10.0"], "0.10.0", "0.9.0")), "## 0.10.0\n");
    // The release has none either, but the skipped one has.
    assert_eq!(printed(release_notes(&["0.9.0", "0.9.1"], "0.10.0", "0.9.0")), "# v0.9.1\n\n## 0.9.1\n");
    // Neither has.
    assert_eq!(printed(release_notes(&["0.9.0"], "0.10.0", "0.9.0")), "");
}

#[test]
fn a_call_without_both_versions_is_a_usage_error() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let output = std::process::Command::new("sh")
        .args([xtask::checkout().join("ci/release-notes.sh").to_str().expect("utf-8"), "0.10.0"])
        .current_dir(dir.path())
        .output()
        .expect("sh runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: release-notes.sh"));
}
