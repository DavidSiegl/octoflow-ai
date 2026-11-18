//! `cargo xtask raise-gate-pin`: which release it pins, what it changes with the pin, and that it changes nothing
//! when a check fails.
//!
//! Offline: the xtask runs in a small checkout of its own, with this checkout's ci/fetch-gate.sh, and stubs of gh and
//! curl on PATH serve a release list and a release tarball the test builds.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Prints FAKE_RELEASES, the file of `gh api --paginate --slurp`'s output, whatever it is asked.
const GH_STUB: &str = "#!/bin/sh\ncat \"$FAKE_RELEASES\"\n";

/// Appends the URL, its last argument, to CURL_LOG; prints FAKE_SHA256 for a .sha256 asset and copies FAKE_TARBALL to
/// the file after -o for anything else.
const CURL_STUB: &str = r#"#!/bin/sh
for arg; do last=$arg; done
echo "$last" >> "$CURL_LOG"
case $last in *.sha256) cat "$FAKE_SHA256"; exit 0 ;; esac
while [ $# -gt 0 ]; do
    if [ "$1" = -o ]; then cp "$FAKE_TARBALL" "$2"; exit 0; fi
    shift
done
exit 2
"#;

/// The pinned release's sha256 in the checkout before a raise.
const OLD_SHA: &str = "8f6007f046be5fcf516496201a41bc7185a3af53c5e7778a9489a0d10414525a";
const PIN_COMMENT: &str = "# The release that judges this repository.\n";
/// The released golden reports, as (name, what it holds).
const GOLDEN: &[(&str, &str)] = &[("gate_ok.json", "{\"new\": true}\n"), ("gate_ok.txt", "new\n")];

/// A checkout pinning octoflow-gate 0.10.0 at octoflow 0.89.3, with the stubs and the release they serve. Removed
/// when dropped.
struct Raise {
    dir: tempfile::TempDir,
}

/// Run program with args in dir; panics unless it succeeds. Returns its standard output.
fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let output = Command::new(program).current_dir(dir).args(args).output().expect("runs");
    assert!(output.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("made");
    fs::write(path, text).expect("written");
}

/// A release as GitHub's API lists it.
fn release(version: &str, immutable: bool) -> serde_json::Value {
    serde_json::json!({
        "tag_name": format!("v{version}"),
        "draft": false,
        "prerelease": false,
        "immutable": immutable,
        "body": format!("Notes of {version}."),
    })
}

impl Raise {
    /// The checkout, with releases as the release list, served in two pages.
    fn new(releases: &[serde_json::Value]) -> Raise {
        let dir =
            tempfile::Builder::new().prefix("xtask-raise-gate-pin-test-").tempdir().expect("a temporary directory");
        let raise = Raise { dir };
        for (name, stub) in [("gh", GH_STUB), ("curl", CURL_STUB)] {
            let path = raise.bin().join(name);
            write(&path, stub);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("made executable");
        }
        let (first, second) = releases.split_at(releases.len() / 2);
        write(&raise.dir.path().join("releases.json"), &serde_json::json!([first, second]).to_string());

        let release = raise.dir.path().join("release");
        for (name, text) in GOLDEN {
            write(&release.join("golden").join(name), text);
        }
        write(&release.join("gate"), "the released gate\n");
        run(raise.dir.path(), "tar", &["-czf", "release.tar.gz", "-C", "release", "gate", "golden"]);
        let sum = run(raise.dir.path(), "sha256sum", &["release.tar.gz"]);
        write(&raise.dir.path().join("release.sha256"), &format!("{}  release.tar.gz\n", raise.release_sha()));
        assert_eq!(sum.split_whitespace().next(), Some(raise.release_sha().as_str()));

        let repo = raise.repo();
        write(&repo.join("Cargo.toml"), "[workspace]\nmembers = [\"engine\"]\nresolver = \"2\"\n");
        write(
            &repo.join("engine/Cargo.toml"),
            "[package]\nname = \"engine\"\nversion = \"0.89.3\"\nedition = \"2021\"\n\n[dependencies]\n",
        );
        write(&repo.join("engine/src/lib.rs"), "");
        write(&repo.join("ci/gate.pin"), &format!("{PIN_COMMENT}0.10.0 {OLD_SHA}\n"));
        fs::copy(xtask::checkout().join("ci/fetch-gate.sh"), repo.join("ci/fetch-gate.sh")).expect("copied");
        let quality = repo.join("engine/tests/data/quality");
        write(&quality.join("README.md"), &format!("The golden files of octoflow-gate 0.10.0, sha256 `{OLD_SHA}`.\n"));
        write(&quality.join("gate_error.json"), "{\"old\": true}\n");
        write(&quality.join("gate_error.txt"), "old\n");
        write(&repo.join("docs/releases/v0.89.3.md"), "## Earlier\n");
        run(&repo, "git", &["init", "-q"]);
        raise
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    /// The sha256 the release's .sha256 asset gives, its tarball's.
    fn release_sha(&self) -> String {
        let sum = run(self.dir.path(), "sha256sum", &["release.tar.gz"]);
        sum.split_whitespace().next().expect("a sum").to_owned()
    }

    /// Run `cargo xtask raise-gate-pin args` in a subdirectory of the checkout.
    fn raise(&self, args: &[&str]) -> Output {
        let path = format!("{}:{}", self.bin().display(), std::env::var("PATH").unwrap_or_default());
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("raise-gate-pin")
            .args(args)
            .current_dir(self.repo().join("engine"))
            .env("PATH", path)
            .env("FAKE_RELEASES", self.dir.path().join("releases.json"))
            .env("FAKE_TARBALL", self.dir.path().join("release.tar.gz"))
            .env("FAKE_SHA256", self.dir.path().join("release.sha256"))
            .env("CURL_LOG", self.dir.path().join("curl.log"))
            .output()
            .expect("xtask runs")
    }

    /// Every file of the checkout outside .git and target, by path, with what it holds.
    fn files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).expect("readable") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    if !path.ends_with(".git") && !path.ends_with("target") {
                        walk(&path, root, files);
                    }
                } else {
                    let rel = path.strip_prefix(root).expect("under root").to_path_buf();
                    files.insert(rel, fs::read(&path).expect("readable"));
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(&self.repo(), &self.repo(), &mut files);
        files
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.repo().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// The version in the checkout's engine/Cargo.toml.
    fn engine_version(&self) -> String {
        let manifest = self.read("engine/Cargo.toml");
        let line = manifest.lines().find(|l| l.starts_with("version = ")).expect("a version");
        line.trim_start_matches("version = ").trim_matches('"').to_owned()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_latest_release_is_pinned_with_everything_that_goes_with_it() {
    let mut draft = release("0.13.0", true);
    draft["draft"] = true.into();
    let mut pre = release("0.12.1", true);
    pre["prerelease"] = true.into();
    let raise = Raise::new(&[release("0.10.0", true), release("0.11.0", true), release("0.12.0", true), draft, pre]);
    let output = raise.raise(&[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let sha = raise.release_sha();

    assert_eq!(raise.read("ci/gate.pin"), format!("{PIN_COMMENT}0.12.0 {sha}\n"));
    let quality = raise.repo().join("engine/tests/data/quality");
    let mut golden: Vec<String> = fs::read_dir(&quality)
        .expect("readable")
        .map(|e| e.expect("an entry").file_name().into_string().expect("utf-8"))
        .collect();
    golden.sort();
    assert_eq!(golden, ["README.md", "gate_ok.json", "gate_ok.txt"]);
    assert_eq!(fs::read_to_string(quality.join("gate_ok.json")).expect("copied"), GOLDEN[0].1);
    assert_eq!(
        raise.read("engine/tests/data/quality/README.md"),
        format!("The golden files of octoflow-gate 0.12.0, sha256 `{sha}`.\n")
    );
    assert_eq!(raise.engine_version(), "0.90.0");
    assert!(raise.read("Cargo.lock").contains("version = \"0.90.0\""));
    let notes = raise.read("docs/releases/v0.90.0.md");
    assert!(notes.starts_with("## The quality gate is octoflow-gate 0.12.0\n"), "{notes}");
    assert!(notes.contains("in place of 0.10.0"), "{notes}");
    let tag = "https://github.com/DavidSiegl/octoflow-gate/releases/tag";
    let words = notes.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(words.contains(&format!("from [0.11.0]({tag}/v0.11.0) to [0.12.0]({tag}/v0.12.0).")), "{notes}");
    assert!(notes.lines().all(|l| l.len() <= 120), "{notes}");
    assert_eq!(raise.read("docs/releases/v0.89.3.md"), "## Earlier\n");

    assert_eq!(
        stdout(&output),
        "## octoflow-gate 0.12.0\n\nNotes of 0.12.0.\n\n## octoflow-gate 0.11.0\n\nNotes of 0.11.0.\n\n"
    );
    let download = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v0.12.0";
    assert_eq!(
        fs::read_to_string(raise.dir.path().join("curl.log")).expect("curl ran"),
        format!("{download}/octoflow-gate-0.12.0.tar.gz.sha256\n{download}/octoflow-gate-0.12.0.tar.gz\n")
    );
}

#[test]
fn a_second_run_changes_nothing() {
    let raise = Raise::new(&[release("0.10.0", true), release("0.11.0", true)]);
    assert_eq!(raise.raise(&[]).status.code(), Some(0));
    let before = raise.files();
    let output = raise.raise(&[]);
    assert_eq!((output.status.code(), stdout(&output)), (Some(0), String::new()), "{}", stderr(&output));
    assert!(stderr(&output).contains("already names octoflow-gate 0.11.0"), "{}", stderr(&output));
    assert_eq!(raise.files(), before);
}

#[test]
fn a_mutable_latest_release_is_refused_and_one_named_is_taken() {
    let raise = Raise::new(&[release("0.10.0", true), release("0.10.1", false)]);
    let before = raise.files();
    let output = raise.raise(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("latest release 0.10.1 is not immutable"), "{}", stderr(&output));
    assert_eq!(raise.files(), before);

    let output = raise.raise(&["0.10.1"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(raise.read("ci/gate.pin"), format!("{PIN_COMMENT}0.10.1 {}\n", raise.release_sha()));
    assert_eq!(raise.engine_version(), "0.89.4", "a gate patch is a patch");
    assert!(raise.read("docs/releases/v0.89.4.md").starts_with("## The quality gate is octoflow-gate 0.10.1\n"));
}

#[test]
fn a_raise_onto_an_existing_release_notes_file_appends_to_it() {
    let raise = Raise::new(&[release("0.10.0", true), release("0.10.1", true)]);
    write(&raise.repo().join("docs/releases/v0.89.4.md"), "## Something else\n");
    assert_eq!(raise.raise(&[]).status.code(), Some(0));
    let notes = raise.read("docs/releases/v0.89.4.md");
    assert!(notes.starts_with("## Something else\n\n## The quality gate is octoflow-gate 0.10.1\n"), "{notes}");
}

#[test]
fn a_tarball_that_does_not_match_its_sha256_changes_nothing() {
    let raise = Raise::new(&[release("0.10.0", true), release("0.11.0", true)]);
    write(&raise.dir.path().join("release.sha256"), &format!("{}  release.tar.gz\n", "0".repeat(64)));
    let before = raise.files();
    let output = raise.raise(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("does not match the sha256"), "{}", stderr(&output));
    assert_eq!(raise.files(), before);
}

#[test]
fn a_release_that_is_missing_or_ships_no_gate_is_refused() {
    let raise = Raise::new(&[release("0.6.0", true), release("0.10.0", true)]);
    let before = raise.files();
    for (version, why) in
        [("0.11.0", "has no published release 0.11.0"), ("0.6.0", "ships no gate"), ("x", "is not a version")]
    {
        let output = raise.raise(&[version]);
        assert_eq!(output.status.code(), Some(1), "{version}");
        assert!(stderr(&output).contains(why), "{version}: {}", stderr(&output));
    }
    assert_eq!(raise.files(), before);
}
