//! ci/version-raised.sh, which fails a pull request build into main that does not raise the release version: run on
//! a git repository of its own, whose origin's main holds a version, against a branch that changes it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use xtask::TempDir;

/// An engine/Cargo.toml whose [package] has version, between tables whose own version lines must not be taken for it.
fn manifest(version: &str) -> String {
    format!(
        "[workspace]\nversion = \"9.9.9\"\n\n[package]\nname = \"p\"\nversion = \"{version}\"\n\n\
         [dependencies.x]\nversion = \"8\"\n"
    )
}

/// The standard output of git args, run in dir; panics, with git's standard error, when git fails.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(output.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A repository on branch `change`, made from main, whose one commit holds engine/Cargo.toml at version 0.9.0, as
/// origin's main does too; origin is a bare repository beside it. Removed when dropped.
struct Repo {
    /// Holds the repository and its origin; kept for its Drop.
    _dir: TempDir,
    /// The repository's top level.
    path: PathBuf,
}

impl Repo {
    fn new() -> Repo {
        let dir = TempDir::new("xtask-version-raised-test-").expect("a temporary directory");
        let path = dir.path().join("repo");
        let origin = dir.path().join("origin.git");
        git(dir.path(), &["init", "--quiet", "-b", "main", path.to_str().expect("UTF-8")]);
        git(dir.path(), &["init", "--quiet", "--bare", origin.to_str().expect("UTF-8")]);
        let repo = Repo { _dir: dir, path };
        // Whatever the user's own git configuration says.
        for (key, setting) in [("user.name", "t"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
            repo.git(&["config", key, setting]);
        }
        repo.commit(&[("engine/Cargo.toml", &manifest("0.9.0"))], "base");
        repo.git(&["remote", "add", "origin", origin.to_str().expect("UTF-8")]);
        repo.git(&["push", "--quiet", "origin", "main"]);
        repo.git(&["switch", "--quiet", "-c", "change"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        git(&self.path, args)
    }

    /// Write each (path relative to the repository, text), making its directories, then commit every change in the
    /// working tree with message.
    fn commit(&self, files: &[(&str, &str)], message: &str) {
        for (name, text) in files {
            let path = self.path.join(name);
            fs::create_dir_all(path.parent().expect("a parent")).expect("directories made");
            fs::write(&path, text).expect("written");
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    /// The script's run at HEAD, as a pull request build into main runs it.
    fn run_script(&self) -> Output {
        let script = xtask::checkout().join("ci/version-raised.sh");
        Command::new("sh").arg(script).current_dir(&self.path).env("CHANGE_TARGET", "main").output().expect("sh runs")
    }

    /// The script's run with HEAD's engine/Cargo.toml at version, in a commit that changes another file too, so that
    /// it commits when the version is main's.
    fn check(&self, version: &str) -> Output {
        self.commit(&[("engine/Cargo.toml", &manifest(version)), ("change.txt", version)], version);
        self.run_script()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_raised_version_passes_compared_number_by_number() {
    let repo = Repo::new();
    for version in ["0.9.1", "0.10.0", "1.0.0"] {
        let output = repo.check(version);
        assert_eq!((output.status.code(), stderr(&output)), (Some(0), String::new()), "{version}");
    }
}

#[test]
fn a_kept_or_lowered_version_fails_naming_both() {
    let repo = Repo::new();
    for version in ["0.9.0", "0.8.9"] {
        let output = repo.check(version);
        assert!(!output.status.success(), "{version}");
        assert!(stderr(&output).contains(&format!("version is {version}, not greater than main's 0.9.0")), "{version}");
    }
}

#[test]
fn a_change_to_docs_tests_ci_or_repository_tooling_alone_passes_at_main_s_version() {
    let repo = Repo::new();
    let only: Vec<(&str, &str)> = [
        "docs/guide.md",
        "README.md",
        "TODO.md",
        "tests/golden/x.txt",
        "engine/e2e/recorded/a.json",
        "engine/tests/oracle/b.json",
        "engine/src/cli/tests.rs",
        "engine/src/workflow/tests/resume.rs",
        "engine/src/herdr/fake.rs",
        "engine/src/cli/fixture.rs",
        "engine/src/test_support.rs",
        "engine/src/prompt_parity.rs",
        "ui/src/App.test.tsx",
        "ui/src/runs.test.ts",
        "ui/src/test/fakes.ts",
        "ci/build.sh",
        "Jenkinsfile",
        "gate.toml",
        "pyproject.toml",
        "uv.lock",
        ".python-version",
        ".github/workflows/w.yml",
        "evals/tasks/t/task.md",
        "xtask/tests/cli.rs",
        "xtask/src/main.rs",
        ".gitignore",
        ".copier-answers.yml",
    ]
    .into_iter()
    .map(|path| (path, "x"))
    .collect();
    repo.commit(&only, "docs, tests, CI and repository tooling");
    let output = repo.run_script();
    assert_eq!((output.status.code(), stderr(&output)), (Some(0), String::new()));
    assert!(String::from_utf8_lossy(&output.stdout).contains("changes only docs, tests, CI or repository tooling"));
}

#[test]
fn a_change_that_also_touches_released_code_still_needs_the_raise() {
    let repo = Repo::new();
    for path in [
        "engine/src/main.rs",
        "engine/src/prompts/builder.md",
        "ui/src/App.tsx",
        "examples/workflows/q.toml",
        "packaging/build-desktop.sh",
    ] {
        // From main each time, so that path is the only released one in the diff.
        repo.git(&["switch", "--quiet", "-C", "change", "main"]);
        repo.commit(&[("docs/guide.md", path), (path, "x")], path);
        let output = repo.run_script();
        assert!(!output.status.success(), "{path}");
        assert!(stderr(&output).contains("version is 0.9.0, not greater than main's 0.9.0"), "{path}");
    }
}

#[test]
fn a_manifest_without_a_package_version_fails() {
    let repo = Repo::new();
    repo.commit(&[("engine/Cargo.toml", "[workspace]\nversion = \"9.9.9\"\n\n[package]\nname = \"p\"\n")], "no version");
    let output = repo.run_script();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("no [package] version in engine/Cargo.toml"), "{}", stderr(&output));
}
