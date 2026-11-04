//! `cargo xtask contract` over a scripted Jenkins: the variables it needs, a chain whose answers are as the live
//! Jenkins gives them, and an answer that differs or a queue item Jenkins cancelled.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use octoflow_engine::http::{Http, HttpError, Request, Response};
use serde_json::{json, Value};
use xtask::contract::{run, Timing, INPUTS};

const JENKINS: &str = "https://jenkins.example";
const BUILD: i64 = 7;
/// No pause between polls, and a quality timeout no test comes near.
const TIMING: Timing = Timing { poll: Duration::ZERO, quality_timeout: Duration::from_secs(60) };

/// The quality job's Jenkins, answering as the live one did (ticket live-checks/01's Comments): the queue item waits
/// once, then build BUILD runs for two polls and ends SUCCESS with quality.md and a test report.
struct ScriptedJenkins {
    state: Mutex<Script>,
}

/// Part of a URL, and a change to the JSON answered for each URL that contains it.
type Edit = (&'static str, fn(&mut Value));

struct Script {
    /// The URL of every request, in order.
    urls: Vec<String>,
    /// How many more polls see the queue item waiting.
    queue_waits: u32,
    /// How many more polls see the build running.
    build_polls: u32,
    /// Whether Jenkins cancelled the queue item.
    cancelled: bool,
    /// How many more polls of the build Jenkins answers with 503.
    unavailable: u32,
    /// The change made to an answer before it is sent; None for none.
    edit: Option<Edit>,
}

impl ScriptedJenkins {
    fn new() -> Arc<ScriptedJenkins> {
        ScriptedJenkins::with(|_| {})
    }

    fn with(script: impl FnOnce(&mut Script)) -> Arc<ScriptedJenkins> {
        let mut state =
            Script { urls: vec![], queue_waits: 1, build_polls: 2, cancelled: false, unavailable: 0, edit: None };
        script(&mut state);
        Arc::new(ScriptedJenkins { state: Mutex::new(state) })
    }

    fn urls(&self) -> Vec<String> {
        self.state.lock().unwrap().urls.clone()
    }
}

impl Script {
    /// The answer to url: Ok(JSON or text, a Location header), or the status of a failed request.
    fn answer(&mut self, url: &str) -> Result<(Value, Option<String>), u16> {
        let job = "/job/Octoflow-AI/job/octoflow-ai-quality/";
        let path = url.strip_prefix(JENKINS).unwrap_or(url);
        if path.starts_with("/queue/item/5/") {
            if self.cancelled {
                return Ok((json!({"_class": "hudson.model.Queue$CancelledItem", "cancelled": true}), None));
            }
            if self.queue_waits > 0 {
                self.queue_waits -= 1;
                let waiting = json!({"_class": "hudson.model.Queue$WaitingItem", "why": "Waiting for next executor"});
                return Ok((waiting, None));
            }
            let started =
                json!({"_class": "hudson.model.Queue$LeftItem", "cancelled": false, "executable": {"number": BUILD}});
            return Ok((started, None));
        }
        let rest = path.strip_prefix(job).ok_or(404_u16)?;
        let ended = self.build_polls == 0;
        let answer = match rest {
            "buildWithParameters" => return Ok((Value::Null, Some(format!("{JENKINS}/queue/item/5/")))),
            _ if rest.starts_with("api/json?tree=property") => json!({"property": [
                {"_class": "jenkins.model.BuildDiscarderProperty"},
                {"parameterDefinitions": [{"_class": "hudson.model.StringParameterDefinition", "name": "GIT_REF"}]},
            ]}),
            _ if rest.starts_with("api/json?tree=builds%5Bnumber%2Cactions") => json!({"builds": [
                {"number": BUILD, "actions": [{"parameters": [{"name": "GIT_REF", "value": "main"}]}, {}]},
                {"number": BUILD - 1, "actions": [{"parameters": [{"name": "GIT_REF", "value": "other"}]}]},
            ]}),
            _ if rest.starts_with("api/json?tree=builds") => json!({"builds": [
                {"number": BUILD, "result": if ended { json!("SUCCESS") } else { Value::Null }, "description": null},
            ]}),
            _ if rest.starts_with("7/api/json") => {
                if self.unavailable > 0 {
                    self.unavailable -= 1;
                    return Err(503);
                }
                if ended {
                    json!({"building": false, "result": "SUCCESS"})
                } else {
                    self.build_polls -= 1;
                    json!({"building": true, "result": null})
                }
            }
            "7/artifact/quality.md" if ended => json!("GATE: OK\n\nNo issues.\n"),
            "7/artifact/quality.md" => return Err(404),
            _ if rest.starts_with("7/testReport/api/json") => json!({"suites": [{"cases": [
                {"className": "t.T", "name": "a", "status": "PASSED", "errorDetails": null},
                {"className": "t.T", "name": "b", "status": "FAILED", "errorDetails": "boom"},
            ]}]}),
            "7/consoleText" => json!("[Pipeline] End of Pipeline\n[2026-10-27T19:31:01.032Z] Finished: SUCCESS\n"),
            _ => return Err(404),
        };
        Ok((answer, None))
    }
}

impl Http for ScriptedJenkins {
    fn send(&self, request: &Request) -> Result<Response, HttpError> {
        let mut state = self.state.lock().unwrap();
        state.urls.push(request.url.clone());
        let (mut answer, location) = state.answer(&request.url).map_err(HttpError::Status)?;
        if let Some((_, edit)) = state.edit.filter(|(part, _)| request.url.contains(part)) {
            edit(&mut answer);
        }
        let body = match answer {
            Value::String(text) => text.into_bytes(),
            Value::Null => vec![],
            json => json.to_string().into_bytes(),
        };
        let headers = location.map(|l| ("Location".to_string(), l)).into_iter().collect();
        Ok(Response { status: if request.url.ends_with("buildWithParameters") { 201 } else { 200 }, headers, body })
    }
}

/// Every one of INPUTS set, but those named in unset.
fn env(unset: &[&str]) -> impl Fn(&str) -> Option<String> {
    let unset: Vec<String> = unset.iter().map(|s| s.to_string()).collect();
    move |name: &str| {
        let value = match name {
            "CONTRACT_JENKINS_URL" => format!("{JENKINS}/"),
            "CONTRACT_JENKINS_USER" => "agents".to_string(),
            "CONTRACT_JENKINS_TOKEN" => "secret".to_string(),
            "CONTRACT_JOB" => "Octoflow-AI/octoflow-ai-quality".to_string(),
            _ => return None,
        };
        (!unset.iter().any(|u| u == name)).then_some(value)
    }
}

/// The check's exit status and lines, over jenkins.
fn check(jenkins: &Arc<ScriptedJenkins>, unset: &[&str]) -> (i32, String) {
    let mut out = Vec::new();
    let status = run(&env(unset), Arc::clone(jenkins) as Arc<dyn Http>, TIMING, &mut out);
    (status, String::from_utf8(out).expect("the lines are UTF-8"))
}

#[test]
fn a_missing_variable_is_refused_before_jenkins_is_asked() {
    assert_eq!(INPUTS.len(), 4);
    for name in INPUTS {
        let jenkins = ScriptedJenkins::new();
        assert_eq!(check(&jenkins, &[name]), (2, String::new()), "{name}");
        assert_eq!(jenkins.urls(), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn answers_shaped_as_the_live_jenkins_pass_every_step() {
    let jenkins = ScriptedJenkins::new();
    let (status, lines) = check(&jenkins, &[]);
    assert_eq!(status, 0, "{lines}");
    let names: Vec<&str> = lines.lines().map(|line| line.split(':').next().unwrap_or_default()).collect();
    assert_eq!(
        names,
        [
            "1 preflight",
            "2 trigger",
            "3 queue_item",
            "4 find_build",
            "5 build_result",
            "6 quality_file",
            "7 failed_tests",
            "8 retained_builds, console_tail",
            "9 timing (apart from the contract)",
        ]
    );
    assert!(lines.lines().take(8).all(|line| line.ends_with(": ok")), "{lines}");
    assert!(lines.contains("could take build #7 0s after the start, within its 60s"), "{lines}");
    let urls = jenkins.urls();
    assert_eq!(urls.iter().filter(|u| u.ends_with("buildWithParameters")).count(), 1, "one build is triggered");
    assert_eq!(urls.iter().filter(|u| u.contains("/queue/item/")).count(), 2, "polled until it started");
}

#[test]
fn a_transient_failure_while_polling_is_asked_again() {
    let jenkins = ScriptedJenkins::with(|s| s.unavailable = 2);
    let (status, lines) = check(&jenkins, &[]);
    assert_eq!(status, 0, "{lines}");
}

#[test]
fn a_field_missing_from_an_answer_is_named() {
    let jenkins = ScriptedJenkins::with(|s| {
        s.edit = Some(("/queue/item/", |item| {
            item.as_object_mut().expect("an object").remove("cancelled");
        }));
    });
    let (status, lines) = check(&jenkins, &[]);
    assert_eq!(status, 1, "{lines}");
    assert!(lines.contains("3 queue_item: a started queue item's `cancelled` is missing, not false"), "{lines}");
    assert!(!lines.contains("4 find_build"), "the chain stops at the step that failed: {lines}");
}

#[test]
fn a_test_case_shaped_otherwise_fails_its_step_alone() {
    let jenkins = ScriptedJenkins::with(|s| {
        s.edit = Some(("testReport", |report| report["suites"][0]["cases"][1]["errorDetails"] = json!(3)));
    });
    let (status, lines) = check(&jenkins, &[]);
    assert_eq!(status, 1, "{lines}");
    assert!(lines.contains("7 failed_tests: a test case's `errorDetails` is number"), "{lines}");
    assert!(lines.contains("6 quality_file: ok") && lines.contains("8 retained_builds, console_tail: ok"), "{lines}");
}

#[test]
fn a_cancelled_queue_item_fails_at_once() {
    let jenkins = ScriptedJenkins::with(|s| s.cancelled = true);
    let (status, lines) = check(&jenkins, &[]);
    assert_eq!(status, 1, "{lines}");
    assert!(lines.contains("3 queue_item: Jenkins cancelled the queue item"), "{lines}");
    assert_eq!(jenkins.urls().iter().filter(|u| u.contains("/queue/item/")).count(), 1, "asked once");
}
