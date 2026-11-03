use super::*;
use crate::test_support::recordings;

/// The state.json texts the e2e recordings hold among their run files.
fn recorded_states() -> Vec<String> {
    let mut texts = Vec::new();
    for (_, recording) in recordings() {
        let files = recording.get("run_files").and_then(Value::as_object).unwrap();
        for (path, text) in files {
            if path.ends_with("state.json") {
                texts.push(text.as_str().unwrap().to_string());
            }
        }
    }
    texts
}

#[test]
fn every_recorded_state_reads_and_writes_back_byte_for_byte() {
    let texts = recorded_states();
    assert!(!texts.is_empty());
    for text in &texts {
        let state = RunState::from_text(text).unwrap_or_else(|e| panic!("{e}: {text}"));
        assert_eq!(state.to_text(), *text);
    }
}

#[test]
fn a_new_state_is_written_pretty_with_its_format_first() {
    let state = RunState::new("20260929-120000-a1b2c3", "add a rate limiter", "/proj");
    let text = state.to_text();
    assert!(
        text.starts_with(
            "{\n  \"format\": 2,\n  \"run_id\": \"20260929-120000-a1b2c3\",\n  \"task\": \"add a rate limiter\",\n  \
             \"cwd\": \"/proj\",\n  \"machine\": null,\n  \"phase\": \"spec\",\n"
        ),
        "{text}"
    );
    assert!(text.ends_with("  \"restored\": [],\n  \"proofs\": []\n}\n"), "{text}");
    assert_eq!(RunState::from_text(&text).unwrap(), state);
}

#[test]
fn fields_it_does_not_know_are_kept_and_written_back_after_its_own() {
    let mut state = RunState::new("20260929-120000-a1b2c3", "t", "/proj");
    state.unknown.insert("later_field".into(), serde_json::json!({"b": [1, 2.5], "a": null}));
    state.unknown.insert("another".into(), Value::from("x"));
    let Value::Object(saved) = state.to_json() else { unreachable!("a state is saved as an object") };
    let keys: Vec<&str> = saved.keys().map(String::as_str).collect();
    assert_eq!(keys[keys.len() - 3..], ["proofs", "later_field", "another"]);
    let back = RunState::from_text(&state.to_text()).unwrap();
    assert_eq!(back, state);
    assert_eq!(back.to_text(), state.to_text());
}

#[test]
fn no_field_it_does_not_know_replaces_one_of_its_own() {
    let mut state = RunState::new("r", "t", "/p");
    state.unknown.insert("phase".into(), Value::from("stale"));
    state.unknown.insert("format".into(), Value::from(9));
    let saved = state.to_json();
    assert_eq!(saved.get("phase"), Some(&Value::from("spec")));
    assert_eq!(saved.get("format"), Some(&Value::from(STATE_FORMAT)));
}

/// The herdr server a CLI reaches with these variables set and no other.
fn server(set: &[(&str, &str)]) -> HerdrServer {
    let vars: Vec<_> = set.iter().map(|(k, v)| (std::ffi::OsString::from(k), std::ffi::OsString::from(v))).collect();
    HerdrServer::of_environment(&vars).unwrap()
}

/// A state of STATE_FORMAT with these fields replaced, or removed for None, as state.json's text.
fn saved_with(fields: &[(&str, Option<&str>)]) -> String {
    let Value::Object(mut saved) = RunState::new("r1", "t", "/p").to_json() else { unreachable!("an object") };
    for (key, value) in fields {
        match value {
            Some(text) => saved.insert(key.to_string(), serde_json::from_str(text).unwrap()),
            None => saved.remove(*key),
        };
    }
    Value::Object(saved).to_string()
}

