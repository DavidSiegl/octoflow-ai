//! Jenkins's quality job, as orchestrator.py's CI asks it: over HTTP from the orchestrator's own machine
//! (docs/quality-gate.md, "The calls").
//!
//! The credentials travel only in the Authorization header, never in a URL, so no error, log or state.json can carry
//! them; and text from Jenkins goes through [`Jenkins::mask`] before it is shown or saved.

use std::fmt;
use std::sync::Arc;

use crate::constants::{CI_ENV, CI_TOKENS, HTTP_TIMEOUT, JOB_PARAMETERS, REMOTE};
use crate::env::{ci_credentials, ConfigDir};
use crate::error::OrchestratorError;
use crate::host::{Host, PushError, Scanned};
use crate::local_gate;
use crate::state::RunState;
use crate::host::last_lines;
use crate::http::{quote, urlencode, Http, HttpClient, HttpError, Method, Request, Response};
use serde_json::Value;
use crate::map::Map;
use crate::json::{object, Object, ValueExt};

#[cfg(test)]
pub(crate) mod fake;
mod judging;

pub use judging::{Judging, Poll, Started, Verdict};

/// Why a call to Jenkins failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CIError {
    /// A request Jenkins gave no answer to, an HTTP error, or an answer of another shape than the call reads:
    /// Python's CIError. The message names the method, the URL and why, masked.
    Request {
        message: String,
        /// The HTTP status of the answer; None when no answer came, or it was not what the call reads.
        status: Option<u16>,
    },
    /// The call cannot be made, or failed in a way that asking again will not change: the credentials are missing,
    /// the job is gone, its answer named no queue item. Python's OrchestratorError.
    Refused(OrchestratorError),
}

impl CIError {
    /// The HTTP status a request was answered with; None for one without an answer, and for a refused call.
    pub fn status(&self) -> Option<u16> {
        match self {
            CIError::Request { status, .. } => *status,
            CIError::Refused(_) => None,
        }
    }

    /// Whether asking again may succeed: no answer, an answer of another shape, or the server's own trouble (5xx).
    /// A 4xx answer, and a refused call, will not change by asking again.
    pub fn transient(&self) -> bool {
        match self {
            CIError::Request { status, .. } => status.is_none_or(|s| s >= 500),
            CIError::Refused(_) => false,
        }
    }
}

impl fmt::Display for CIError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CIError::Request { message, .. } => f.write_str(message),
            CIError::Refused(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CIError {}

impl From<CIError> for OrchestratorError {
    fn from(e: CIError) -> OrchestratorError {
        match e {
            CIError::Request { message, .. } => OrchestratorError(message),
            CIError::Refused(e) => e,
        }
    }
}

impl From<OrchestratorError> for CIError {
    fn from(e: OrchestratorError) -> CIError {
        CIError::Refused(e)
    }
}

/// A build of the quality job, as Jenkins lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityBuild {
    pub number: i64,
    /// Such as SUCCESS, UNSTABLE, FAILURE or ABORTED; None while it runs.
    pub result: Option<String>,
    /// As the build set it, verbatim; None when it set none.
    pub description: Option<String>,
}

/// A queue item of the quality job, as [`Jenkins::queue_item`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueItem {
    /// Whether Jenkins cancelled it, so that no build will start.
    pub cancelled: bool,
    /// The number of the build it started; None while it waits.
    pub build: Option<i64>,
}

/// What --quality-gate gives a run's quality rounds to: who judges each snapshot of the change.
pub enum QualityGate {
    /// A Jenkins quality job, which judges a snapshot pushed to origin.
    Jenkins(Jenkins),
    /// The engine runs the gate itself (crate::local_gate): --quality-gate LOCAL_GATE.
    Local,
}

/// The --quality-gate name of the local gate, a reserved word rather than a job's name; a top-level Jenkins job of
/// that name is given as `/local` (Jenkins::job_url).
pub const LOCAL_GATE: &str = "local";

impl QualityGate {
    /// The gate --quality-gate's name names: Local for LOCAL_GATE, else the Jenkins job jenkins makes of it. Errors:
    /// jenkins's.
    pub fn named(name: &str, jenkins: &dyn Fn(&str) -> Result<Jenkins, OrchestratorError>) -> Result<QualityGate, OrchestratorError> {
        if name == LOCAL_GATE {
            return Ok(QualityGate::Local);
        }
        Ok(QualityGate::Jenkins(jenkins(name)?))
    }

