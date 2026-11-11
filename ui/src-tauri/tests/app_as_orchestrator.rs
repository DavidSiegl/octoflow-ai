//! The app's executable as the orchestrator: given arguments it is the engine's CLI, `serve` included, which is how
//! the window starts its sidecar. No test here opens a window.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// The app's executable, as cargo built it for these tests.
const APP: &str = env!("CARGO_BIN_EXE_octoflow-ui");

#[test]
fn given_arguments_the_app_is_the_cli() {
    let out = Command::new(APP).arg("workflows").output().expect("the app runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let listed = String::from_utf8_lossy(&out.stdout);
    assert!(listed.lines().any(|line| line.starts_with("default ")), "{listed}");
}

#[test]
fn the_cli_s_exit_status_is_the_app_s() {
    let out = Command::new(APP).arg("no-such-command").output().expect("the app runs");
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn the_app_serves_as_its_own_sidecar() {
    let mut serve = Command::new(APP)
        .args(["serve", "--origin", "tauri://localhost"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the app runs");
    let mut line = String::new();
    let read = BufReader::new(serve.stdout.take().expect("serve's output")).read_line(&mut line);
    serve.kill().expect("serve killed");
    serve.wait().expect("serve waited for");
    read.expect("serve's first line read");
    let served: serde_json::Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line:?}: {e}"));
    assert!(served["url"].as_str().is_some_and(|url| url.starts_with("http://127.0.0.1:")), "{line}");
    assert!(served["token"].as_str().is_some_and(|token| !token.is_empty()), "{line}");
}

/// The app's executable run with args as an AppImage would start it from proj, a directory since removed: `OWD`
/// names it, and `APPDIR` and `APPIMAGE` name a bundle, which need not exist.
fn from_a_removed_directory(args: &[&str]) -> std::process::Output {
    Command::new(APP)
        .args(args)
        .env("APPDIR", "/nonexistent/Octoflow.AppDir")
        .env("APPIMAGE", "/nonexistent/octoflow.AppImage")
        .env("OWD", "/nonexistent/proj")
        .stdin(Stdio::null())
        .output()
        .expect("the app runs")
}

#[test]
fn the_cli_refuses_a_directory_since_removed() {
    let out = from_a_removed_directory(&["workflows"]);
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("octoflow: cannot start in /nonexistent/proj: it is not a directory"), "{said}");
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn the_sidecar_refuses_it_too_before_its_first_line() {
    let out = from_a_removed_directory(&["serve", "--origin", "tauri://localhost"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
}
