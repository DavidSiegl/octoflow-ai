//! `cargo xtask contract`: the engine's Jenkins client (`octoflow_engine::ci::Jenkins`) against the real quality job,
//! to find where the fakes the engine's tests run on (`engine/src/ci/fake.rs`'s FakeJenkins and the e2e world's
//! Jenkins, `engine/e2e/tests/e2e/servers.rs`) answer otherwise than Jenkins does. docs/quality-gate.md, "The calls",
//! says when to run it.
//!
//! It triggers one real build of the quality job with `GIT_REF=main`, so it pushes nothing, and the Jenkinsfile's
//! Release stage, which needs an empty GIT_REF, cannot run. It then makes each call a quality round makes, through the
//! real `Jenkins` over the real `HttpClient`, and checks both what `Jenkins` returns and the raw answer it read, field
//! by field, as the fakes model it. One line per step goes to standard output: `ok`, or what differed.
//!
//! The inputs are the variables of [`INPUTS`], named apart from the engine's CI_ENV so that a developer's ci.env never
//! starts a build by accident. Polls come every CI_POLL_SECONDS, and the build is waited for 2 x QUALITY_TIMEOUT at
//! most, so that a build too slow for a quality round still has every answer checked; whether it was quick enough for
//! one is the last step, reported apart from the contract ([`Timing`]).
//!
//! Exit status: 0 when steps 1 to 8 pass, whatever the timing; 1 when one does not; 2 for an argument, or a variable
//! of INPUTS that is missing or empty; 130 on Ctrl-C.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use octoflow_engine::ci::{CIError, Jenkins, QueueItem};
use octoflow_engine::constants::{CI_POLL_SECONDS, QUALITY_TIMEOUT};
use octoflow_engine::http::{Http, HttpClient, HttpError, Request, Response};
use octoflow_engine::json::ValueExt;
use octoflow_engine::map::Map;
use octoflow_engine::quality::parse_gate;
use serde_json::Value;

use crate::process::{interrupted, watch_interrupts};

/// The variables the check reads: Jenkins's URL, the user and API token it is asked as, and the quality job's full
/// name, folders included (`Octoflow-AI/octoflow-ai-quality`), as `--quality-gate` takes it.
pub const INPUTS: [&str; 4] =
    ["CONTRACT_JENKINS_URL", "CONTRACT_JENKINS_USER", "CONTRACT_JENKINS_TOKEN", "CONTRACT_JOB"];
/// The branch the check's build judges.
const GIT_REF: &str = "main";
const USAGE: &str = "usage: cargo xtask contract";
const EXIT_INTERRUPTED: i32 = 130;

/// How the check paces its polls.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// The pause between two polls of the queue item or the build.
    pub poll: Duration,
    /// How long after the check's start a quality round would give up on the build: the build is waited for twice
    /// that, and the last step says whether quality.md, or the build's end, came within it.
    pub quality_timeout: Duration,
}

impl Timing {
    /// A quality round's: CI_POLL_SECONDS and QUALITY_TIMEOUT.
    pub fn of_quality_round() -> Timing {
        Timing { poll: Duration::from_secs(CI_POLL_SECONDS), quality_timeout: Duration::from_secs(QUALITY_TIMEOUT) }
    }
}

/// The command as `cargo xtask contract` runs it, argv being its arguments: the check of the module's comment, with
/// the process's environment, the network and a quality round's timing. The exit status is the module's.
pub fn main(argv: &[String]) -> i32 {
    if !argv.is_empty() {
        if argv.iter().all(|arg| arg == "-h" || arg == "--help") {
            let help = writeln!(
                io::stdout(),
                "{USAGE}\n\nCheck the engine's Jenkins client against the real quality job; it triggers one build."
            );
            return i32::from(help.is_err());
        }
        eprintln!("{USAGE}\nerror: it takes no arguments; its inputs are the variables {}", INPUTS.join(", "));
        return 2;
    }
    if let Err(e) = watch_interrupts() {
        eprintln!("error: could not watch for Ctrl-C: {e}");
        return 1;
    }
    let env = |name: &str| std::env::var(name).ok();
    run(&env, Arc::new(HttpClient::from_env()), Timing::of_quality_round(), &mut io::stdout())
}