    /// Its name as --quality-gate takes it and state.json's quality_job keeps it.
    pub fn name(&self) -> &str {
        match self {
            QualityGate::Jenkins(jenkins) => jenkins.job(),
            QualityGate::Local => LOCAL_GATE,
        }
    }

    /// text with the gate's secrets masked (Jenkins::mask); the local gate has none.
    pub fn mask(&self, text: &str) -> String {
        match self {
            QualityGate::Jenkins(jenkins) => jenkins.mask(text),
            QualityGate::Local => text.to_string(),
        }
    }

    /// Fails if a run with this gate in the checkout at cwd could not judge its change: gitleaks, which scans every
    /// snapshot before it is judged, cannot run; for a Jenkins job, cwd has no origin to push snapshots to, or the job
    /// fails Jenkins::preflight; for the local gate, host fails local_gate::preflight. Errors: those, and git failing.
    pub fn preflight(&self, host: &dyn Host, cwd: &str) -> Result<(), OrchestratorError> {
        host.check_gitleaks()?;
        match self {
            QualityGate::Jenkins(jenkins) => {
                if !host.has_origin(cwd)? {
                    return Err(OrchestratorError(format!(
                        "the quality gate pushes snapshots to {REMOTE}, and {cwd} has no {REMOTE}"
                    )));
                }
                jenkins.preflight()
            }
            QualityGate::Local => local_gate::preflight(host),
        }
    }

    /// Fails if a run past its preflight, resumed into its quality loop, could not have a snapshot judged: a Jenkins
    /// job's credentials are missing (Jenkins::check_credentials). Asks no one over the network, so it is cheap
    /// enough for every start.
    pub fn check_resume(&self) -> Result<(), OrchestratorError> {
        match self {
            QualityGate::Jenkins(jenkins) => jenkins.check_credentials(),
            QualityGate::Local => Ok(()),
        }
    }

    /// Start judging quality round q of round n of the run in state: a snapshot of its change, a commit of its work
    /// dir's tree whose only parent is the run's base, scanned by gitleaks by the base's rules, then judged.
    ///
    /// Jenkins judges a snapshot pushed to origin's branch RunState::ci_ref, once gitleaks finds no secret in it or in
    /// the commits below it that origin lacks; state.ci then records the push, in place of what it held, for the
    /// caller to save. When state.ci already records that branch, as on a resume, the judgement goes on from the
    /// furthest link it records, and no snapshot is taken. The local gate judges a new snapshot each start, scanning
    /// only what it adds to the base, in a job of its own under RunState::gate_dir (local_gate::start).
    ///
    /// Errors: the run has no base; state.ci's link is of another type than a chain saves; git, the scan or the push
    /// fails; local_gate::start's.
    pub fn start(
        &self,
        host: &dyn Host,
        state: &mut RunState,
        n: u32,
        q: u32,
    ) -> Result<Started<'_>, OrchestratorError> {
        let base =
            state.base.clone().ok_or_else(|| OrchestratorError("a quality round before the run has a base".into()))?;
        let (git_ref, place) = (state.ci_ref(n, q), state.quality_place(n, q));
        if let QualityGate::Jenkins(jenkins) = self {
            if state.ci.get("ref").and_then(Value::as_str) == Some(git_ref.as_str()) {
                let sha = judging::ci_text(&state.ci, "sha")?;
                return Ok(Started::Judging(Judging::jenkins(jenkins, &sha, &git_ref, &place, true)));
            }
        }
        let work_dir = state.work_dir().to_string();
        let message = format!("Orchestrator run {}: round {n}, quality round {q}", state.run_id);
        let sha = host.snapshot(&work_dir, &base, &message)?;
        let leaks_path = state.leaks_path("snapshot");
        let (scanned, so_what) = match self {
            QualityGate::Jenkins(_) => (
                host.push_ref(&work_dir, &sha, &git_ref, &base, &leaks_path),
                "it was not pushed and Jenkins did not judge the change",
            ),
            QualityGate::Local => (
                host.refuse_leaks(&work_dir, &sha, &base, Scanned::SinceBase, &leaks_path),
                "the local gate did not judge the change",
            ),
        };
        match scanned {
            Ok(()) => {}
            Err(PushError::Failed(e)) => return Err(e),
            Err(PushError::Leaks(found)) => return Ok(Started::Leaked { found, snapshot: sha, so_what }),
        }
        Ok(Started::Judging(match self {
            QualityGate::Jenkins(jenkins) => {
                state.ci = object([("ref", Value::from(git_ref.as_str())), ("sha", Value::from(sha.as_str()))]);
                Judging::jenkins(jenkins, &sha, &git_ref, &place, false)
            }
            QualityGate::Local => {
                let job = local_gate::start(host, &work_dir, &sha, &base, &state.gate_dir(n, q))?;
                Judging::local(job, &sha, &place)
            }
        }))
    }
}

