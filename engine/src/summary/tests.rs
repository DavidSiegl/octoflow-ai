use super::*;
use crate::pipeline::DEFAULT_WORKFLOW;
use crate::test_support::{helper_cases, oracle, unhex, GOLDEN_SUMMARY};
use crate::json::object;
use crate::state::AgentRecord;

#[test]
fn every_recorded_run_is_summed_up_as_the_python_engine_does() {
    let cases = helper_cases();
    assert!(cases.len() >= 40, "helpers.json holds {} runs", cases.len());
    for case in cases {
        let expected = |key: &str| case.expected.get(key).and_then(Value::as_str).map(String::from);
        let place = format!("{} {}", expected("recording").unwrap(), expected("run_dir").unwrap());
        let pipeline = case.state.pipeline().ok();
        let rounds: Vec<String> = (1..=case.state.round).map(|k| format!("Round {k}: as recorded")).collect();
        assert_eq!(pipeline.map(|p| run_summary(&case.state, &p, &rounds, case.now)), expected("run_summary"), "{place}");
        // The keys and values the Python engine wrote, its spacing aside.
        let recorded = expected("run_metrics").map(|text| serde_json::from_str::<Value>(&text).unwrap());
        assert_eq!(Some(run_metrics(&case.state, case.now)), recorded, "{place}");
        let lines = Value::Array(turn_lines(&case.state.turns, &jiff::tz::TimeZone::UTC).into_iter().map(Value::from).collect());
        assert_eq!(Some(&lines), case.expected.get("turn_lines"), "{place}");
    }
}

#[test]
fn durations_and_ages() {
    for (seconds, text) in [(0, "0s"), (59, "59s"), (65, "1m 05s"), (3599, "59m 59s"), (3725, "1h 02m")] {
        assert_eq!(format_duration(seconds), text);
    }
    for (seconds, text) in [(0, "0s"), (59, "59s"), (65, "1m"), (3725, "1h2m")] {
        assert_eq!(format_age(seconds), text);
    }
}

#[test]
fn times_are_written_and_read_as_rfc_3339() {
    assert_eq!(iso_utc(0), "1970-01-01T00:00:00+00:00");
    assert_eq!(iso_utc(1_791_000_000), "2026-10-03T04:00:00+00:00");
    assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00+00:00");
    assert_eq!(iso_utc(-1), "1969-12-31T23:59:59+00:00");
    for seconds in [0, 1_791_000_000, 951_782_400, 4_102_444_799, -86_400] {
        assert_eq!(epoch_seconds(&iso_utc(seconds)), Ok(seconds));
    }
    assert_eq!(epoch_seconds("2026-10-03T06:00:00+02:00"), Ok(1_791_000_000));
    assert_eq!(epoch_seconds("2026-10-03T04:00:00.999Z"), Ok(1_791_000_000));
    assert_eq!(epoch_time("2026-10-03T04:00:00.25Z"), Ok(1_791_000_000.25));
    // Whole seconds are those before the time, before the epoch too.
    assert_eq!(epoch_seconds("1969-12-31T23:59:59.5Z"), Ok(-1));
    for bad in ["<time>", "garbled", "2026-13-01T00:00:00Z", "2026-02-30T00:00:00Z", "2026-10-03T04:00:00", "2026-10-03"] {
        assert!(epoch_seconds(bad).is_err(), "{bad}");
    }
}

/// A turn record that started and ended this many seconds after 2026-10-03T04:00:00, as tests/test_orchestrator.py's
/// turn() makes one.
fn turn(step: &str, file: &str, start: Option<i64>, end: Option<i64>, change: impl FnOnce(&mut Object)) -> TurnRecord {
    let at = |s: Option<i64>| Value::from(s.map(|s| iso_utc(1_791_000_000 + s)));
    let mut t: Object = object([
        ("step", Value::from(step)), ("role", Value::from(step)), ("round", Value::from(1)), ("quality_round", Value::from(0)),
        ("file", Value::from(file)), ("agent", Value::from("claude")), ("model", Value::Null), ("session", Value::Null),
        ("tokens", Value::Null), ("human_paced", Value::Bool(false)),
        ("started_at", at(start)), ("ended_at", at(end)),
        ("seconds", Value::from(start.zip(end).map(|(s, e)| e - s))),
    ]);
    change(&mut t);
    serde_json::from_value(Value::Object(t)).expect("a turn record")
}

