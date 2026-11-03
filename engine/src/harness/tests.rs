//! The harnesses as the Python engine knows them: tests/test_orchestrator.py's launch-argument cases of
//! TestHarnessFlags, and harness.json's tables (engine/tests/oracle/harness.py), which hold every harness, permission
//! mode, model, session and directory, and the token usage of sample session logs.

use super::*;
use crate::constants::DEFAULT_AGENT;
use crate::test_support::{oracle, unhex};
use crate::json::Object;

/// The cases harness.json holds of kind.
fn cases(kind: &str) -> Vec<Vec<Value>> {
    let recorded = oracle("harness.json");
    let cases = recorded.get(kind).and_then(Value::as_array).unwrap_or_else(|| panic!("harness.json has no {kind}"));
    cases.iter().map(|case| case.as_array().cloned().unwrap()).collect()
}

/// A string or null of the oracle as an Option.
fn text(value: &Value) -> Option<&str> {
    value.as_str()
}

/// A list of strings of the oracle.
fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect()
}

#[test]
fn every_harness_s_tables_are_the_python_engine_s() {
    let recorded = oracle("harness.json");
    let constants = recorded.get("constants").unwrap();
    let kinds: Vec<&str> = HARNESSES.iter().map(|h| h.kind).collect();
    assert_eq!(strings(constants.get("AGENT_KINDS").unwrap()), kinds);
    assert_eq!(kinds, AGENT_KINDS);
    assert_eq!(constants.get("DEFAULT_AGENT").and_then(Value::as_str), Some(DEFAULT_AGENT));
    // Python's four tables, as the rows here make them.
    let (mut permissions, mut resume, mut add_dir, mut logs) = (Object::new(), Object::new(), Object::new(), Object::new());
    for h in &HARNESSES {
        if let Permissions::Equivalents(table) = h.permissions {
            let mut modes = Object::new();
            for (mode, args) in table {
                modes.insert(mode.to_string(), Value::Array(args.iter().map(|a| Value::from(*a)).collect()));
            }
            permissions.insert(h.kind.into(), Value::Object(modes));
        }
        let (Resume::Flag(word) | Resume::Subcommand(word)) = h.resume;
        resume.insert(h.kind.into(), Value::from(word));
        if let Some(flag) = h.add_dir {
            add_dir.insert(h.kind.into(), Value::from(flag));
        }
        if let Some(log) = h.session_log {
            logs.insert(h.kind.into(), Value::from(log.glob));
        }
    }
    assert_eq!(constants.get("PERMISSION_ARGS"), Some(&Value::Object(permissions)));
    assert_eq!(constants.get("RESUME_ARGS"), Some(&Value::Object(resume)));
    assert_eq!(constants.get("ADD_DIR_ARGS"), Some(&Value::Object(add_dir)));
    assert_eq!(constants.get("SESSION_LOGS"), Some(&Value::Object(logs)));
    // Only codex resumes by a subcommand, and its arguments start with it (launch_args below).
    assert!(HARNESSES.iter().all(|h| matches!(h.resume, Resume::Subcommand(_)) == (h.kind == "codex")));
}

#[test]
fn each_mode_s_arguments_in_each_harness_are_the_python_engine_s() {
    let cases = cases("permission_args");
    assert_eq!(cases.len(), 5 * 7);
    for case in cases {
        let [kind, mode, expected] = &case[..] else { panic!("{case:?}") };
        let (kind, mode) = (kind.as_str().unwrap(), mode.as_str().unwrap());
        let expected = (!expected.is_null()).then(|| strings(expected));
        assert_eq!(harness(kind).unwrap().permission_args(mode), expected, "{kind} {mode}");
    }
}

#[test]
fn the_launch_arguments_of_every_combination_are_the_python_engine_s() {
    let cases = cases("launch_args");
    assert_eq!(cases.len(), 5 * 9 * 3 * 3 * 3);
    for case in cases {
        let [kind, mode, model, session, extra, expected] = &case[..] else { panic!("{case:?}") };
        let h = harness(kind.as_str().unwrap()).unwrap();
        let args = h.launch_args(text(mode), text(model), text(session), text(extra));
        assert_eq!(args, strings(expected), "{case:?}");
    }
}

