//! ci/actions-pinned.sh, which fails its one JUnit test when a workflow or local action uses an action by anything but
//! a commit sha: run in a git checkout of its own, made in a temporary directory, holding a copy of the script and the
//! files a test writes, and on this checkout.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Output};

/// A commit sha, as a pinned `uses:` names one.
const SHA: &str = "3d3c42e5aac5ba805825da76410c181273ba90b1";

/// An image digest, as a pinned docker:// `uses:` names one.
const DIGEST: &str = "sha256:9a0c7a0f6d3a6c4b1e2f3d4c5b6a79808f7e6d5c4b3a29181716151413121110";

/// What a run of the script left: its exit status and standard error, and the JUnit report it wrote.
struct Run {
    output: Output,
    /// target/actions-pinned/junit.xml, empty when the script wrote none.
    junit: String,
}

impl Run {
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    /// Asserts the script ran and its test passed.
    fn assert_pinned(&self) {
        assert!(self.output.status.success(), "the script failed: {}", self.stderr());
        assert!(self.junit.contains("name=\"by-sha\">"), "no test in the report: {}", self.junit);
        assert!(!self.junit.contains("<failure"), "the test failed: {}", self.junit);
    }

    /// Asserts the script ran, and its test failed naming exactly the file:line places given, in that order, in the
    /// report and on standard error.
    fn assert_unpinned(&self, places: &[&str]) {
        assert!(self.output.status.success(), "the script failed: {}", self.stderr());
        let message = format!("{} uses: not pinned by a commit sha, or not read: {}\"", places.len(), places.join(" "));
        assert!(self.junit.contains(&message), "the report does not say {message:?}: {}", self.junit);
        let stderr = self.stderr();
        let named: Vec<&str> = stderr.lines().skip(1).filter_map(|line| line.split(": ").next()).collect();
        assert_eq!(named, places, "standard error: {stderr}");
    }
}

/// Run a copy of ci/actions-pinned.sh in a git checkout tracking each (path, contents) of tracked, with each of
/// untracked written beside them but not added, and each path of removed, among tracked, deleted from disk only.
fn check_tracked(tracked: &[(&str, &str)], untracked: &[(&str, &str)], removed: &[&str]) -> Run {
    let dir = tempfile::Builder::new().prefix("xtask-actions-pinned-test-").tempdir().expect("a temporary directory");
    let write = |files: &[(&str, &str)]| {
        for (path, contents) in files {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("made");
            fs::write(&path, contents).expect("written");
        }
    };
    let git = |args: &[&str]| {
        let status = Command::new("git").arg("-C").arg(dir.path()).args(args).status().expect("git runs");
        assert!(status.success(), "git {args:?}: {status}");
    };
    git(&["init", "-q"]);
    write(&[("ci/actions-pinned.sh", script().as_str())]);
    write(tracked);
    git(&["add", "-A"]);
    write(untracked);
    for path in removed {
        fs::remove_file(dir.path().join(path)).expect("removed");
    }
    run(dir.path())
}

/// Run a copy of ci/actions-pinned.sh in a git checkout tracking each (path, contents) of files.
fn check(files: &[(&str, &str)]) -> Run {
    check_tracked(files, &[], &[])
}

fn script() -> String {
    fs::read_to_string(xtask::checkout().join("ci/actions-pinned.sh")).expect("ci/actions-pinned.sh is read")
}

/// Run ci/actions-pinned.sh of the checkout at root.
fn run(root: &Path) -> Run {
    let output = Command::new("sh").arg(root.join("ci/actions-pinned.sh")).output().expect("sh runs");
    let path = root.join("target/actions-pinned/junit.xml");
    let junit = match fs::read_to_string(&path) {
        Ok(junit) => junit,
        Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
        Err(e) => panic!("could not read {}: {e}", path.display()),
    };
    Run { output, junit }
}

/// A workflow whose one job's steps are the lines given, each indented as a step.
fn workflow(steps: &[&str]) -> String {
    let mut text = String::from("on: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n");
    for step in steps {
        text.push_str(&format!("      {step}\n"));
    }
    text
}

#[test]
fn this_checkouts_workflows_are_pinned() {
    run(&xtask::checkout()).assert_pinned();
}