#[test]
fn turn_lines_shows_starts_in_the_zone_given() {
    // run-usability/04's case: 20:30:37 UTC is 21:30:37 at +01:00; turns_table stays UTC.
    let mut t = turn("build", "build-1.md", Some(0), Some(5), |_| {});
    t.started_at = Some("2026-11-05T20:30:37+00:00".into());
    let zone = jiff::tz::TimeZone::fixed(jiff::tz::offset(1));
    assert!(turn_lines(std::slice::from_ref(&t), &zone)[0].contains("2026-11-05 21:30:37"));
    assert!(turns_table(&[t]).contains("| 2026-11-05 20:30:37 |"));
}

#[test]
fn a_pipe_in_a_cell_does_not_split_it() {
    let table = turns_table(&[turn("build", "a|b.md", Some(0), Some(5), |t| {
        t.insert("model".into(), Value::from("x|y"));
    })]);
    assert!(table.ends_with("| build | build | 1 | claude | x\\|y | 2026-10-03 04:00:00 | 5s | a\\|b.md |"), "{table}");
}

fn approved_run() -> RunState {
    let mut s = RunState::new("20260929-120000-a1b2c3", "add a rate limiter", "/proj");
    s.verdict = Some("APPROVE".into());
    s.pr_url = Some("https://github.com/o/r/pull/7".into());
    s.branch = Some("orchestrator/add-a-token-bucket-rate-limiter-a1b2c3".into());
    s.round = 1;
    for (role, kind) in [("spec", "claude"), ("build", "codex"), ("review", "claude")] {
        s.agents.insert(role, AgentRecord::new(&format!("{role}-a1b2c3"), "p1", kind));
    }
    s.models.insert("review", "opus".to_string());
    s.turns = vec![
        turn("spec", "spec.md", Some(0), Some(600), |t| { t.insert("round".into(), Value::from(0)); t.insert("human_paced".into(), Value::Bool(true)); }),
        turn("build", "build-1.md", Some(600), Some(1800), |t| { t.insert("agent".into(), Value::from("codex")); }),
        turn("review", "review-1.md", Some(1800), Some(2100), |t| { t.insert("model".into(), Value::from("opus")); }),
    ];
    s
}

#[test]
fn the_summary_of_an_approved_run() {
    let summary = run_summary(&approved_run(), &DEFAULT_WORKFLOW, &["Round 1: review APPROVE".to_string()], 1_791_000_000 + 2100);
    assert_eq!(summary, GOLDEN_SUMMARY);
}

#[test]
fn a_run_without_turns_or_an_end_says_so() {
    let mut s = RunState::new("r-a1b2c3", "  t \n", "/proj");
    s.error = Some("the Builder did not write it".into());
    let summary = run_summary(&s, &DEFAULT_WORKFLOW, &[], 0);
    assert!(summary.contains("- **Outcome:** error: the Builder did not write it\n- **Pull request:** none\n- **Branch:** none\n"));
    assert!(summary.contains("- **Wall time:** ?, from the first turn's start to 1970-01-01T00:00:00+00:00\n"));
    assert!(summary.contains("## Task\n\nt\n") && summary.ends_with("## Turns\n\nNo turns were recorded.\n"));
    s.error = None;
    s.next_harnesses.insert("build", "pi".to_string());
    let summary = run_summary(&s, &DEFAULT_WORKFLOW, &[], 0);
    assert!(summary.contains("- **Outcome:** unfinished\n") && summary.contains("| Builder (build) | pi | default |"));
    let metrics = run_metrics(&s, 10);
    assert_eq!(metrics.get("outcome"), Some(&Value::from("unfinished")));
    assert_eq!(metrics.get("started_at"), Some(&Value::Null));
    assert_eq!(metrics.get("max_quality_rounds"), Some(&Value::Null));
}