/// Jenkins's quality job, reached through an [`Http`].
pub struct Jenkins {
    /// The job's full name, folders included, as `--quality-gate` takes it: `folder/job`.
    job: String,
    /// The value of each of CI_ENV, by name; "" for one that is missing.
    credentials: Map<String>,
    /// Where the credentials missing from the environment are read from, as ci_env_path names it.
    ci_env_path: String,
    http: Arc<dyn Http>,
}

impl Jenkins {
    /// The job named job, asked with the credentials of CI_ENV that credentials holds, through http. ci_env_path is
    /// the file a missing credential is to be put in, which check_credentials names.
    pub fn new(job: &str, credentials: &Map<String>, ci_env_path: &str, http: Arc<dyn Http>) -> Jenkins {
        let credentials = CI_ENV.iter().map(|&k| (k, credentials.get(k).cloned().unwrap_or_default())).collect();
        Jenkins { job: job.to_string(), credentials, ci_env_path: ci_env_path.to_string(), http }
    }

    /// The job named job, asked over the network with the credentials of the process's environment, each one it
    /// lacks taken from config's ci.env, as ci_credentials reads them. Errors: ci_credentials's, for a ci.env it
    /// cannot read or refuses.
    pub fn from_env(job: &str, config: &ConfigDir) -> Result<Jenkins, OrchestratorError> {
        Jenkins::with_env(job, config, &|name| std::env::var(name).ok())
    }