#[test]
fn a_sha_a_local_action_and_an_image_by_digest_pass() {
    let steps = [
        format!("- uses: actions/checkout@{SHA}"),
        format!("- uses: actions/checkout@{SHA} # v7.0.1"),
        format!("- uses: \"actions/cache@{SHA}\""),
        format!("- uses: 'actions/cache@{SHA}'  # v4.2.0"),
        "- uses: ./.github/actions/setup".to_owned(),
        format!("- uses: docker://alpine@{DIGEST}"),
        format!("- uses: docker://ghcr.io/owner/image:1.2@{DIGEST}"),
        "- name: named first".to_owned(),
        format!("  uses: owner/repo/path@{SHA}"),
        "- run: echo the word uses: in a script is not a uses".to_owned(),
    ];
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
    check(&[(".github/workflows/ci.yml", &workflow(&steps))]).assert_pinned();
}

#[test]
fn a_tag_a_branch_a_short_sha_and_no_ref_fail_naming_file_and_line() {
    let steps = [
        "- uses: actions/cache@v4",
        "- uses: actions/cache@main # the moving branch",
        "- uses: \"actions/cache@3d3c42e\"",
        "- uses: actions/cache",
        "- uses: actions/cache@3D3C42E5AAC5BA805825DA76410C181273BA90B1",
    ];
    let run = check(&[(".github/workflows/ci.yml", &workflow(&steps))]);
    run.assert_unpinned(&[
        ".github/workflows/ci.yml:6",
        ".github/workflows/ci.yml:7",
        ".github/workflows/ci.yml:8",
        ".github/workflows/ci.yml:9",
        ".github/workflows/ci.yml:10",
    ]);
    assert!(run.stderr().contains(".github/workflows/ci.yml:6: uses: actions/cache@v4\n"), "{}", run.stderr());
}

#[test]
fn an_image_by_tag_fails() {
    let steps = ["- uses: docker://alpine:3.20", "- uses: docker://alpine@sha256:9a0c7a0f"];
    check(&[(".github/workflows/ci.yml", &workflow(&steps))])
        .assert_unpinned(&[".github/workflows/ci.yml:6", ".github/workflows/ci.yml:7"]);
}

#[test]
fn every_workflow_and_local_action_is_read_and_nothing_else() {
    let composite = format!(
        "runs:\n  using: composite\n  steps:\n    - uses: actions/setup-node@v4\n    - uses: actions/cache@{SHA}\n"
    );
    let action = "runs:\n  using: composite\n  steps:\n    - uses: a/b@v1\n";
    let run = check_tracked(
        &[
            (".github/workflows/a.yaml", &workflow(&["- uses: actions/checkout@v7"])),
            (".github/workflows/b.yml", &workflow(&[&format!("- uses: actions/checkout@{SHA}")])),
            (".github/actions/setup/action.yml", &composite),
            (".github/actions/other/action.yaml", action),
            // A local action outside .github, which a workflow names by ./ci/actions/x or ./.
            ("ci/actions/x/action.yml", action),
            ("action.yml", action),
            // Neither a workflow nor an action GitHub runs: a file of another kind and a workflow below the top level.
            (".github/dependabot.yml", "uses: a/b@v1\n"),
            (".github/workflows/notes.md", "uses: a/b@v1\n"),
            (".github/workflows/sub/c.yml", &workflow(&["- uses: a/b@v1"])),
            // Tracked, but deleted from the working tree.
            (".github/workflows/removed.yml", &workflow(&["- uses: a/b@v1"])),
        ],
        &[
            // Not the repository's: what a build or a tool puts in the checkout, such as a crate's own action.
            (".cargo-home/registry/src/c-1.0/.github/actions/setup/action.yml", action),
            (".github/workflows/untracked.yml", &workflow(&["- uses: a/b@v1"])),
        ],
        &[".github/workflows/removed.yml"],
    );
    run.assert_unpinned(&[
        ".github/actions/other/action.yaml:4",
        ".github/actions/setup/action.yml:4",
        ".github/workflows/a.yaml:6",
        "action.yml:4",
        "ci/actions/x/action.yml:4",
    ]);
}

#[test]
fn only_a_uses_github_runs_is_judged() {
    let text = format!(
        "on: push\njobs:\n  build:\n    runs-on: x\n    env:\n      uses: a/env@v1\n    steps:\n      - run: |\n          \
         uses: a/script@v1\n          steps:\n            - uses: a/script-steps@v1\n      - name: >-\n          uses: a/folded@v1\n      - uses: a/b@{SHA}\n        with:\n          \
         uses: a/input@v1\n      - uses: a/b@{SHA}\n        with: {{uses: a/nested@v1, steps: [{{uses: a/s@v1}}]}}\n"
    );
    check(&[(".github/workflows/ci.yml", &text)]).assert_pinned();
}