#[test]
fn turns_with_unknown_starts_and_ends_and_quality_rounds() {
    let open = turn("build", "build-1.md", None, None, |_| {});
    let analysis = turn("quality", "quality-1-1.md", Some(0), Some(65), |t| {
        t.insert("agent".into(), Value::Null);
        t.insert("quality_round".into(), Value::from(1));
    });
    let lines = turn_lines(&[open, analysis.clone()], &jiff::tz::TimeZone::UTC);
    assert_eq!(lines, ["build    1     claude  ?                    ?       build-1.md", "quality  1 q1  -       2026-10-03 04:00:00  1m 05s  quality-1-1.md"]);
    let mut s = approved_run();
    s.turns = vec![analysis];
    s.restored = vec![serde_json::from_str::<Value>(r#"{"round": 1, "file": "locked-build-1.diff", "paths": {"tests": ["a", "b"], "x": ["c"]}}"#).unwrap().as_object().unwrap().clone()];
    let metrics = run_metrics(&s, 1_791_000_100);
    assert_eq!(metrics.get("restored_locked_files"), Some(&Value::from(3)));
    assert_eq!(metrics.get("seconds"), Some(&Value::from(100)));
    let turns = metrics.get("turns").and_then(Value::as_array).unwrap();
    assert_eq!(turns[0].get("tokens"), Some(&Value::Null));
}

#[test]
fn turn_tokens_sums_the_answers_within_the_turn_as_the_python_engine_does() {
    // TestRunMetrics.test_turn_tokens_needs_a_known_window_and_a_harness_it_reads, and more turns, over each Claude
    // Code log of harness.json and over no log.
    let recorded = oracle("harness.json");
    // Each log's usage, None for the one of Python's time forms, which this engine reads otherwise
    // (crate::harness's a_time_is_read_only_with_its_offset): its expected sums are not compared.
    let logs: Vec<Option<Vec<Usage>>> = recorded.get("session_usage").and_then(Value::as_array).unwrap().iter()
        .map(|case| case.as_array().unwrap())
        .filter(|case| case[0].as_str() == Some("claude"))
        .map(|case| {
            let usage = crate::harness::harness("claude").unwrap().session_usage(&unhex(case[2].as_str().unwrap()));
            (case[1].as_str() != Some("times in each form")).then_some(usage)
        })
        .collect();
    let cases = recorded.get("turn_tokens").and_then(Value::as_array).unwrap();
    assert!(cases.len() >= 16);
    let mut refused = 0;
    for case in cases {
        let [fields_given, expected] = &case.as_array().unwrap()[..] else { panic!("{case:?}") };
        // The case's fields over a turn's others; a field of a type that no turn record has (a session of 7, a
        // start of 5) is refused when state.json is read, so never reaches turn_tokens.
        let mut record = turn("build", "build-1.md", None, None, |_| {});
        let Value::Object(mut fields) = serde_json::to_value(&record).unwrap() else { unreachable!() };
        fields.extend(fields_given.as_object().unwrap().clone());
        match serde_json::from_value::<TurnRecord>(Value::Object(fields)) {
            Ok(typed) => record = typed,
            Err(_) => {
                refused += 1;
                continue;
            }
        }
        let expected = expected.as_array().unwrap();
        assert_eq!(expected.len(), logs.len() + 1);
        for (usage, expected) in logs.iter().map(|u| u.as_deref().map(Some)).chain([Some(None)]).zip(expected) {
            if let Some(usage) = usage {
                assert_eq!(&turn_tokens(&record, usage), expected, "{}", case);
            }
        }
    }
    // The session of 7 and the start of 5; any more means TurnRecord stopped holding cases the engine writes.
    assert_eq!((refused, cases.len()), (2, 16));
}