/// The check of the module's comment, with env giving each variable by name, http reaching Jenkins, paced by timing,
/// its lines written to out. Returns the module's exit status; a line out could not take ends it with 1. A missing
/// variable is named on standard error, and nothing is asked of Jenkins.
pub fn run(env: &dyn Fn(&str) -> Option<String>, http: Arc<dyn Http>, timing: Timing, out: &mut dyn Write) -> i32 {
    let missing: Vec<&str> = INPUTS.iter().copied().filter(|name| env(name).is_none_or(|v| v.is_empty())).collect();
    if !missing.is_empty() {
        eprintln!("{USAGE}\nerror: set {}", missing.join(", "));
        return 2;
    }
    let value = |name: &str| env(name).unwrap_or_default();
    let credentials: Map<String> = [
        ("JENKINS_URL", value("CONTRACT_JENKINS_URL")),
        ("JENKINS_USER", value("CONTRACT_JENKINS_USER")),
        ("JENKINS_TOKEN", value("CONTRACT_JENKINS_TOKEN")),
    ]
    .into_iter()
    .collect();
    let recorder = Arc::new(Recorder { http, exchanges: Mutex::new(Vec::new()) });
    // The path names where a missing credential would go, which never shows: every one is set by now.
    let http = Arc::clone(&recorder) as Arc<dyn Http>;
    let jenkins = Jenkins::new(&value("CONTRACT_JOB"), &credentials, "the CONTRACT_ variables", http);
    let mut check = Check { jenkins, recorder, timing, start: Instant::now(), out };
    match check.steps() {
        Ok(passed) => i32::from(!passed),
        Err(Halt::Interrupted) => EXIT_INTERRUPTED,
        Err(Halt::Output(e)) => {
            eprintln!("error: could not write the check's lines: {e}");
            1
        }
    }
}

/// Why the check stopped before its last step.
enum Halt {
    Interrupted,
    Output(io::Error),
}

impl From<io::Error> for Halt {
    fn from(e: io::Error) -> Halt {
        Halt::Output(e)
    }
}

/// A request and the answer Jenkins gave it, raw.
struct Exchange {
    url: String,
    answer: Result<Response, HttpError>,
}

/// An Http that sends each request through another and keeps the exchange, so that the check reads every field of
/// every answer, not only what Jenkins returns of it.
struct Recorder {
    http: Arc<dyn Http>,
    /// The exchanges since the check last took them, oldest first.
    exchanges: Mutex<Vec<Exchange>>,
}

impl Recorder {
    fn exchanges(&self) -> MutexGuard<'_, Vec<Exchange>> {
        // A panic while the lock was held leaves the list whole: each push is one call.
        self.exchanges.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Http for Recorder {
    fn send(&self, request: &Request) -> Result<Response, HttpError> {
        let answer = self.http.send(request);
        self.exchanges().push(Exchange { url: request.url.clone(), answer: answer.clone() });
        answer
    }
}

/// What a step found differing from what the fakes model, or the error Jenkins gave; the step's line shows it.
type Finding = String;

/// One run of the check.
struct Check<'a> {
    jenkins: Jenkins,
    recorder: Arc<Recorder>,
    timing: Timing,
    /// When the check began, which a quality round's timeout is counted from.
    start: Instant,
    out: &'a mut dyn Write,
}

/// What the wait for the build's end found.
struct Ended {
    /// The build's result.
    result: String,
    /// How long after the check's start a quality round could have taken the build: quality.md first read, or the
    /// build's end, whichever came first.
    taken_after: Duration,
}

