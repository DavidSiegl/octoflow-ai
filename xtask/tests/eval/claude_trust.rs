//! Claude Code's trust of a trial's clone (xtask::claude_trust): marked in its config file before the engine starts,
//! and removed with the clone, the rest of the file as it was.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::{json, Value};
use xtask::claude_trust::config_file;
use xtask::eval::{self, Halt};

use super::{after, load, strings, FakeEngine, Plan};

/// A config file as Claude Code leaves one: other keys before and after `projects`, another folder trusted, the file
/// readable by its owner alone. Its text, as the runner writes it back.
fn claude_config(path: &Path) -> String {
    let config = json!({
        "numStartups": 3,
        "projects": {"/home/someone/project": {"hasTrustDialogAccepted": true, "lastCost": 0.5}},
        "oauthAccount": {"emailAddress": "someone@example.com"},
    });
    let text = format!("{}\n", serde_json::to_string_pretty(&config).expect("JSON"));
    fs::write(path, &text).expect("the config is written");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("its mode is set");
    text
}

/// The folders config's `projects` trusts, in its order.
fn trusted(config: &Value) -> Vec<String> {
    let projects = config["projects"].as_object().expect("projects is an object");
    projects.iter().filter(|(_, p)| p["hasTrustDialogAccepted"] == true).map(|(path, _)| path.clone()).collect()
}

#[test]
fn a_clone_is_trusted_while_the_engine_runs_and_forgotten_with_it() {
    let engine = FakeEngine::new();
    engine.plan(Plan::default());
    let before = claude_config(&engine.claude_config());
    engine.trial("feature", &[], false, 1);
    let run = &engine.log()[0];
    let seen: Value = serde_json::from_str(run["seen"].as_str().expect("the engine saw the config")).expect("JSON");
    let cwd = after(&run["argv"], "--cwd");
    assert_eq!(trusted(&seen), ["/home/someone/project", cwd], "{seen:#}");
    assert_eq!(seen["numStartups"], 3);
    assert_eq!(fs::read_to_string(engine.claude_config()).expect("the config reads"), before);
    let mode = fs::metadata(engine.claude_config()).expect("its metadata").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_kept_clone_stays_trusted() {
    let engine = FakeEngine::new();
    engine.plan(Plan::default());
    claude_config(&engine.claude_config());
    engine.trial("feature", &[], true, 1);
    let repo = Path::new(after(&engine.log()[0]["argv"], "--cwd")).to_path_buf();
    let config: Value =
        serde_json::from_str(&fs::read_to_string(engine.claude_config()).expect("reads")).expect("JSON");
    let canonical = repo.canonicalize().map(|path| path.to_string_lossy().into_owned());
    fs::remove_dir_all(repo.parent().expect("the clone's directory")).expect("the kept directory is removed");
    assert_eq!(trusted(&config), ["/home/someone/project".to_string(), canonical.expect("the clone is there")]);
}

#[test]
fn a_config_that_is_a_link_stays_one() {
    let engine = FakeEngine::new();
    engine.plan(Plan::default());
    let link = engine.claude_config();
    let target = link.with_file_name("dotfiles.claude.json");
    let before = claude_config(&target);
    std::os::unix::fs::symlink(&target, &link).expect("the link is made");
    engine.trial("feature", &[], false, 1);
    let seen: Value = serde_json::from_str(engine.log()[0]["seen"].as_str().expect("seen")).expect("JSON");
    assert_eq!(trusted(&seen).len(), 2, "{seen:#}");
    assert!(fs::symlink_metadata(&link).expect("the link is there").file_type().is_symlink());
    assert_eq!(fs::read_to_string(&target).expect("the target reads"), before);
}

#[test]
fn without_a_config_file_none_is_made() {
    let engine = FakeEngine::new();
    engine.plan(Plan::default());
    engine.trial("feature", &[], false, 1);
    assert_eq!(engine.log()[0]["seen"], Value::Null);
    assert!(!engine.claude_config().exists());
}

#[test]
fn a_config_that_is_not_json_fails_the_trial_before_the_engine_runs() {
    let engine = FakeEngine::new();
    engine.plan(Plan::default());
    fs::write(engine.claude_config(), "{\"projects\":").expect("written");
    let (task,) = load("feature");
    let config = engine.claude_config();
    match eval::run_trial(&task, 1, &engine.command(), &strings(&[]), false, Some(&config)) {
        Err(Halt::Failed(e)) => {
            let named = format!("could not parse Claude Code's config {}", config.display());
            assert!(e.0.starts_with(&named), "{e}");
        }
        other => panic!("not an error of the runner's: {other:?}"),
    }
    assert_eq!(engine.log(), Vec::<Value>::new(), "the engine never started");
}

#[test]
fn the_config_file_is_in_claude_config_dir_else_home() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |name: &str| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| v.to_string())
    };
    let both = env(&[("CLAUDE_CONFIG_DIR", "/c"), ("HOME", "/h")]);
    assert_eq!(config_file(&both), Some("/c/.claude.json".into()));
    let empty = env(&[("CLAUDE_CONFIG_DIR", ""), ("HOME", "/h")]);
    assert_eq!(config_file(&empty), Some("/h/.claude.json".into()));
    assert_eq!(config_file(&env(&[])), None);
}
