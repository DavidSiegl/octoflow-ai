//! ci/fetch-gate.sh, which fetches the release a pin names: what it unpacks, and what it refuses before unpacking.
//! A copy of octoflow-gate's test of its own copy of the script: a fix to either goes to both repositories.
//!
//! Offline: a curl stub on PATH serves a tarball the test builds, and records each URL it was asked for.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Copies FAKE_TARBALL to the file after -o and appends the URL, its last argument, to CURL_LOG; exits 22, as curl -f
/// does on an HTTP error, when FAKE_TARBALL is unset.
const CURL_STUB: &str = r#"#!/bin/sh
for arg; do last=$arg; done
echo "$last" >> "$CURL_LOG"
[ -n "${FAKE_TARBALL:-}" ] || exit 22
while [ $# -gt 0 ]; do
    if [ "$1" = -o ]; then cp "$FAKE_TARBALL" "$2"; exit 0; fi
    shift
done
exit 2
"#;

/// What the release's one file, gate, holds.
const GATE: &str = "the released gate\n";

/// A directory holding the curl stub, a release tarball of gate, and where the script unpacks to. Removed when
/// dropped.
struct Fetch {
    dir: tempfile::TempDir,
    bin: PathBuf,
    tarball: PathBuf,
    /// The tarball's sha256, in lower-case hex.
    sha256: String,
    /// Whether curl's stub serves the tarball (true) or fails as on an HTTP error.
    served: bool,
}

/// Run program with args in dir; panics unless it succeeds. Returns its standard output.
fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let output = Command::new(program).current_dir(dir).args(args).output().expect("runs");
    assert!(output.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

impl Fetch {
    fn new() -> Fetch {
        let dir = tempfile::Builder::new().prefix("xtask-fetch-gate-test-").tempdir().expect("a temporary directory");
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).expect("made");
        let curl = bin.join("curl");
        fs::write(&curl, CURL_STUB).expect("written");
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).expect("made executable");
        let release = dir.path().join("release");
        fs::create_dir(&release).expect("made");
        fs::write(release.join("gate"), GATE).expect("written");
        let tarball = dir.path().join("release.tar.gz");
        run(dir.path(), "tar", &["-czf", "release.tar.gz", "-C", "release", "gate"]);
        let sum = run(dir.path(), "sha256sum", &["release.tar.gz"]);
        let sha256 = sum.split_whitespace().next().expect("a sum").to_owned();
        Fetch { dir, bin, tarball, sha256, served: true }
    }

    fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    /// Run fetch-gate.sh on the pin pin, into out().
    fn fetch_pin(&self, pin: &Path) -> Output {
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap_or_default());
        let mut command = Command::new("sh");
        command
            .arg(xtask::checkout().join("ci/fetch-gate.sh"))
            .arg(pin)
            .arg(self.out())
            .env("PATH", path)
            .env("CURL_LOG", self.dir.path().join("curl.log"));
        if self.served {
            command.env("FAKE_TARBALL", &self.tarball);
        } else {
            command.env_remove("FAKE_TARBALL");
        }
        command.output().expect("sh runs")
    }

    /// Run fetch-gate.sh on a pin holding text, into out().
    fn fetch(&self, text: &str) -> Output {
        let pin = self.dir.path().join("gate.pin");
        fs::write(&pin, text).expect("written");
        self.fetch_pin(&pin)
    }

    /// The URLs curl was asked for, in order.
    fn urls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("curl.log"))
            .map(|log| log.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_matching_release_is_unpacked_and_its_version_printed() {
    let fetch = Fetch::new();
    let output = fetch.fetch(&format!("# a comment\n0.5.0 {}\n", fetch.sha256));
    assert_eq!(
        (output.status.code(), String::from_utf8_lossy(&output.stdout).into_owned()),
        (Some(0), "0.5.0\n".into())
    );
    let release = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v0.5.0";
    assert_eq!(fetch.urls(), [format!("{release}/octoflow-gate-0.5.0.tar.gz")]);
    assert_eq!(fs::read_to_string(fetch.out().join("gate")).expect("unpacked"), GATE);
}

#[test]
fn a_tarball_that_does_not_match_the_sha256_unpacks_nothing() {
    let fetch = Fetch::new();
    let output = fetch.fetch(&format!("0.5.0 {}\n", "0".repeat(64)));
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("does not match the sha256"), "{}", stderr(&output));
    assert!(!fetch.out().exists());
}

#[test]
fn a_failed_download_fails() {
    let mut fetch = Fetch::new();
    fetch.served = false;
    let output = fetch.fetch(&format!("0.5.0 {}\n", fetch.sha256));
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("cannot download"), "{}", stderr(&output));
    assert!(!fetch.out().exists());
}

#[test]
fn a_malformed_pin_is_refused_before_any_download() {
    let fetch = Fetch::new();
    let sha256 = &fetch.sha256;
    for text in [
        "# only a comment\n".to_owned(),
        "0.5.0\n".to_owned(),
        format!("0.5.0 {}\n", &sha256[..sha256.len() - 1]),
        format!("0.5.0 {}\n", sha256.to_uppercase()),
        format!("../0.5.0 {sha256}\n"),
        format!("0.5.0?x {sha256}\n"),
    ] {
        let output = fetch.fetch(&text);
        assert_eq!(output.status.code(), Some(1), "{text:?}");
        assert!(stderr(&output).contains("is not '<version> <sha256>'"), "{text:?}: {}", stderr(&output));
    }
    assert_eq!(fetch.urls(), Vec::<String>::new());
}

#[test]
fn an_unreadable_pin_is_refused() {
    let fetch = Fetch::new();
    let output = fetch.fetch_pin(&fetch.dir.path().join("missing.pin"));
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("cannot read the pin"), "{}", stderr(&output));
    assert_eq!(fetch.urls(), Vec::<String>::new());
}
