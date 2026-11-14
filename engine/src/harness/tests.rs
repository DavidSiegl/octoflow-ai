//! The harnesses' launch arguments, held to harness.json's tables (engine/tests/oracle/), which hold every harness,
//! permission mode, model, session and directory, and the token usage of sample session logs.

use super::*;
use crate::constants::DEFAULT_HARNESS;
use crate::test_support::{oracle, unhex};
use crate::json::Object;

/// The harness harness.json has no cases of: agy, which replaced gemini (GitHub issue #125). Each comparison with
/// harness.json leaves it out, and DROPPED; agy_takes_ticket_01_s_flags tests agy.
const UNRECORDED: &str = "agy";
/// The harness harness.json records and this engine no longer offers, as UNRECORDED replaced it.
const DROPPED: &str = "gemini";

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
fn every_harness_s_tables_are_harness_json_s() {
    let recorded = oracle("harness.json");
    let constants = recorded.get("constants").unwrap();
    let kinds: Vec<&str> = HARNESSES.iter().map(|h| h.kind).collect();
    assert_eq!(kinds, HARNESS_NAMES);
    let mut recorded_kinds = strings(constants.get("AGENT_KINDS").unwrap());
    recorded_kinds.retain(|k| k != DROPPED);
    assert_eq!(recorded_kinds, kinds.iter().filter(|k| **k != UNRECORDED).copied().collect::<Vec<_>>());
    assert_eq!(constants.get("DEFAULT_AGENT").and_then(Value::as_str), Some(DEFAULT_HARNESS));
    // The four tables harness.json records, as the rows here make them.
    let (mut permissions, mut resume, mut add_dir, mut logs) = (Object::new(), Object::new(), Object::new(), Object::new());
    for h in HARNESSES.iter().filter(|h| h.kind != UNRECORDED) {
        if let Permissions::Equivalents(table) = h.permissions {
            let mut modes = Object::new();
            for (mode, args) in table {
                modes.insert(mode.to_string(), Value::Array(args.iter().map(|a| Value::from(*a)).collect()));
            }
            permissions.insert(h.kind.into(), Value::Object(modes));
        }
        let (Resume::Flag(word) | Resume::Subcommand(word)) = h.resume;
        resume.insert(h.kind.into(), Value::from(word));
        if let Config::Flags { add_dir: Some(flag) } = h.config {
            add_dir.insert(h.kind.into(), Value::from(flag));
        }
        if let Some(log) = h.session_log {
            logs.insert(h.kind.into(), Value::from(log.glob));
        }
    }
    // harness.json's table of the name, without its DROPPED row.
    let table_of = |name: &str| {
        let mut table = constants.get(name).and_then(Value::as_object).cloned().unwrap();
        table.remove(DROPPED);
        Value::Object(table)
    };
    assert_eq!(table_of("PERMISSION_ARGS"), Value::Object(permissions));
    assert_eq!(table_of("RESUME_ARGS"), Value::Object(resume));
    assert_eq!(table_of("ADD_DIR_ARGS"), Value::Object(add_dir));
    assert_eq!(table_of("SESSION_LOGS"), Value::Object(logs));
    // Only codex resumes by a subcommand, and its arguments start with it (launch below).
    assert!(HARNESSES.iter().all(|h| matches!(h.resume, Resume::Subcommand(_)) == (h.kind == "codex")));
}

#[test]
fn each_mode_s_arguments_in_each_harness_are_harness_json_s() {
    let cases = cases("permission_args");
    assert_eq!(cases.len(), 5 * 7);
    for case in cases.iter().filter(|case| case[0] != DROPPED) {
        let [kind, mode, expected] = &case[..] else { panic!("{case:?}") };
        let (kind, mode) = (kind.as_str().unwrap(), mode.as_str().unwrap());
        let expected = (!expected.is_null()).then(|| strings(expected));
        assert_eq!(harness(kind).unwrap().permission_args(mode), expected, "{kind} {mode}");
    }
}

#[test]
fn the_launch_of_every_combination_is_harness_json_s() {
    // opencode's is left out (opencode_takes_its_model_and_grant_as_config): harness.json passes it --model, which
    // opencode 2.0.24's TUI does not take.
    let cases = cases("launch_args");
    assert_eq!(cases.len(), 5 * 9 * 3 * 3 * 3);
    for case in cases.iter().filter(|case| case[0] != "opencode" && case[0] != DROPPED) {
        let [kind, mode, model, session, extra, expected] = &case[..] else { panic!("{case:?}") };
        let h = harness(kind.as_str().unwrap()).unwrap();
        let launch = h.launch(text(mode), text(model), text(session), text(extra));
        assert_eq!(launch, Launch { args: strings(expected), env: vec![] }, "{case:?}");
    }
}