impl Check<'_> {
    /// Steps 1 to 9, each line written as it ends. Steps 1 to 5 are a chain, each needing the one before, so the first
    /// that fails ends it; 6 to 8 read the ended build, each on its own. Whether steps 1 to 8 passed.
    fn steps(&mut self) -> Result<bool, Halt> {
        let chain = self.chain()?;
        let Some((number, ended)) = chain else { return Ok(false) };
        let quality = self.quality_file(number);
        let quality = self.passed("6 quality_file", quality)?.is_some();
        let tests = self.failed_tests(number);
        let tests = self.passed("7 failed_tests", tests)?.is_some();
        let listed = self.retained_and_console(number, &ended.result);
        let listed = self.passed("8 retained_builds, console_tail", listed)?.is_some();
        let within = ended.taken_after <= self.timing.quality_timeout;
        writeln!(
            self.out,
            "9 timing (apart from the contract): a quality round could take build #{number} {}s after the start, {} \
             its {}s",
            ended.taken_after.as_secs(),
            if within { "within" } else { "past" },
            self.timing.quality_timeout.as_secs()
        )?;
        Ok(quality && tests && listed)
    }

    /// Steps 1 to 5: the build's number and how it ended; None when a step failed, its line written.
    fn chain(&mut self) -> Result<Option<(i64, Ended)>, Halt> {
        let preflight = self.preflight();
        if self.passed("1 preflight", preflight)?.is_none() {
            return Ok(None);
        }
        let queue_url = self.trigger();
        let Some(queue_url) = self.passed("2 trigger", queue_url)? else { return Ok(None) };
        let number = self.queued_build(&queue_url)?;
        let Some(number) = self.passed("3 queue_item", number)? else { return Ok(None) };
        let found = self.find_build(number);
        if self.passed("4 find_build", found)?.is_none() {
            return Ok(None);
        }
        let ended = self.ended(number)?;
        Ok(self.passed("5 build_result", ended)?.map(|ended| (number, ended)))
    }

    /// Writes step's line for outcome, `ok` or the finding; what the step found when it passed, None when it did not.
    fn passed<T>(&mut self, step: &str, outcome: Result<T, Finding>) -> Result<Option<T>, Halt> {
        match outcome {
            Ok(found) => {
                writeln!(self.out, "{step}: ok")?;
                Ok(Some(found))
            }
            Err(finding) => {
                writeln!(self.out, "{step}: {finding}")?;
                Ok(None)
            }
        }
    }

    /// The JSON of the answer to the last request whose URL contains part, taking every exchange recorded so far.
    fn answered_json(&self, part: &str) -> Result<Value, Finding> {
        let body = self.answered(part)?.ok_or_else(|| format!("no answer was recorded for {part}"))?;
        serde_json::from_slice(&body).map_err(|e| format!("the answer for {part} is not JSON: {e}"))
    }

    /// The body of the 2xx answer to the last request whose URL contains part, None for a 404 there, taking every
    /// exchange recorded so far. Errors: no such request, or another failure.
    fn answered(&self, part: &str) -> Result<Option<Vec<u8>>, Finding> {
        let exchanges = std::mem::take(&mut *self.recorder.exchanges());
        let exchange = exchanges.into_iter().rev().find(|e| e.url.contains(part));
        match exchange.map(|e| e.answer) {
            None => Err(format!("no request for {part} was recorded")),
            Some(Ok(response)) => Ok(Some(response.body)),
            Some(Err(HttpError::Status(404))) => Ok(None),
            Some(Err(e)) => Err(format!("the request for {part} failed: {e:?}")),
        }
    }

    /// Step 1: the job takes GIT_REF, and each of its properties' parameter definitions has a string name.
    fn preflight(&self) -> Result<(), Finding> {
        self.jenkins.preflight().map_err(|e| e.to_string())?;
        let job = self.answered_json("api/json")?;
        for property in list(&job, "property", "the job")? {
            if property.get("parameterDefinitions").is_some() {
                for definition in list(property, "parameterDefinitions", "a property")? {
                    text(definition, "name", "a parameter definition")?;
                }
            }
        }
        Ok(())
    }

    /// Step 2: a queue item's URL, which the answer's Location header names.
    fn trigger(&self) -> Result<String, Finding> {
        let queue_url = self.jenkins.trigger(GIT_REF).map_err(|e| e.to_string())?;
        let exchanges = std::mem::take(&mut *self.recorder.exchanges());
        let location = exchanges.last().and_then(|e| e.answer.as_ref().ok()).and_then(|r| r.header("Location"));
        let item = queue_url.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
        match location {
            Some(location) if location.trim_end_matches('/').ends_with(&format!("/queue/item/{item}")) => Ok(queue_url),
            other => Err(format!("the Location header {other:?} does not name queue item {item}")),
        }
    }

    /// Step 3: the number of the build the queue item at queue_url started, polled for until it has one; a started
    /// item says `cancelled: false` beside `executable.number`. A cancelled item fails at once.
    fn queued_build(&mut self, queue_url: &str) -> Result<Result<i64, Finding>, Halt> {
        let polled = self.poll("the queue item to start a build", |check| {
            match check.jenkins.queue_item(queue_url)? {
                QueueItem { cancelled: true, .. } => Ok(Some(Err("Jenkins cancelled the queue item".to_string()))),
                QueueItem { build: None, .. } => Ok(None),
                QueueItem { build: Some(number), .. } => Ok(Some(check.started_item(number))),
            }
        })?;
        Ok(polled.and_then(|found| found))
    }

    /// The raw answer that told step 3 the build's number checked: `cancelled` false and `executable.number` it.
    fn started_item(&self, number: i64) -> Result<i64, Finding> {
        let item = self.answered_json("/queue/item/")?;
        match item.get("cancelled") {
            Some(Value::Bool(false)) => {}
            other => return Err(format!("a started queue item's `cancelled` is {}, not false", shown(other))),
        }
        let executable = item.get("executable").ok_or("a started queue item has no `executable`")?;
        match executable.get("number").and_then(Value::as_i64) {
            Some(n) if n == number => Ok(number),
            _ => Err(format!("a started queue item's `executable.number` is {}", shown(executable.get("number")))),
        }
    }

    /// Step 4: the job's builds list number as GIT_REF's newest, each build with an integer `number` and `actions`.
    fn find_build(&self, number: i64) -> Result<(), Finding> {
        let found = self.jenkins.find_build(GIT_REF, &[]).map_err(|e| e.to_string())?;
        if found != Some(number) {
            return Err(format!("the newest build of {GIT_REF} is {found:?}, not #{number}"));
        }
        let job = self.answered_json("api/json")?;
        for build in list(&job, "builds", "the job")? {
            build.get("number").and_then(Value::as_i64).ok_or("a build's `number` is not an integer")?;
            for action in list(build, "actions", "a build")? {
                if action.get("parameters").is_some() {
                    for parameter in list(action, "parameters", "an action")? {
                        text(parameter, "name", "a parameter")?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Step 5: the build polled, as a quality round polls it, for its result and its quality.md, until it has ended;
    /// each answer about it has `building` a boolean and `result` null or a string, a string once it is not building.
    fn ended(&mut self, number: i64) -> Result<Result<Ended, Finding>, Halt> {
        let mut taken_after = None;
        let polled = self.poll(&format!("build #{number} to end"), |check| {
            let result = check.jenkins.build_result(number)?;
            if let Err(finding) = check.build_answer(result.is_some()) {
                return Ok(Some(Err(finding)));
            }
            let quality = check.jenkins.quality_file(number)?;
            if (quality.is_some() || result.is_some()) && taken_after.is_none() {
                taken_after = Some(check.start.elapsed());
            }
            Ok(result.map(|result| Ok(Ended { result, taken_after: taken_after.unwrap_or_default() })))
        })?;
        Ok(polled.and_then(|ended| ended))
    }

    /// The raw answer build_result last read checked, ended saying whether it said the build had ended.
    fn build_answer(&self, ended: bool) -> Result<(), Finding> {
        let build = self.answered_json("api/json?tree=building")?;
        if !matches!(build.get("building"), Some(Value::Bool(_))) {
            return Err(format!("a build's `building` is {}, not a boolean", shown(build.get("building"))));
        }
        match build.get("result") {
            Some(Value::String(_)) => Ok(()),
            Some(Value::Null) if !ended => Ok(()),
            other => Err(format!("a build's `result` is {}", shown(other))),
        }
    }

    /// Step 6: the ended build's quality.md has a GATE line, or the build archived none.
    fn quality_file(&self, number: i64) -> Result<(), Finding> {
        match self.jenkins.quality_file(number).map_err(|e| e.to_string())? {
            Some(quality) if parse_gate(&quality).is_none() => Err("quality.md has no GATE line".to_string()),
            _ => Ok(()),
        }
    }

    /// Step 7: the build's test report, when it has one, is suites of cases, each with string className, name and
    /// status, and errorDetails a string or null.
    fn failed_tests(&self, number: i64) -> Result<(), Finding> {
        self.jenkins.failed_tests(number).map_err(|e| e.to_string())?;
        let Some(body) = self.answered("testReport")? else { return Ok(()) };
        let report: Value = serde_json::from_slice(&body).map_err(|e| format!("the test report is not JSON: {e}"))?;
        for suite in list(&report, "suites", "the test report")? {
            for case in list(suite, "cases", "a suite")? {
                for field in ["className", "name", "status"] {
                    text(case, field, "a test case")?;
                }
                match case.get("errorDetails") {
                    Some(Value::Null | Value::String(_)) => {}
                    other => return Err(format!("a test case's `errorDetails` is {}", shown(other))),
                }
            }
        }
        Ok(())
    }

    /// Step 8: the job's retained builds hold number with result, and its console's last line ends `Finished:
    /// <result>`, after any timestamp.
    fn retained_and_console(&self, number: i64, result: &str) -> Result<(), Finding> {
        let builds = self.jenkins.retained_builds().map_err(|e| e.to_string())?;
        let ours = builds.iter().find(|b| b.number == number);
        let ours = ours.ok_or_else(|| format!("build #{number} is not retained"))?;
        if ours.result.as_deref() != Some(result) {
            return Err(format!("the retained build #{number} has the result {:?}, not {result}", ours.result));
        }
        let tail = self.jenkins.console_tail(number).map_err(|e| e.to_string())?;
        let last = tail.lines().last().unwrap_or_default();
        if !last.ends_with(&format!("Finished: {result}")) {
            return Err(format!("the console's last line is {last:?}, not one ending `Finished: {result}`"));
        }
        Ok(())
    }

    /// Calls ask every timing.poll until it answers, giving that answer; a failure that may pass is asked again, as a
    /// quality round asks again. Errors: as a Finding, any other failure, or no answer within twice the quality
    /// timeout of the start, waiting saying what was waited for; the Ctrl-C, as a Halt.
    fn poll<T>(
        &mut self,
        waiting: &str,
        mut ask: impl FnMut(&mut Self) -> Result<Option<T>, CIError>,
    ) -> Result<Result<T, Finding>, Halt> {
        let deadline = self.start + 2 * self.timing.quality_timeout;
        loop {
            // The failure of this call, when it failed in a way that may pass.
            let failing = match ask(self) {
                Ok(Some(answer)) => return Ok(Ok(answer)),
                Ok(None) => None,
                Err(e) if e.transient() => Some(e),
                Err(e) => return Ok(Err(e.to_string())),
            };
            if Instant::now() >= deadline {
                let why = failing.map(|e| format!("; last error: {e}")).unwrap_or_default();
                let limit = (2 * self.timing.quality_timeout).as_secs();
                return Ok(Err(format!("waited {limit}s for {waiting}{why}")));
            }
            self.pause()?;
        }
    }

    /// Waits timing.poll, a tenth of a second at a time so that a Ctrl-C ends it soon. Errors: the Ctrl-C.
    fn pause(&self) -> Result<(), Halt> {
        let until = Instant::now() + self.timing.poll;
        loop {
            if interrupted() {
                return Err(Halt::Interrupted);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    }
}

/// The items of value's list field key, what naming value in the finding when it is missing or not a list.
fn list<'v>(value: &'v Value, key: &str, what: &str) -> Result<&'v Vec<Value>, Finding> {
    let found = value.get(key);
    found.and_then(Value::as_array).ok_or_else(|| format!("{what}'s `{key}` is {}, not a list", shown(found)))
}

/// value's string field key, what naming value in the finding when it is missing or not a string.
fn text<'v>(value: &'v Value, key: &str, what: &str) -> Result<&'v str, Finding> {
    let found = value.get(key);
    found.and_then(Value::as_str).ok_or_else(|| format!("{what}'s `{key}` is {}, not a string", shown(found)))
}

/// A field's value as a finding shows it: `missing` when there is none, else its JSON type.
fn shown(value: Option<&Value>) -> &'static str {
    value.map_or("missing", ValueExt::kind)
}
