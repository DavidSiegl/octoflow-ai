//! The e2e scenarios (scenarios.rs), each run against the engine and compared with its recording in `recorded/`.
//!
//! A scenario's record (World::record) must equal `recorded/<name>.json`; a difference fails the test, naming where the
//! two first part. `E2E_RECORD=1` writes recordings instead: for a scenario added since, or an intended change of the
//! engine's behaviour, so read the diff of what it recorded as a review of that behaviour before committing it.
//!
//! The engine is E2E_ENGINE, the path of an orchestrator binary, when set, such as the build Jenkins's Test stage
//! made for coverage; else this checkout's, built with cargo (engine::binary). Linux only, as the engine's other
//! process tests.

mod app_recording;
mod client;
mod compare;
mod scenarios;
mod servers;
mod world;

use std::path::{Path, PathBuf};

use octoflow_e2e::engine;
use octoflow_e2e::scratch::Scratch;
use serde::Serialize;
use serde_json::Value;

/// The recordings scenarios are checked against: `<name>.json` in a directory, each read, or, when writing, written.
struct Recordings {
    /// The directory the recordings are in.
    dir: PathBuf,
    /// true means a check writes the scenario's recording, rather than comparing it with the one there.
    writing: bool,
}

impl Recordings {
    /// The checkout's, `recorded/`, written under E2E_RECORD=1.
    fn of_checkout() -> Recordings {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recorded");
        Recordings { dir, writing: std::env::var("E2E_RECORD").as_deref() == Ok("1") }
    }

    /// Run scenario name in a world of its own and compare its record with its recording, or write the recording
    /// when writing. Panics, failing the scenario, when it has no recording, the recording is no JSON, the record
    /// differs from it, naming where they first part, or a recording cannot be written.
    fn check(&self, name: &str, scenario: impl FnOnce(&mut world::World)) {
        let mut w = world::World::new(name, &engine::binary());
        scenario(&mut w);
        let got = w.record();
        self.hold(&self.dir.join(format!("{name}.json")), name, &got);
    }

    /// Compare got, what scenario name gave, with the recording at path, or write it there when writing. Panics as
    /// check does.
    fn hold(&self, path: &Path, name: &str, got: &Value) {
        if self.writing {
            std::fs::write(path, recording_text(got)).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
            return;
        }
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("scenario {name} has no recording at {} ({e}); make it with E2E_RECORD=1, and review what it records", path.display())
        });
        let want = serde_json::from_str::<Value>(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if let Some(difference) = compare::first_difference(&want, got, "$") {
            panic!("scenario {name} differs from its recording at {difference}");
        }
    }
}

/// A recording's text: json pretty-printed with one space of indent a level, every character beyond ASCII as it is,
/// and a newline.
fn recording_text(json: &Value) -> String {
    let mut out = Vec::new();
    let mut writer = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(b" "));
    // A Value serializes into a Vec without fail, as UTF-8.
    json.serialize(&mut writer).expect("JSON serializes");
    String::from_utf8(out).expect("serde_json writes UTF-8") + "\n"
}

#[test]
fn a_recording_keeps_characters_beyond_ascii_as_they_are() {
    let record = serde_json::from_str::<Value>(r#"{"a": ["é \u2014", {}, []], "b": {"n": 1.5, "x": null}}"#).expect("JSON");
    let text = recording_text(&record);
    assert_eq!(text, "{\n \"a\": [\n  \"é \u{2014}\",\n  {},\n  []\n ],\n \"b\": {\n  \"n\": 1.5,\n  \"x\": null\n }\n}\n");
}

/// Recordings in a directory of the test's own, removed when dropped, writing or not.
fn scratch_recordings(name: &str, writing: bool) -> (Scratch, Recordings) {
    let scratch = Scratch::new(&format!("recordings-{name}"));
    let recordings = Recordings { dir: scratch.path().to_path_buf(), writing };
    (scratch, recordings)
}

/// The message a check of an empty scenario named name fails with. The failed check keeps its world for a look;
/// this removes it, so that a passing test leaves nothing behind.
fn failure_of_check(recordings: &Recordings, name: &str) -> String {
    let mut root = None;
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        recordings.check(name, |w| root = Some(w.root().to_path_buf()));
    }))
    .expect_err("a failed check");
    let root = root.expect("the scenario ran");
    std::fs::remove_dir_all(&root).unwrap_or_else(|e| panic!("removing the kept world {}: {e}", root.display()));
    failed.downcast_ref::<String>().cloned().unwrap_or_default()
}

/// A scenario with no recording fails, rather than passing for want of anything to compare.
#[test]
fn a_scenario_without_a_recording_fails() {
    let (_scratch, recordings) = scratch_recordings("unrecorded", false);
    let message = failure_of_check(&recordings, "check_unrecorded");
    assert!(message.starts_with("scenario check_unrecorded has no recording"), "{message}");
}

/// A scenario whose record differs from its recording fails, naming where.
#[test]
fn a_scenario_that_differs_from_its_recording_fails() {
    let (_scratch, recordings) = scratch_recordings("differs", false);
    let path = recordings.dir.join("check_differs.json");
    std::fs::write(&path, "{}\n").unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
    let message = failure_of_check(&recordings, "check_differs");
    assert_eq!(message, "scenario check_differs differs from its recording at $: key \"invocations\" only in this run");
}

/// A recording written holds the scenario: a check that compares with it passes.
#[test]
fn a_recording_written_holds_the_scenario_again() {
    let (_scratch, mut recordings) = scratch_recordings("written", true);
    let scenario = |w: &mut world::World| w.note("seen", Value::from("once"));
    recordings.check("check_written", scenario);
    recordings.writing = false;
    recordings.check("check_written", scenario);
}

/// A #[test] for each scenario, by its name in scenarios.rs.
macro_rules! scenarios {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                Recordings::of_checkout().check(stringify!($name), scenarios::$name);
            }
        )*
    };
}

scenarios!(
    agent_exits,
    blocked_build_answered,
    branches_pruned,
    conflict_refreshed,
    default_approved,
    default_changes_then_approved,
    default_max_rounds,
    doctor_checks,
    failed_run_closed,
    finished_run_closed,
    runs_closed_all,
    gate_turned_off_by_resume,
    gate_turned_on_by_resume,
    kill_in_build,
    kill_in_design,
    kill_in_publish,
    kill_in_quality,
    kill_in_review,
    kill_in_spec,
    kill_in_tests,
    locks_restored,
    machine_run,
    no_pr_run,
    prompt_file_run,
    proofs_run,
    publish_with_conflict,
    quality_fails_then_passes,
    quality_passes,
    quality_stuck,
    quality_workflow_without_review,
    resume_after_agent_exit,
    runs_listed_and_shown,
    serve_endpoints_and_events,
    spec_file_run,
    specs_drained,
    takeover,
    verdict_retried,
    workflow_product_team,
    workflow_quick,
    workflow_spec_build_review,
    workflows_listed,
    worktree_run,
);

/// The desktop app's tests replay what serve sends: `ui/src/test/recorded-stream.json` is app_event_stream's event
/// stream as app_recording writes it, compared here as a scenario's recording is, and written under E2E_RECORD=1.
#[test]
fn app_event_stream() {
    let mut w = world::World::new("app_event_stream", &engine::binary());
    let batches = scenarios::app_event_stream(&mut w);
    let got = app_recording::recording(&batches, &mut w.normalizer, &w.project);
    let path = octoflow_e2e::checkout().join("ui/src/test/recorded-stream.json");
    Recordings::of_checkout().hold(&path, "app_event_stream", &got);
}