    /// The job named job, as from_env makes it, in the environment env gives each variable of, by name.
    pub(crate) fn with_env(
        job: &str,
        config: &ConfigDir,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Jenkins, OrchestratorError> {
        let path = config.ci_env_path();
        let credentials = ci_credentials(env, &path)?;
        Ok(Jenkins::new(job, &credentials, &path, Arc::new(HttpClient::with_env(env))))
    }

    /// The job's full name, as given.
    pub fn job(&self) -> &str {
        &self.job
    }

    /// The job's URL, with a slash at the end: each folder of its name a `/job/<name>` under JENKINS_URL. A name may
    /// start with one `/`, which names no folder: `/local` is the top-level job that `local` (LOCAL_GATE) cannot name.
    pub fn job_url(&self) -> String {
        let job = self.job.strip_prefix('/').unwrap_or(&self.job);
        let path: String = job.split('/').map(|part| format!("/job/{}", quote(part))).collect();
        format!("{}{path}/", self.base())
    }

    /// JENKINS_URL without the slashes at its end.
    fn base(&self) -> &str {
        self.credential("JENKINS_URL").trim_end_matches('/')
    }

    fn credential(&self, name: &str) -> &str {
        self.credentials.get(name).map_or("", String::as_str)
    }

    /// text with each of CI_TOKENS that is set replaced by `****`.
    pub fn mask(&self, text: &str) -> String {
        CI_TOKENS
            .iter()
            .map(|name| self.credential(name))
            .filter(|token| !token.is_empty())
            .fold(text.to_string(), |text, token| text.replace(token, "****"))
    }

    /// Nothing when each of CI_ENV is set; an error naming those that are not, and where to set them, otherwise.
    pub fn check_credentials(&self) -> Result<(), OrchestratorError> {
        let missing: Vec<&str> = CI_ENV.iter().copied().filter(|k| self.credential(k).is_empty()).collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(OrchestratorError(format!(
            "the quality gate needs {} in the orchestrator's environment or in {}",
            missing.join(", "),
            self.ci_env_path
        )))
    }

    /// Fails, before the interview, if the gate could not run: the credentials are missing, Jenkins does not answer
    /// or has no such job, or the job lacks one of JOB_PARAMETERS. Asks Jenkins once.
    pub fn preflight(&self) -> Result<(), OrchestratorError> {
        self.check_credentials()?;
        let url =
            format!("{}api/json?{}", self.job_url(), urlencode(&[("tree", "property[parameterDefinitions[name]]")]));
        let job = self.get_json(&url).map_err(|e| match e {
            CIError::Request { status: Some(404), .. } => {
                OrchestratorError(format!("there is no Jenkins job {} at JENKINS_URL: {e}", self.job))
            }
            CIError::Request { .. } => OrchestratorError(format!("Jenkins at JENKINS_URL failed the preflight: {e}")),
            CIError::Refused(e) => e,
        })?;
        let mut defined = Vec::new();
        for property in listed(job.get("property")).iter().filter_map(Value::as_object) {
            for definition in listed(property.get("parameterDefinitions")) {
                defined.push(definition.get("name").cloned());
            }
        }
        let missing: Vec<&str> =
            JOB_PARAMETERS.iter().copied().filter(|p| !defined.contains(&Some(Value::from(*p)))).collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(OrchestratorError(format!(
            "Jenkins job {} lacks the parameters {}; build it once by hand with empty parameters, so Jenkins learns \
             them from the Jenkinsfile",
            self.job,
            missing.join(", ")
        )))
    }

    /// Queues a build of the branch git_ref, which runs the gate on it; the queue item's URL, under JENKINS_URL.
    /// Errors: refused for a job that is gone (404) and an answer that names no queue item; any other failed request
    /// as it failed.
    pub fn trigger(&self, git_ref: &str) -> Result<String, CIError> {
        let url = format!("{}buildWithParameters", self.job_url());
        let answer =
            self.request(Method::Post, &url, Some(urlencode(&[("GIT_REF", git_ref)]))).map_err(|e| match e {
                CIError::Request { status: Some(404), .. } => {
                    CIError::Refused(OrchestratorError(format!("Jenkins job {} is gone: {e}", self.job)))
                }
                e => e,
            })?;
        let item = answer
            .header("Location")
            .and_then(queue_item_number)
            .ok_or_else(|| CIError::Refused(OrchestratorError(format!("POST {url} named no queue item"))))?;
        // Rebuilt from JENKINS_URL: behind a proxy, Location may name a host only Jenkins itself can reach.
        Ok(format!("{}/queue/item/{item}/", self.base()))
    }

    /// The queue item at queue_url, as trigger gave it. Errors: a failed request, and an answer whose `executable`,
    /// when set, is not an object with an integer `number`.
    pub fn queue_item(&self, queue_url: &str) -> Result<QueueItem, CIError> {
        let url = format!("{queue_url}api/json");
        let item = self.get_json(&url)?;
        let shape = || self.not_shaped("GET", &url, "a queue item");
        let build = match item.get("executable").filter(|e| e.is_set()) {
            None => None,
            Some(Value::Object(executable)) => match executable.get("number") {
                None | Some(Value::Null) => None,
                Some(n) if n.is_number() => Some(n.as_i64().ok_or_else(shape)?),
                Some(_) => return Err(shape()),
            },
            Some(_) => return Err(shape()),
        };
        Ok(QueueItem { cancelled: item.get("cancelled").is_some_and(Value::is_set), build })
    }

    /// The newest of the job's last 20 builds whose GIT_REF is git_ref, leaving out the numbers in skip; None when
    /// none is.
    pub fn find_build(&self, git_ref: &str, skip: &[i64]) -> Result<Option<i64>, CIError> {
        let url = format!(
            "{}api/json?{}",
            self.job_url(),
            urlencode(&[("tree", "builds[number,actions[parameters[name,value]]]{0,20}")])
        );
        let job = self.get_json(&url)?;
        let shape = || self.not_shaped("GET", &url, "the job's builds");
        for build in listed(job.get("builds")) {
            let build = build.as_object().ok_or_else(shape)?;
            let built = git_ref_parameter(build).ok_or_else(shape)?;
            let number = build.get("number").and_then(Value::as_i64);
            if let (Some(Value::String(built)), Some(number)) = (built, number) {
                if built == git_ref && !skip.contains(&number) {
                    return Ok(Some(number));
                }
            }
        }
        Ok(None)
    }

    /// The job's builds that Jenkins still keeps, newest first, the newest 100 at most. Errors: refused before
    /// asking when credentials are missing; a failed request, or an answer that is not a list of builds.
    pub fn retained_builds(&self) -> Result<Vec<QualityBuild>, CIError> {
        let url = format!(
            "{}api/json?{}",
            self.job_url(),
            urlencode(&[("tree", "builds[number,result,description]{0,100}")])
        );
        let job = self.get_json(&url)?;
        let builds = job
            .get("builds")
            .and_then(Value::as_array)
            .ok_or_else(|| self.not_shaped("GET", &url, "the job's builds"))?;
        builds
            .iter()
            .map(|build| {
                let number = build.get("number").and_then(Value::as_i64);
                let result = optional_text(build.get("result"));
                let description = optional_text(build.get("description"));
                match (number, result, description) {
                    (Some(number), Some(result), Some(description)) => Ok(QualityBuild { number, result, description }),
                    _ => Err(self.not_shaped("GET", &url, "the job's builds")),
                }
            })
            .collect()
    }

    /// The finished build's result, such as SUCCESS, UNSTABLE, FAILURE or ABORTED; None while it runs.
    pub fn build_result(&self, number: i64) -> Result<Option<String>, CIError> {
        let url = self.build_url(number, "api/json", Some("building,result"));
        let build = self.get_json(&url)?;
        if build.get("building").is_some_and(Value::is_set) {
            return Ok(None);
        }
        optional_text(build.get("result")).ok_or_else(|| self.not_shaped("GET", &url, "the build's result"))
    }

    /// The quality file the build's Gate stage archived, quality.md, each byte that is not UTF-8 replaced; None when
    /// the build archived none.
    pub fn quality_file(&self, number: i64) -> Result<Option<String>, CIError> {
        self.artifact(number, "quality.md")
    }

    /// The gate's report the build's Gate stage archived beside its quality file, quality.json, each byte that is not
    /// UTF-8 replaced; None when the build archived none.
    pub fn quality_json(&self, number: i64) -> Result<Option<String>, CIError> {
        self.artifact(number, "quality.json")
    }

    /// The build's archived artifact name as text, each byte that is not UTF-8 replaced; None when it has none.
    fn artifact(&self, number: i64, name: &str) -> Result<Option<String>, CIError> {
        match self.request(Method::Get, &self.build_url(number, &format!("artifact/{name}"), None), None) {
            Ok(answer) => Ok(Some(String::from_utf8_lossy(&answer.body).into_owned())),
            Err(e) if e.status() == Some(404) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// (test, error) of each failing test of the build: its class name and name joined by a dot, and its error, ""
    /// for none; None when the build has no test report.
    pub fn failed_tests(&self, number: i64) -> Result<Option<Vec<(String, String)>>, CIError> {
        let url =
            self.build_url(number, "testReport/api/json", Some("suites[cases[className,name,status,errorDetails]]"));
        let report = match self.get_json(&url) {
            Ok(report) => report,
            Err(e) if e.status() == Some(404) => return Ok(None),
            Err(e) => return Err(e),
        };
        let shape = || self.not_shaped("GET", &url, "a test report");
        let mut failed = Vec::new();
        for suite in listed(report.get("suites")) {
            let suite = suite.as_object().ok_or_else(shape)?;
            for case in listed(suite.get("cases")) {
                failed.extend(failed_test(case.as_object().ok_or_else(shape)?));
            }
        }
        Ok(Some(failed))
    }

    /// The end of the build's console (crate::host::last_lines), each byte that is not UTF-8 replaced.
    pub fn console_tail(&self, number: i64) -> Result<String, CIError> {
        let answer = self.request(Method::Get, &self.build_url(number, "consoleText", None), None)?;
        Ok(last_lines(&String::from_utf8_lossy(&answer.body)))
    }

    fn build_url(&self, number: i64, path: &str, tree: Option<&str>) -> String {
        let query = tree.map_or(String::new(), |tree| format!("?{}", urlencode(&[("tree", tree)])));
        format!("{}{number}/{path}{query}", self.job_url())
    }

    /// The 2xx answer to a request with Basic authentication. Errors: refused, before asking, when credentials are
    /// missing; a failed request, worded as `<method> <url>: HTTP <status>` or `<method> <url>: <why>`, masked.
    fn request(&self, method: Method, url: &str, form: Option<String>) -> Result<Response, CIError> {
        self.check_credentials()?;
        let auth = basic_auth(self.credential("JENKINS_USER"), self.credential("JENKINS_TOKEN"));
        let request = Request {
            method,
            url: url.to_string(),
            headers: vec![("Authorization".to_string(), auth)],
            form,
            timeout: HTTP_TIMEOUT,
        };
        self.http.send(&request).map_err(|e| {
            let (why, status) = match e {
                HttpError::Status(status) => (format!("HTTP {status}"), Some(status)),
                HttpError::NoAnswer(why) => (why, None),
            };
            CIError::Request { message: self.mask(&format!("{} {url}: {why}", method.as_str())), status }
        })
    }

    /// The JSON object Jenkins answers a GET of url with. Errors: request's, and an answer that is not a JSON object.
    fn get_json(&self, url: &str) -> Result<Value, CIError> {
        let answer = self.request(Method::Get, url, None)?;
        let not_json = |why: String| CIError::Request {
            message: self.mask(&format!("GET {url}: the answer is not JSON: {why}")),
            status: None,
        };
        let text = std::str::from_utf8(&answer.body).map_err(|e| not_json(e.to_string()))?;
        let value = serde_json::from_str::<Value>(text).map_err(|e| not_json(e.to_string()))?;
        if value.as_object().is_none() {
            return Err(CIError::Request {
                message: self.mask(&format!("GET {url}: the answer is not a JSON object")),
                status: None,
            });
        }
        Ok(value)
    }

    /// The error of an answer that is not what was asked for, as retained_builds words it.
    fn not_shaped(&self, method: &str, url: &str, what: &str) -> CIError {
        CIError::Request { message: self.mask(&format!("{method} {url}: the answer is not {what}")), status: None }
    }
}

/// The items of a JSON list, as `value or []` iterates them: none for a missing or false value, the list's for a list.
/// Any other true value has none either, where Python would fail on it.
fn listed(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The value of the last GIT_REF parameter among the actions of build, a build as the job's builds list it:
/// Some(None) when it has none, or that one has no value; None when one of its parameters is not an object. An action
/// that is not an object has no parameters.
fn git_ref_parameter(build: &Object) -> Option<Option<&Value>> {
    let mut value = None;
    for action in listed(build.get("actions")).iter().filter_map(Value::as_object) {
        for parameter in listed(action.get("parameters")) {
            let parameter = parameter.as_object()?;
            if parameter.get("name").and_then(Value::as_str) == Some("GIT_REF") {
                value = parameter.get("value");
            }
        }
    }
    Some(value)
}

/// (test, error) of case, a case of a test report, as failed_tests lists it when it failed (status FAILED or
/// REGRESSION): its className and name joined by a dot, and its errorDetails, "" for none; None for a case that did
/// not fail.
fn failed_test(case: &Object) -> Option<(String, String)> {
    if !matches!(case.get("status").and_then(Value::as_str), Some("FAILED" | "REGRESSION")) {
        return None;
    }
    let field = |name: &str| case.get(name).cloned().unwrap_or(Value::Null);
    let error = field("errorDetails");
    let error = if error.is_set() { error.text() } else { String::new() };
    Some((format!("{}.{}", field("className").text(), field("name").text()), error))
}

/// A string or null field: Some(None) for null or missing, Some(Some(text)) for a string, None for anything else.
fn optional_text(value: Option<&Value>) -> Option<Option<String>> {
    match value {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => None,
    }
}

/// The number at the end of a Location naming a queue item, `.../queue/item/<digits>/`, the slash optional; None for
/// any other.
fn queue_item_number(location: &str) -> Option<&str> {
    let rest = location.strip_suffix('/').unwrap_or(location);
    let start = rest.rfind("/queue/item/")? + "/queue/item/".len();
    let digits = &rest[start..];
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then_some(digits)
}

/// The Authorization header of HTTP Basic authentication as user with password.
fn basic_auth(user: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = format!("{user}:{password}").into_bytes();
    let mut out = String::from("Basic ");
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