#[test]
fn opencode_takes_its_model_and_grant_as_config() {
    let h = harness("opencode").unwrap();
    let d = "/proj/.orchestrator/runs/r";
    // A launch with args, its config exported as the JSON json when given one.
    let launch = |args: &[&str], json: Option<&str>| Launch {
        args: args.iter().map(|a| a.to_string()).collect(),
        env: json.map(|j| ("OPENCODE_CONFIG_CONTENT".to_string(), j.to_string())).into_iter().collect(),
    };
    let grant = r#""permission":{"external_directory":{"/proj/.orchestrator/runs/r/*":"allow"}}"#;
    let cases = [
        (None, None, None, launch(&[], None)),
        (Some(""), Some(""), Some(""), launch(&[], None)),
        // No `\\` in the value, which fish would read as an escape.
        (Some(r#"a\b"c"#), None, None, launch(&["--standalone"], Some(r#"{"model":"a\u005cb\"c"}"#))),
        (None, Some("s-1"), None, launch(&["--session", "s-1"], None)),
        (Some("big-pickle"), None, None, launch(&["--standalone"], Some(r#"{"model":"big-pickle"}"#))),
        (None, None, Some(d), launch(&["--standalone"], Some(&format!("{{{grant}}}")))),
        (
            Some("m"),
            Some("s-1"),
            Some(d),
            launch(&["--session", "s-1", "--standalone"], Some(&format!(r#"{{"model":"m",{grant}}}"#))),
        ),
    ];
    for (model, session, extra, expected) in cases {
        // opencode has no equivalent of any permission mode, so none changes its launch.
        for mode in [None, Some("acceptEdits"), Some("bypassPermissions")] {
            assert_eq!(h.launch(mode, model, session, extra), expected, "{mode:?} {model:?} {session:?} {extra:?}");
        }
    }
}

#[test]
fn opencode_grants_no_run_directory_its_matcher_would_read_as_more_than_itself() {
    // opencode 2.0.24 reads `*` and `?` in a grant as wildcards, and `\` as `/`, so each would widen or move it.
    let h = harness("opencode").unwrap();
    for dir in ["/p*/.orchestrator/runs/r", "/p?/.orchestrator/runs/r", r"/p\q/.orchestrator/runs/r"] {
        let expected = Launch {
            args: vec!["--standalone".to_string()],
            env: vec![("OPENCODE_CONFIG_CONTENT".to_string(), r#"{"model":"m"}"#.to_string())],
        };
        assert_eq!(h.launch(None, Some("m"), None, Some(dir)), expected, "{dir}");
    }
    // Other characters it takes as they are.
    let launch = h.launch(None, None, None, Some("/p[1]{a,b}+/r"));
    assert_eq!(launch.env[0].1, r#"{"permission":{"external_directory":{"/p[1]{a,b}+/r/*":"allow"}}}"#);
}

#[test]
fn the_launch_is_the_same_each_time() {
    // A launch must not change the table it reads its arguments from.
    for h in &HARNESSES {
        let first = h.launch(Some("acceptEdits"), Some("m"), None, Some("/run"));
        assert_eq!(h.launch(Some("acceptEdits"), Some("m"), None, Some("/run")), first, "{}", h.kind);
    }
}

#[test]
fn the_run_directory_is_granted_where_the_harness_can() {
    // The launch half, for the harnesses that take a flag; the Workflow half is in workflow/tests/harnesses.rs, and
    // opencode's grant in
    // opencode_takes_its_model_and_grant_as_config.
    let d = "/proj/.orchestrator/runs/r";
    let grants: [(&str, &[&str]); 4] = [
        ("agy", &["--add-dir", d]),
        ("claude", &["--add-dir", d]),
        ("codex", &["--add-dir", d]),
        ("pi", &[]),
    ];
    for (kind, grant) in grants {
        let h = harness(kind).unwrap();
        for (mode, model, session) in [(None, None, None), (Some("acceptEdits"), Some("m"), Some("s1"))] {
            let mut expected = h.launch(mode, model, session, None);
            expected.args.extend(grant.iter().map(|g| g.to_string()));
            assert_eq!(h.launch(mode, model, session, Some(d)), expected, "{kind}");
        }
        assert_eq!(h.launch(None, None, None, None), Launch { args: vec![], env: vec![] });
    }
}

#[test]
fn an_unknown_harness_is_refused_naming_those_there_are() {
    assert_eq!(
        harness("cursor").unwrap_err().0,
        "agent cursor is not a harness this orchestrator supports; it takes agy, claude, codex, opencode, pi"
    );
}

#[test]
fn gemini_is_refused_naming_agy_which_replaced_it() {
    assert_eq!(
        harness("gemini").unwrap_err().0,
        "agent gemini is not a harness this orchestrator supports any more; agy replaced it"
    );
}

#[test]
fn agy_takes_ticket_01_s_flags() {
    // The flags agy 1.5.5's --help gives, tried together by hand (GitHub issue #124).
    let h = harness("agy").unwrap();
    let args = |mode| h.launch(mode, Some("m"), Some("c-1"), Some("/run")).args;
    let rest = ["--conversation", "c-1", "--model", "m", "--add-dir", "/run"];
    let cases: [(Option<&str>, &[&str]); 6] = [
        (None, &[]),
        (Some("default"), &[]),
        (Some("acceptEdits"), &["--mode", "accept-edits"]),
        (Some("bypassPermissions"), &["--dangerously-skip-permissions"]),
        (Some("plan"), &["--mode", "plan"]),
        // A mode agy has no equivalent of is not passed.
        (Some("auto"), &[]),
    ];
    for (mode, permission) in cases {
        let expected: Vec<String> = permission.iter().chain(&rest).map(|a| a.to_string()).collect();
        assert_eq!(args(mode), expected, "{mode:?}");
    }
    assert_eq!(h.launch(None, None, None, None), Launch { args: vec![], env: vec![] });
    assert_eq!(h.session_log, None);
}

#[test]
fn every_session_log_reads_as_harness_json_records() {
    let cases = cases("session_usage");
    assert!(cases.len() >= 13);
    for case in cases {
        let [kind, name, log, expected] = &case[..] else { panic!("{case:?}") };
        if name == "times in each form" {
            // Forms of a time this engine does not all read; the ones it reads are
            // a_time_is_read_only_with_its_offset's.
            continue;
        }
        let usage = harness(kind.as_str().unwrap()).unwrap().session_usage(&unhex(log.as_str().unwrap()));
        let usage: Vec<Value> =
            usage.into_iter().map(|u| Value::Array(vec![Value::from(u.when), u.tokens.to_json()])).collect();
        assert_eq!(&Value::Array(usage), expected, "{} {}", kind, name);
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
