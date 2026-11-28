//! Running the Release stage's scripts under ci/, which read the tags of the releases there are on standard input.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// script, a path under dir such as `ci/release-notes.sh`, run by sh in dir with args, given released, one tag each,
/// on standard input.
pub fn run(dir: &Path, script: &str, args: &[&str], released: &[&str]) -> Output {
    let mut child = Command::new("sh")
        .arg(script)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sh runs");
    let tags: String = released.iter().map(|t| format!("{t}\n")).collect();
    child.stdin.take().expect("piped").write_all(tags.as_bytes()).expect("written");
    child.wait_with_output().expect("sh ends")
}

/// What output's script printed on standard output; panics with its standard error when it did not exit 0.
pub fn printed(output: Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}