#[test]
fn launch_args_are_the_same_each_time() {
    // TestHarnessFlags.test_launch_args_leave_the_permission_table_alone: Python's extended its table's list in place.
    for h in &HARNESSES {
        let first = h.launch_args(Some("acceptEdits"), Some("m"), None, Some("/run"));
        assert_eq!(h.launch_args(Some("acceptEdits"), Some("m"), None, Some("/run")), first, "{}", h.kind);
    }
}

#[test]
fn the_run_directory_is_granted_where_the_harness_can() {
    // TestHarnessFlags.test_run_dir_granted_in_worktree_runs_only, its launch_args half; the Workflow half is in
    // workflow/tests/harnesses.rs.
    let d = "/proj/.orchestrator/runs/r";
    let grants: [(&str, &[&str]); 5] = [
        ("claude", &["--add-dir", d]),
        ("codex", &["--add-dir", d]),
        ("gemini", &["--include-directories", d]),
        ("opencode", &[]),
        ("pi", &[]),
    ];
    for (kind, grant) in grants {
        let h = harness(kind).unwrap();
        for (mode, model, session) in [(None, None, None), (Some("acceptEdits"), Some("m"), Some("s1"))] {
            let mut expected = h.launch_args(mode, model, session, None);
            expected.extend(grant.iter().map(|g| g.to_string()));
            assert_eq!(h.launch_args(mode, model, session, Some(d)), expected, "{kind}");
        }
        assert!(h.launch_args(None, None, None, None).is_empty());
    }
}

#[test]
fn an_unknown_harness_is_refused_naming_those_there_are() {
    assert_eq!(
        harness("cursor").unwrap_err().0,
        "agent cursor is not a harness this orchestrator supports; it takes claude, codex, gemini, opencode, pi"
    );
}

#[test]
fn every_session_log_reads_as_the_python_engine_reads_it() {
    let cases = cases("session_usage");
    assert!(cases.len() >= 13);
    for case in cases {
        let [kind, name, log, expected] = &case[..] else { panic!("{case:?}") };
        if name == "times in each form" {
            // Python's fromisoformat forms; the times this engine reads are a_time_is_read_only_with_its_offset's.
            continue;
        }
        let usage = harness(kind.as_str().unwrap()).unwrap().session_usage(&unhex(log.as_str().unwrap()));
        let usage: Vec<Value> =
            usage.into_iter().map(|u| Value::Array(vec![Value::from(u.when), u.tokens.to_json()])).collect();
        assert_eq!(&Value::Array(usage), expected, "{} {}", kind.to_string(), name.to_string());
    }
}

#[test]
fn a_time_is_read_only_with_its_offset() {
    let line = |n: u32, at: &str| {
        format!(
            r#"{{"type": "assistant", "timestamp": "{at}", "message": {{"id": "m{n}", "usage": {{"input_tokens": {n}, "output_tokens": {n}}}}}}}"#
        )
    };
    let times = [
        "2026-10-03T04:00:01.000Z", "2026-10-03T04:00:02.000+00:00", "2026-10-03T04:00:03.000+02:00",
        "2026-10-03T04:00:07.123456789Z", "20261003T040009Z", "2026-10-03T24:00:00Z", "2026-10-03T04:00:13+00",
        "2026-10-03T04:00:14",
    ];
    let log: Vec<String> = times.iter().zip(1..).map(|(at, n)| line(n, at)).collect();
    let usage = harness("claude").unwrap().session_usage((log.join("\n") + "\n").as_bytes());
    let read: Vec<(f64, i64)> = usage.iter().map(|u| (u.when, u.tokens.input)).collect();
    // Hour 24 and a time without an offset are not read; the line is skipped.
    let want = [(1_791_000_001.0, 1), (1_791_000_002.0, 2), (1_790_992_803.0, 3), (1_791_000_007.123456, 4)];
    assert_eq!(read[..4], want);
    assert_eq!(read[4..], [(1_791_000_009.0, 5), (1_791_000_013.0, 7)]);
}

#[test]
fn a_harness_without_a_session_log_reads_none() {
    let log = concat!(
        r#"{"type": "assistant", "timestamp": "2026-10-28T12:00:00Z", "#,
        r#""message": {"id": "a", "usage": {"input_tokens": 1, "output_tokens": 1}}}"#
    )
    .as_bytes();
    assert!(harness("codex").unwrap().session_usage(log).is_empty());
    assert_eq!(harness("claude").unwrap().session_usage(log).len(), 1);
}