#[test]
fn a_reusable_workflow_and_crlf_lines_are_read() {
    let text = format!(
        "on: push\r\njobs:\r\n  call:\r\n    uses: o/r/.github/workflows/w.yml@v1\r\n  build:\r\n    steps:\r\n      \
         - uses: a/b@{SHA}\r\n      - uses: a/crlf@v1\r\n"
    );
    check(&[(".github/workflows/ci.yml", &text)])
        .assert_unpinned(&[".github/workflows/ci.yml:4", ".github/workflows/ci.yml:8"]);
}

#[test]
fn a_comment_after_a_key_hides_nothing_under_it() {
    let text = "on: push\njobs: # all jobs\n  build:\n    steps: # each step\n      - uses: a/step@v1\n";
    let action = "runs: # composite\n  using: composite\n  steps:\n    - uses: a/action@v1\n";
    check(&[(".github/workflows/ci.yml", text), ("action.yml", action)])
        .assert_unpinned(&[".github/workflows/ci.yml:5", "action.yml:4"]);
}

#[test]
fn a_step_or_job_in_flow_style_fails_as_not_read_even_when_pinned() {
    let text = format!(
        "on: push\njobs:\n  call: {{uses: o/r/w.yml@{SHA}}}\n  build:\n    steps:\n      \
         - {{name: \"Build #1\", uses: a/b@{SHA}}}\n  other:\n    steps: [{{uses: a/b@{SHA}}}]\n"
    );
    let run = check(&[
        (".github/workflows/ci.yml", &text),
        (".github/workflows/jobs.yml", "on: push\njobs: {build: {steps: [{uses: a/b@v1}]}}\n"),
        ("action.yml", "runs: {using: composite, steps: [{uses: a/c@v1}]}\n"),
        ("ci/a/action.yml", "runs:\n  using: composite\n  steps: [{uses: a/c@v1}]\n"),
    ]);
    run.assert_unpinned(&[
        ".github/workflows/ci.yml:3",
        ".github/workflows/ci.yml:6",
        ".github/workflows/ci.yml:8",
        ".github/workflows/jobs.yml:2",
        "action.yml:1",
        "ci/a/action.yml:3",
    ]);
    let stderr = run.stderr();
    let step = ".github/workflows/ci.yml:6: a step in flow style, which this check does not read";
    assert!(stderr.contains(step), "{stderr}");
}

#[test]
fn a_flow_collection_below_its_key_or_an_anchor_tag_or_alias_fails_as_not_read() {
    let anchored = "on: push\njobs:\n  a:\n    steps: &s [{uses: a/b@v1}]\n  b:\n    steps: !!seq [{uses: a/b@v1}]\n  \
                    c: &j {uses: o/r/w.yml@v1}\n  d:\n    steps: *s\n";
    let run = check(&[
        (".github/workflows/anchors.yml", anchored),
        (".github/workflows/ci.yml", &workflow(&[&format!("- &x {{uses: a/b@{SHA}}}"), "- &k uses: a/b@v1"])),
        (".github/workflows/job-below.yml", "on: push\njobs:\n  build:\n    {runs-on: x, steps: [{uses: a/b@v1}]}\n"),
        (".github/workflows/jobs-anchor.yml", "on: push\njobs: &j {build: {steps: [{uses: a/b@v1}]}}\n"),
        (".github/workflows/jobs-below.yml", "on: push\njobs:\n  {build: {steps: [{uses: a/b@v1}]}}\n"),
        ("action.yml", "runs:\n  {using: composite, steps: [{uses: a/c@v1}]}\n"),
    ]);
    run.assert_unpinned(&[
        ".github/workflows/anchors.yml:4",
        ".github/workflows/anchors.yml:6",
        ".github/workflows/anchors.yml:7",
        ".github/workflows/anchors.yml:9",
        ".github/workflows/ci.yml:6",
        ".github/workflows/ci.yml:7",
        ".github/workflows/job-below.yml:4",
        ".github/workflows/jobs-anchor.yml:2",
        ".github/workflows/jobs-below.yml:3",
        "action.yml:2",
    ]);
    let stderr = run.stderr();
    for said in [
        ".github/workflows/ci.yml:6: a step with an anchor, tag or alias, which this check does not read",
        ".github/workflows/job-below.yml:4: job build in flow style",
        ".github/workflows/jobs-below.yml:3: jobs: in flow style",
        "action.yml:2: runs: in flow style",
    ] {
        assert!(stderr.contains(said), "{said}: {stderr}");
    }
}

#[test]
fn a_checkout_without_github_passes() {
    check(&[]).assert_pinned();
}