#[test]
fn the_herdr_server_is_written_after_the_machine_and_only_when_known() {
    let mut state = RunState::new("20260929-120000-a1b2c3", "t", "/proj");
    assert!(!state.to_text().contains("herdr_server"));
    state.herdr_server = Some(server(&[("HERDR_SOCKET_PATH", "/s/herdr.sock")]));
    let text = state.to_text();
    let saved = "\"machine\": null,\n  \"herdr_server\": {\n    \"HERDR_SOCKET_PATH\": \"/s/herdr.sock\",\n    \
                 \"HERDR_SESSION\": null\n  },\n";
    assert!(text.contains(saved), "{text}");
    assert_eq!(RunState::from_text(&text).unwrap(), state);
    // The default session, neither variable set, is a server too, and not the absence of one.
    state.herdr_server = Some(server(&[]));
    assert_eq!(RunState::from_text(&state.to_text()).unwrap().herdr_server, Some(server(&[])));
    // null is none, and a variable left out of the object was unset.
    assert_eq!(RunState::from_text(&saved_with(&[("herdr_server", Some("null"))])).unwrap().herdr_server, None);
    let session = saved_with(&[("herdr_server", Some(r#"{"HERDR_SESSION": "work"}"#))]);
    assert_eq!(RunState::from_text(&session).unwrap().herdr_server, Some(server(&[("HERDR_SESSION", "work")])));
    for value in ["\"work\"", "{\"HERDR_SESSION\": 1}", "1"] {
        let e = RunState::from_text(&saved_with(&[("herdr_server", Some(value))])).unwrap_err();
        assert!(e.0.starts_with("run state r1 is invalid: "), "{value}: {e}");
    }
}

#[test]
fn a_state_without_the_format_is_refused_with_what_to_do_with_its_run() {
    let old = saved_with(&[("format", None)]);
    let e = RunState::from_text(&old).unwrap_err();
    assert_eq!(
        e.0,
        format!(
            "run r1 was saved by an octoflow before {STATE_FORMAT_SINCE}; finish or close it with that release, or \
             remove .orchestrator/runs/r1"
        )
    );
    let other = saved_with(&[("format", Some("3"))]);
    let e = RunState::from_text(&other).unwrap_err();
    assert_eq!(e.0, "run r1 has state format 3, which this octoflow does not read; it reads format 2");
}

#[test]
fn a_state_with_a_field_missing_or_of_the_wrong_type_is_refused_naming_the_run() {
    for (field, value) in [
        ("cwd", None), ("phase", None), ("round", Some("\"1\"")), ("round", Some("-1")), ("phase", Some("1")),
        ("machine", Some("3")), ("conflicts", Some("[1]")), ("agents", Some("{\"a\": 1}")),
        ("pull_request", Some("1")), ("owner", Some("[]")), ("ci", Some("null")), ("turns", Some("[1]")),
        ("locked", Some("{\"s\": [1]}")), ("models", Some("{\"a\": null}")),
    ] {
        let e = RunState::from_text(&saved_with(&[(field, value)])).unwrap_err();
        assert!(e.0.starts_with("run state r1 is invalid: "), "{field}: {e}");
    }
    assert!(RunState::from_text("[]").is_err());
    assert!(RunState::from_text("{").is_err());
}

#[test]
fn the_runs_paths() {
    let mut s = RunState::new("20260929-120000-a1b2c3", "t", "/proj");
    let d = "/proj/.orchestrator/runs/20260929-120000-a1b2c3";
    assert_eq!(s.key(), "a1b2c3");
    assert_eq!(s.dir(), d);
    assert_eq!(s.work_dir(), "/proj");
    assert_eq!(s.refs_prefix(), "refs/orchestrator/a1b2c3/");
    assert_eq!(s.lock_ref("tests"), "refs/orchestrator/a1b2c3/locked/tests");
    assert_eq!(s.proofs_dir(2), format!("{d}/proofs-2"));
    assert_eq!(s.spec_path(), format!("{d}/spec.md"));
    assert_eq!(s.review_path(2), format!("{d}/review-2.md"));
    assert_eq!(s.leaks_path("snapshot"), format!("{d}/leaks-snapshot.json"));
    assert_eq!(s.quality_path(1, 2), format!("{d}/quality-1-2.md"));
    assert_eq!(s.builder_report_path(1, 0), format!("{d}/build-1.md"));
    assert_eq!(s.builder_report_path(1, 2), format!("{d}/build-1-q2.md"));
    assert_eq!(s.last_report_path(1), format!("{d}/build-1.md"));
    s.quality_round = 3;
    assert_eq!(s.last_report_path(1), format!("{d}/build-1-q2.md"));
    assert!(!s.had_quality_gate());
    s.quality_gate_disabled = true;
    assert!(s.had_quality_gate());
    s.worktree = Some("/proj/.orchestrator/worktrees/a1b2c3".into());
    assert_eq!(s.work_dir(), "/proj/.orchestrator/worktrees/a1b2c3");
    assert_eq!(s.prompts().unwrap(), Prompts::default());
    s.prompt_overrides.insert("LATER_NOTE", "x".to_string());
    assert!(s.prompts().unwrap_err().0.contains("replaces prompt LATER_NOTE"));
}
