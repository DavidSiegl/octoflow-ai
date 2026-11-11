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
             remove .orchestrator/runs/r1"
        )
    );
    let other = saved_with(&[("format", Some("3"))]);
    let e = RunState::from_text(&other).unwrap_err();
    assert_eq!(e.0, "run r1 has state format 3, which this octoflow does not read; it reads format 2");
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
    assert!(s.may_have_ci_refs());
    s.quality_job = Some("local".into());
    assert!(!s.may_have_ci_refs());
    // A build in ci was of a Jenkins job named `local`, saved before `local` named the local gate.
    s.ci.insert("ref".to_string(), Value::from("refs/orchestrator/a1b2c3/1-1"));
    assert!(s.may_have_ci_refs());
    s.ci = Object::new();
    s.worktree = Some("/proj/.orchestrator/worktrees/a1b2c3".into());
    assert_eq!(s.work_dir(), "/proj/.orchestrator/worktrees/a1b2c3");
    assert_eq!(s.prompts().unwrap(), Prompts::default());
    s.prompt_overrides.insert("LATER_NOTE", "x".to_string());
    assert!(s.prompts().unwrap_err().0.contains("replaces prompt LATER_NOTE"));
}

/// saved_with's state.json as the object it reads back as.
fn saved_object(fields: &[(&str, Option<&str>)]) -> Value {
    serde_json::from_str(&saved_with(fields)).unwrap()
}

#[test]
fn a_saved_run_of_this_format_is_its_state() {
    let run = SavedRun::read(40, saved_object(&[("phase", Some("\"build\""))]));
    assert_eq!((run.age(), run.problem()), (40, None));
    assert_eq!(run.state().unwrap().phase, "build");
    assert_eq!(run.shown(), run.state().unwrap());
}

#[test]
fn an_unreadable_saved_run_is_refused_but_shown_as_far_as_it_reads() {
    // (state.json, the problem's start, what shown keeps)
    let cases = [
        (saved_object(&[("format", None), ("phase", Some("\"build\""))]), "run r1 was saved by an octoflow before", "build"),
        (saved_object(&[("format", Some("3")), ("phase", Some("\"build\""))]), "run r1 has state format 3", "build"),
        (saved_object(&[("round", Some("\"2\"")), ("phase", Some("\"build\""))]), "run state r1 is invalid: ", "build"),
        (Value::from("a run"), "a run state is a JSON object", "spec"),
    ];
    for (saved, problem, phase) in cases {
        let run = SavedRun::read(7, saved.clone());
        assert!(run.problem().is_some_and(|p| p.starts_with(problem)), "{saved}: {:?}", run.problem());
        assert_eq!(run.state().unwrap_err().0, run.problem().unwrap());
        assert_eq!((run.age(), run.shown().phase.as_str()), (7, phase), "{saved}");
    }
}

#[test]
fn an_unreadable_field_is_shown_at_its_default_and_the_others_as_saved() {
    let saved = saved_object(&[
        ("format", None), ("round", Some("\"2\"")), ("task", Some("\"add a rate limiter\"")), ("later", Some("1")),
        ("owner", Some("{\"host\": \"here\", \"pid\": 1}")),
    ]);
    let shown = SavedRun::read(0, saved).shown().clone();
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
        ("quality", &quick, true), ("impl", &impl_, true), ("review", &impl_, false), ("build", &quick, false),
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
    let quick = crate::test_support::quick();
    state.workflow = "quick".into();
    assert!(state.pipeline().is_err());
    state.workflow_definition = Some(crate::workflow_file::workflow_definition(&quick));
    assert_eq!(state.pipeline().unwrap(), quick);
    // Worded by its saved prompts, which must name prompts this engine has.
    let mut state = RunState::new("r1", "t", "/p");
    state.prompt_overrides.insert("LATER_NOTE", "x".into());
    assert!(state.pipeline().is_err());
    let fix = "Fix {findings}; report to {build_path}.";
    state.prompt_overrides = [("FIX_PROMPT", fix.to_string())].into_iter().collect();
    assert_eq!(state.pipeline().unwrap().step("build").unwrap().again.as_deref(), Some(fix));
}

// -- TestFindRun -----------------------------------------------------------------------------------------------------

fn runs() -> Vec<SavedRun> {
    ["20260930-070000-c0ffee", "20260930-080000-beef00", "20260929-080000-beef00"]
        .iter()
        .zip(1..)
        .map(|(id, age)| SavedRun::read(age, RunState::new(id, "a task", "/p").to_json()))
        .collect()
}

#[test]
fn by_key_or_full_id() {
    assert_eq!(find_run(&runs(), "c0ffee", "/p").unwrap().age(), 1);
    assert_eq!(find_run(&runs(), "20260929-080000-beef00", "/p").unwrap().age(), 3);
    // Unreadable or not.
    let old = SavedRun::read(4, Value::Object(crate::json::object([("run_id", Value::from("20260928-080000-0dd000"))])));
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
    assert_eq!(find_run(&runs(), "abcdef", "/p").unwrap_err().0, "no run abcdef under /p/.orchestrator/runs");
}
