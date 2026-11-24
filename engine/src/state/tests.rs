use super::*;
use crate::test_support::{denormalized, read_saved, recordings};

/// The state.json texts the e2e recordings hold among their run files, denormalized, as a turn record's seconds must
/// be a number.
fn recorded_states() -> Vec<String> {
    let mut texts = Vec::new();
    for (_, recording) in recordings() {
        let files = recording.get("run_files").and_then(Value::as_object).unwrap();
        for (path, text) in files {
            if path.ends_with("state.json") {
                texts.push(denormalized(text.as_str().unwrap(), 1_791_000_000));
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
fn a_new_state_is_written_on_one_line_with_its_format_first() {
    let state = RunState::new("20260929-120000-a1b2c3", "add a rate limiter", "/proj");
    let text = state.to_text();
    assert!(
        text.starts_with(
            r#"{"format":2,"run_id":"20260929-120000-a1b2c3","task":"add a rate limiter","cwd":"/proj","machine":null,"herdr_server":null,"phase":"spec","#
        ),
        "{text}"
    );
    assert!(text.ends_with("\"restored\":[],\"proofs\":[]}\n"), "{text}");
    assert_eq!(text.matches('\n').count(), 1);
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
fn the_herdr_server_is_written_after_the_machine() {
    let mut state = RunState::new("20260929-120000-a1b2c3", "t", "/proj");
    assert!(state.to_text().contains(r#""machine":null,"herdr_server":null,"#));
    state.herdr_server = Some(server(&[("HERDR_SOCKET_PATH", "/s/herdr.sock")]));
    let text = state.to_text();
    let saved = r#""machine":null,"herdr_server":{"HERDR_SOCKET_PATH":"/s/herdr.sock","HERDR_SESSION":null},"#;
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
             remove .octoflow-ai/runs/r1"
        )
    );
    let other = saved_with(&[("format", Some("4"))]);
    let e = RunState::from_text(&other).unwrap_err();
    assert_eq!(e.0, "run r1 has state format 4, which this octoflow does not read; it reads formats 2 and 3");
}

#[test]
fn an_acp_run_is_saved_in_format_3_and_a_herdr_run_as_before() {
    let mut state = RunState::new("r1", "t", "/proj");
    let herdr = state.to_json();
    assert_eq!(herdr["format"], STATE_FORMAT);
    assert!(herdr.get("backend").is_none(), "{herdr}");
    state.backend = Backend::Acp;
    let acp = state.to_json();
    // A release that reads only format 2 refuses it (format_problem's own check, before this engine's).
    assert_eq!(acp["format"], 3);
    assert_ne!(acp["format"], STATE_FORMAT);
    assert_eq!(acp["backend"], "acp");
    assert_eq!(RunState::from_text(&state.to_text()).unwrap(), state);
}

#[test]
fn a_format_that_is_not_its_backend_s_is_refused() {
    let mismatch = "run r1 has state format {f} with another backend than that format's; state format 2 is a herdr \
                    run's, 3 an ACP run's";
    let acp_in_2 = saved_with(&[("backend", Some("\"acp\""))]);
    assert_eq!(RunState::from_text(&acp_in_2).unwrap_err().0, mismatch.replace("{f}", "2"));
    let herdr_in_3 = saved_with(&[("format", Some("3"))]);
    assert_eq!(RunState::from_text(&herdr_in_3).unwrap_err().0, mismatch.replace("{f}", "3"));
}

#[test]
fn a_missing_option_is_none() {
    let state = RunState::from_text(&saved_with(&[("error", None), ("owner", None), ("herdr_server", None)])).unwrap();
    assert_eq!((state.error, state.owner, state.herdr_server), (None, None, None));
}

#[test]
fn a_state_saved_with_one_verdict_and_no_record_of_it_loads() {
    let old = saved_with(&[("verdicts", None), ("verdict", Some("\"CHANGES_REQUESTED\""))]);
    let state = RunState::from_text(&old).unwrap();
    assert_eq!((state.verdict.as_deref(), state.verdicts.len()), (Some("CHANGES_REQUESTED"), 0));
    assert!(state.unknown.is_empty());
    let e = RunState::from_text(&saved_with(&[("verdicts", Some("[{\"round\": 1}]"))])).unwrap_err();
    assert!(e.0.starts_with("run state r1 is invalid: "), "{e}");
}

#[test]
fn each_verdict_is_recorded_once_per_step_and_round() {
    let mut state = RunState::new("r1", "t", "/p");
    state.verdict = Some("CHANGES_REQUESTED".into());
    state.record_verdict(1, "review", "APPROVE");
    state.record_verdict(1, "accept", "CHANGES_REQUESTED");
    // A resume that took accept's turn of round 1 again.
    state.record_verdict(1, "accept", "CHANGES_REQUESTED");
    state.record_verdict(2, "review", "APPROVE");
    let given: Vec<(u32, &str, &str)> =
        state.verdicts.iter().map(|v| (v.round, v.step.as_str(), v.verdict.as_str())).collect();
    assert_eq!(given, [(1, "review", "APPROVE"), (1, "accept", "CHANGES_REQUESTED"), (2, "review", "APPROVE")]);
    // The run's verdict is the caller's to set.
    assert_eq!(state.verdict.as_deref(), Some("CHANGES_REQUESTED"));
    let text = state.to_text();
    assert!(
        text.contains(r#""verdict":"CHANGES_REQUESTED","verdicts":[{"round":1,"step":"review","verdict":"APPROVE"},"#),
        "{text}"
    );
    assert_eq!(RunState::from_text(&text).unwrap(), state);
}

/// A turn record of every field, as Workflow::turn_record saves one, with these fields replaced, or removed for None.
fn turn_with(fields: &[(&str, Option<&str>)]) -> String {
    let mut turn: Object = serde_json::from_str(
        r#"{"step": "build", "role": "build", "round": 1, "quality_round": 0, "file": "build-1.md", "agent": "codex",
            "model": null, "unoffered_model": null, "session": "s1", "tokens": null, "human_paced": false,
            "started_at": "2026-10-03T04:00:00+00:00", "ended_at": null, "seconds": null}"#,
    )
    .unwrap();
    for (key, value) in fields {
        match value {
            Some(text) => turn.insert(key.to_string(), serde_json::from_str(text).unwrap()),
            None => turn.remove(*key),
        };
    }
    format!("[{}]", Value::Object(turn))
}

#[test]
fn a_turn_record_saved_before_waits_were_kept_has_none() {
    let text = saved_with(&[("turns", Some(&turn_with(&[("waited_seconds", None)])))]);

    let state = RunState::from_text(&text).unwrap();

    assert_eq!(state.turns[0].waited_seconds, None);
    assert_eq!((&state.turns[0].outside_wait_seconds, &state.turns[0].waiting_on), (&None, &None));
    assert_eq!(state.to_text(), text + "\n");
}

#[test]
fn a_standing_wait_and_the_seconds_of_those_ended_are_saved_as_read() {
    let wait = r#"{"on": "the merge of pull request #526", "since": "2026-10-03T04:01:00+00:00"}"#;
    let turns = turn_with(&[("outside_wait_seconds", Some("30")), ("waiting_on", Some(wait))]);
    let text = saved_with(&[("turns", Some(&turns))]);

    let state = RunState::from_text(&text).unwrap();

    let since = "2026-10-03T04:01:00+00:00".to_string();
    let standing = StandingWait { on: "the merge of pull request #526".into(), since };
    assert_eq!((state.turns[0].outside_wait_seconds, &state.turns[0].waiting_on), (Some(30), &Some(standing)));
    assert_eq!(state.to_text(), text + "\n");
}

/// turn_with's record as read.
fn build_1_under_way() -> TurnRecord {
    RunState::from_text(&saved_with(&[("turns", Some(&turn_with(&[])))])).unwrap().turns.remove(0)
}

#[test]
fn ending_a_wait_adds_its_seconds_to_those_of_the_waits_ended_before() {
    let at = |s: i64| crate::summary::iso_utc(1_791_000_000 + s);
    let mut turn = build_1_under_way();
    assert_eq!(turn.end_wait(1_791_000_100), Ok(false));
    assert_eq!(turn.outside_wait_seconds, None);

    turn.waiting_on = Some(StandingWait { on: "x".into(), since: at(10) });
    assert_eq!(turn.end_wait(1_791_000_040), Ok(true));
    turn.waiting_on = Some(StandingWait { on: "y".into(), since: at(100) });
    assert_eq!(turn.end_wait(1_791_000_105), Ok(true));
    // A wait found after the turn's end, as its file's mtime gives it, counts nothing.
    turn.waiting_on = Some(StandingWait { on: "z".into(), since: at(200) });
    assert_eq!(turn.end_wait(1_791_000_150), Ok(true));

    assert_eq!((turn.outside_wait_seconds, turn.waiting_on), (Some(35), None));
}

#[test]
fn a_wait_that_started_at_no_time_stays_standing() {
    let mut turn = build_1_under_way();
    turn.waiting_on = Some(StandingWait { on: "x".into(), since: "soon".into() });

    let err = turn.end_wait(1_791_000_000).unwrap_err();

    assert!(err.0.starts_with("the wait of the turn that writes build-1.md started at no time: "), "{err:?}");
    assert!(turn.waiting_on.is_some());
}

#[test]
fn agent_and_turn_records_keep_the_fields_they_do_not_know_after_their_own() {
    let agents = r#"{"build": {"name": "build-r1", "pane": "p2", "kind": "pi", "later": 1}}"#;
    let turns = turn_with(&[("later", Some("[2]"))]);
    let text = saved_with(&[("agents", Some(agents)), ("turns", Some(&turns))]);
    let state = RunState::from_text(&text).unwrap();
    let (agent, turn) = (state.agents.get("build").unwrap(), &state.turns[0]);
    assert_eq!((agent.harness.as_str(), agent.session.as_deref()), ("pi", None));
    assert_eq!((turn.harness.as_deref(), turn.session.as_deref(), turn.under_way()), (Some("codex"), Some("s1"), true));
    assert_eq!(state.to_text(), text + "\n");
}

#[test]
fn an_agents_start_is_kept_until_it_is_prompted_and_reads_as_none_where_an_older_engine_saved_none() {
    let agents =
        r#"{"build": {"name": "build-r1", "pane": "p2", "kind": "pi", "session": "s1", "unprompted": "resumed"}}"#;
    let text = saved_with(&[("agents", Some(agents))]);
    let state = RunState::from_text(&text).unwrap();
    assert_eq!(state.agents.get("build").unwrap().unprompted, Some(AgentStart::Resumed));
    assert_eq!(state.to_text(), text + "\n");
    let older = saved_with(&[("agents", Some(r#"{"build": {"name": "build-r1", "pane": "p2", "kind": "pi"}}"#))]);
    assert_eq!(RunState::from_text(&older).unwrap().agents.get("build").unwrap().unprompted, None);
}

#[test]
fn an_agent_past_its_startup_is_saved_so_and_reads_as_not_where_an_older_engine_saved_nothing() {
    let past =
        r#"{"build": {"name": "build-r1", "pane": "p2", "kind": "pi", "unprompted": "new", "past_startup": true}}"#;
    let text = saved_with(&[("agents", Some(past))]);
    let state = RunState::from_text(&text).unwrap();
    assert!(state.agents.get("build").unwrap().past_startup);
    assert_eq!(state.to_text(), text + "\n");
    let older = r#"{"build": {"name": "build-r1", "pane": "p2", "kind": "pi", "unprompted": "new"}}"#;
    let older = saved_with(&[("agents", Some(older))]);
    let state = RunState::from_text(&older).unwrap();
    assert!(!state.agents.get("build").unwrap().past_startup);
    assert_eq!(state.to_text(), older + "\n", "false is not saved");
}

#[test]
fn an_agent_or_turn_record_without_a_field_its_engine_writes_is_refused() {
    let agentless =
        [r#"{"build": {"name": "build-r1", "pane": "p2"}}"#, r#"{"build": {"name": "b", "kind": 1, "pane": ""}}"#];
    for agents in agentless {
        let e = RunState::from_text(&saved_with(&[("agents", Some(agents))])).unwrap_err();
        assert!(e.0.starts_with("run state r1 is invalid: "), "{agents}: {e}");
    }
    let keys = ["step", "role", "round", "quality_round", "file", "agent", "model", "session", "tokens", "human_paced"];
    for key in keys.into_iter().chain(["started_at", "ended_at", "seconds"]) {
        let e = RunState::from_text(&saved_with(&[("turns", Some(&turn_with(&[(key, None)])))])).unwrap_err();
        assert_eq!(e.0, format!("run state r1 is invalid: missing field `{key}`"));
    }
}

#[test]
fn a_model_not_offered_is_kept_by_the_agent_and_turn_records_and_reads_as_none_where_an_older_engine_saved_none() {
    let agents = r#"{"build": {"name": "build-r1", "pane": "p2", "kind": "claude",
        "unoffered_model": {"asked": "haiku", "taken": null}}}"#;
    let turns = turn_with(&[("unoffered_model", Some(r#""haiku""#))]);
    let text = saved_with(&[("agents", Some(agents)), ("turns", Some(&turns))]);
    let state = RunState::from_text(&text).unwrap();
    let unoffered = UnofferedModel { asked: "haiku".into(), taken: None };
    assert_eq!(state.agents["build"].unoffered_model, Some(unoffered));
    assert_eq!(state.turns[0].unoffered_model.as_deref(), Some("haiku"));
    assert_eq!(RunState::from_text(&state.to_text()).unwrap(), state);

    let older = saved_with(&[("turns", Some(&turn_with(&[("unoffered_model", None)])))]);
    assert_eq!(RunState::from_text(&older).unwrap().turns[0].unoffered_model, None);
}

#[test]
fn an_empty_harness_name_is_refused() {
    // Read, it would pass for no harness: a quality analysis's, whose tokens are never counted.
    let agents = r#"{"build": {"name": "build-r1", "pane": "p2", "kind": ""}}"#;
    let turns = turn_with(&[("agent", Some(r#""""#))]);
    for (key, value) in [("agents", agents), ("turns", turns.as_str())] {
        let e = RunState::from_text(&saved_with(&[(key, Some(value))])).unwrap_err();
        assert!(e.0.contains("an empty harness name"), "{key}: {e}");
    }
    let quality = turn_with(&[("agent", Some("null"))]);
    assert_eq!(RunState::from_text(&saved_with(&[("turns", Some(&quality))])).unwrap().turns[0].harness, None);
}

#[test]
fn a_roles_harness_is_its_agents_then_its_next_agents_then_the_default() {
    let mut state = RunState::new("r1", "t", "/p");
    state.next_harnesses.insert("build".to_string(), "codex".to_string());
    state.next_harnesses.insert("review".to_string(), "pi".to_string());
    state.agents.insert("build".to_string(), AgentRecord::new("build-r1", "p2", "agy"));
    // build's agent runs in agy though its next one starts in codex: a resume's --role-agent changed it.
    for (role, harness) in [("build", "agy"), ("review", "pi"), ("spec", DEFAULT_HARNESS)] {
        assert_eq!(state.harness(role), harness, "{role}");
    }
}

#[test]
fn a_state_with_a_field_missing_or_of_the_wrong_type_is_refused_naming_the_run() {
    for (field, value) in [
        ("cwd", None),
        ("phase", None),
        ("round", Some("\"1\"")),
        ("round", Some("-1")),
        ("phase", Some("1")),
        ("machine", Some("3")),
        ("conflicts", Some("[1]")),
        ("agents", Some("{\"a\": 1}")),
        ("pull_request", Some("1")),
        ("owner", Some("[]")),
        ("ci", Some("null")),
        ("turns", Some("[1]")),
        ("locked", Some("{\"s\": [1]}")),
        ("models", Some("{\"a\": null}")),
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
    let d = "/proj/.octoflow-ai/runs/20260929-120000-a1b2c3";
    assert_eq!(s.key(), "a1b2c3");
    assert_eq!(s.dir(), d);
    assert_eq!(s.work_dir(), "/proj");
    assert_eq!(refs_prefix(&s.run_id), "refs/orchestrator/a1b2c3/");
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
    s.quality_gate_disabled = false;
    // An empty name is no gate, though its files may be there.
    s.quality_job = Some(String::new());
    assert!(!s.has_quality_gate() && s.had_quality_gate());
    s.quality_job = Some("local".into());
    assert!(s.has_quality_gate());
    s.quality_job = None;
    s.worktree = Some("/proj/.octoflow-ai/worktrees/a1b2c3".into());
    assert_eq!(s.work_dir(), "/proj/.octoflow-ai/worktrees/a1b2c3");
    assert_eq!(s.prompts().unwrap(), Prompts::default());
    s.prompt_overrides.insert("LATER_NOTE".to_string(), "x".to_string());
    assert!(s.prompts().unwrap_err().0.contains("replaces prompt LATER_NOTE"));
}

/// saved_with's state.json as the object it reads back as.
fn saved_object(fields: &[(&str, Option<&str>)]) -> Value {
    serde_json::from_str(&saved_with(fields)).unwrap()
}

#[test]
fn a_saved_run_of_this_format_is_its_state() {
    let run = read_saved(40, &saved_object(&[("phase", Some("\"build\""))]));
    assert_eq!((run.age(), run.problem()), (40, None));
    assert_eq!(run.state().unwrap().phase, "build");
    assert_eq!(run.shown(), run.state().unwrap());
}

#[test]
fn an_unreadable_saved_run_is_refused_but_shown_as_far_as_it_reads() {
    // (state.json, the problem's start, what shown keeps)
    let cases = [
        (
            saved_object(&[("format", None), ("phase", Some("\"build\""))]),
            "run r1 was saved by an octoflow before",
            "build",
        ),
        (saved_object(&[("format", Some("3")), ("phase", Some("\"build\""))]), "run r1 has state format 3", "build"),
        (saved_object(&[("round", Some("\"2\"")), ("phase", Some("\"build\""))]), "run state r1 is invalid: ", "build"),
    ];
    for (saved, problem, phase) in cases {
        let run = read_saved(7, &saved);
        assert!(run.problem().is_some_and(|p| p.starts_with(problem)), "{saved}: {:?}", run.problem());
        assert_eq!(run.state().unwrap_err().0, run.problem().unwrap());
        assert_eq!((run.age(), run.shown().phase.as_str()), (7, phase), "{saved}");
    }
}

#[test]
fn a_saved_run_that_is_not_json_or_names_no_run_is_unreadable_and_named_by_its_directory() {
    let dir = "/p/.octoflow-ai/runs/r9";
    let not_named = format!("{dir}/state.json is not a JSON object with a run_id; fix it, or remove {dir}");
    let cases = [
        (
            "{\"run_id\": ".to_string(),
            format!(
                "{dir}/state.json is not JSON (EOF while parsing a value at line 1 column 11); fix it, or remove {dir}"
            ),
            "",
        ),
        ("\"a run\"".to_string(), not_named.clone(), ""),
        (saved_with(&[("run_id", None), ("phase", Some("\"build\""))]), not_named.clone(), "build"),
        (saved_with(&[("run_id", Some("5"))]), not_named.clone(), "spec"),
        (saved_with(&[("run_id", Some("\"\""))]), not_named, "spec"),
    ];
    for (text, problem, phase) in cases {
        let run = SavedRun::read(3, dir, text.as_bytes());
        assert_eq!(run.problem(), Some(problem.as_str()), "{text}");
        assert_eq!(run.state().unwrap_err().0, problem, "{text}");
        assert_eq!((run.age(), run.shown().run_id.as_str()), (3, "r9"), "{text}");
        if !phase.is_empty() {
            assert_eq!(run.shown().phase, phase, "{text}");
        }
    }
}

#[test]
fn an_unreadable_field_is_shown_at_its_default_and_the_others_as_saved() {
    let saved = saved_object(&[
        ("format", None),
        ("round", Some("\"2\"")),
        ("task", Some("\"add a rate limiter\"")),
        ("later", Some("1")),
        ("owner", Some("{\"host\": \"here\", \"pid\": 1}")),
    ]);
    let shown = read_saved(0, &saved).shown().clone();
    assert_eq!((shown.round, shown.task.as_str(), shown.run_id.as_str()), (0, "add a rate limiter", "r1"));
    assert_eq!(shown.owner.map(|o| o.len()), Some(2));
    assert_eq!(shown.unknown, crate::json::object([("later", Value::from(1))]));
}

#[test]
fn done_is_the_done_phase() {
    let mut state = RunState::new("r1", "t", "/p");
    for (phase, done) in [("spec", false), ("build", false), ("quality", false), ("done", true)] {
        state.phase = phase.into();
        assert_eq!(state.done(), done, "{phase}");
    }
}

#[test]
fn in_quality_is_the_quality_phase_or_the_gated_step() {
    let (impl_, quick) = (crate::test_support::impl_(), crate::test_support::quick());
    let mut state = RunState::new("r1", "t", "/p");
    // (phase, pipeline, in quality)
    for (phase, pipeline, quality) in [
        ("quality", &quick, true),
        ("impl", &impl_, true),
        ("review", &impl_, false),
        ("build", &quick, false),
        ("impl", &quick, false),
    ] {
        state.phase = phase.into();
        assert_eq!(state.in_quality(pipeline), quality, "{phase} of {}", pipeline.name());
    }
}

#[test]
fn a_state_names_its_workflow() {
    let mut state = RunState::new("r1", "t", "/p");
    assert_eq!(state.pipeline().unwrap(), *crate::pipeline::DEFAULT_WORKFLOW);
    // A workflow that is not built in is read from its saved definition, never from its file.
    state.workflow = "nosuch".into();
    assert!(state.pipeline().is_err());
    // Even one of a built-in's name, as a run of a workflow file of that name saved it before the name was built in.
    let quick = crate::test_support::quick();
    state.workflow = "quick".into();
    state.workflow_definition = Some(crate::workflow_file::workflow_definition(&quick));
    assert_eq!(state.pipeline().unwrap(), quick);
    // Worded by its saved prompts, which must name prompts this engine has.
    let mut state = RunState::new("r1", "t", "/p");
    state.prompt_overrides.insert("LATER_NOTE".to_string(), "x".into());
    assert!(state.pipeline().is_err());
    let fix = "Fix {findings}; report to {build_path}.";
    state.prompt_overrides = [("FIX_PROMPT".to_string(), fix.to_string())].into_iter().collect();
    assert_eq!(state.pipeline().unwrap().step("build").unwrap().again.as_deref(), Some(fix));
}

// -- Finding a run ---------------------------------------------------------------------------------------------------

fn runs() -> Vec<SavedRun> {
    ["20260930-070000-c0ffee", "20260930-080000-beef00", "20260929-080000-beef00"]
        .iter()
        .zip(1..)
        .map(|(id, age)| read_saved(age, &RunState::new(id, "a task", "/p").to_json()))
        .collect()
}

#[test]
fn by_key_or_full_id() {
    assert_eq!(find_run(&runs(), "c0ffee", "/p").unwrap().age(), 1);
    assert_eq!(find_run(&runs(), "20260929-080000-beef00", "/p").unwrap().age(), 3);
    // Unreadable or not.
    let old = read_saved(4, &Value::Object(crate::json::object([("run_id", Value::from("20260928-080000-0dd000"))])));
    assert_eq!(find_run(&[old], "0dd000", "/p").unwrap().age(), 4);
}

#[test]
fn ambiguous_key() {
    let e = find_run(&runs(), "beef00", "/p").unwrap_err();
    assert_eq!(
        e.0,
        "beef00 matches several runs (20260930-080000-beef00, 20260929-080000-beef00); give the full run id"
    );
}

#[test]
fn no_match() {
    assert_eq!(find_run(&runs(), "abcdef", "/p").unwrap_err().0, "no run abcdef under /p/.octoflow-ai/runs");
}
