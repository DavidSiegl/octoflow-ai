//! ci/release-notes.sh, which gives the Release stage the notes of a release: its own version's docs/releases/ file,
//! and those of the versions above the highest release that main reached without a release of their own.

mod release_scripts;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

/// A tree to run the script in: a copy of it and of ci/release-tags.sh, which it sources, and docs/releases/ holding a
/// notes file `## <version>\n` for each of notes. Removed when dropped.
fn tree(notes: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix("xtask-release-notes-test-").tempdir().expect("a temporary directory");
    fs::create_dir(dir.path().join("ci")).expect("made");
    fs::create_dir_all(dir.path().join("docs/releases")).expect("made");
    for script in ["ci/release-notes.sh", "ci/release-tags.sh"] {
        fs::copy(xtask::checkout().join(script), dir.path().join(script)).expect("copied");
    }
    for v in notes {
        fs::write(dir.path().join(format!("docs/releases/v{v}.md")), format!("## {v}\n")).expect("written");
    }
    dir
}

/// The script's run in dir with args, given the release tags released on standard input.
fn run(dir: &Path, args: &[&str], released: &[&str]) -> Output {
    release_scripts::run(dir, "ci/release-notes.sh", args, released)
}

/// What the script printed on standard output for version, with notes and released as tree and run take them;
/// panics with its standard error when it did not exit 0.
fn printed(notes: &[&str], version: &str, released: &[&str]) -> String {
    let dir = tree(notes);
    release_scripts::printed(run(dir.path(), &[version], released))
}

#[test]
fn a_release_right_after_the_highest_has_its_own_notes_as_they_are() {
    let notes = ["0.9.0", "0.9.1", "0.10.0"];
    assert_eq!(printed(&notes, "0.10.0", &["v0.9.0", "v0.9.1"]), "## 0.10.0\n");
    // No release yet: nothing before the version counts as skipped.
    assert_eq!(printed(&notes, "0.10.0", &[]), "## 0.10.0\n");
}

#[test]
fn a_skipped_version_s_notes_come_after_the_release_s_under_headings() {
    // 0.10.0 sorts after 0.9.1 as a version, not as text; 0.8.0 is older than the highest release, 0.11.0 newer than
    // the one released; a tag that is no plain version, which sort -V would put above them, is no release.
    let notes = ["0.8.0", "0.9.0", "0.9.1", "0.10.0", "0.11.0"];
    assert_eq!(
        printed(&notes, "0.10.0", &["v0.9.0", "v0.8.0", "nightly", "vnext", "v0.10.0-rc.1"]),
        "# v0.10.0\n\n## 0.10.0\n\n# v0.9.1\n\n## 0.9.1\n\n# Pull requests\n"
    );
}

#[test]
fn a_release_below_the_highest_takes_no_other_version_s_notes() {
    // 0.10.0's build released first, with 0.9.1's notes; 0.9.2's build releases after it.
    let notes = ["0.9.0", "0.9.1", "0.9.2", "0.10.0"];
    assert_eq!(printed(&notes, "0.9.2", &["v0.9.0", "v0.10.0"]), "## 0.9.2\n");
}

#[test]
fn a_skipped_version_without_notes_adds_nothing() {
    // 0.9.1 was skipped, and had no notes file.
    assert_eq!(printed(&["0.9.0", "0.10.0"], "0.10.0", &["v0.9.0"]), "## 0.10.0\n");
    // The release has none either, but the skipped one has.
    assert_eq!(printed(&["0.9.0", "0.9.1"], "0.10.0", &["v0.9.0"]), "# v0.9.1\n\n## 0.9.1\n\n# Pull requests\n");
    // Neither has.
    assert_eq!(printed(&["0.9.0"], "0.10.0", &["v0.9.0"]), "");
}

#[test]
fn notes_that_cannot_be_read_print_nothing_and_fail() {
    let dir = tree(&["0.9.0", "0.9.1", "0.10.0"]);
    let unreadable = dir.path().join("docs/releases/v0.9.1.md");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).expect("made unreadable");
    if fs::read(&unreadable).is_ok() {
        // Run as root, which reads it anyway.
        return;
    }
    let output = run(dir.path(), &["0.10.0"], &["v0.9.0"]);
    assert_eq!((output.status.code(), output.stdout.as_slice()), (Some(1), &b""[..]));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read docs/releases/v0.9.1.md"));
}

#[test]
fn a_call_without_the_version_is_a_usage_error() {
    let dir = tree(&[]);
    let output = run(dir.path(), &[], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: release-notes.sh"));
}
