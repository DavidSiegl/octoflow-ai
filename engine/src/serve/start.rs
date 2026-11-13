//! What serve offers for starting a run and what it is asked to start: [`offered_workflows`], GET /workflows'
//! answer; [`RunRequest`], POST /run's body, and the `run` command it is; [`StartWait`], how long POST /run waits for
//! the run; and [`Started`], how the wait ended. ProjectRuns::start starts it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::constants::{HARNESS_NAMES, DEFAULT_HARNESS};
use crate::env::ConfigDir;
use crate::error::OrchestratorError;
use crate::interrupt::Interrupt;
use crate::pipeline::Pipeline;
use crate::prompts::Prompts;
use crate::workflow_file::{named_workflow, workflow_names};

/// How long POST /run waits for the run it started to save its state.json: checking every `every`, for at most
/// `limit` in all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartWait {
    /// Between two looks at the run's state.json and its pane.
    pub every: Duration,
    /// From the run's start, after which serve answers that it is not yet listed.
    pub limit: Duration,
}

impl StartWait {
    /// The answer look gives, asked at once and then every `every`, or Unlisted once `limit` has passed, or stopping
    /// is raised, without one. look answers None for "not yet".
    pub fn until(&self, stopping: &Interrupt, mut look: impl FnMut() -> Option<Started>) -> Started {
        let deadline = Instant::now() + self.limit;
        loop {
            if let Some(answer) = look() {
                return answer;
            }
            if stopping.is_raised() || Instant::now() >= deadline {
                return Started::Unlisted;
            }
            stopping.sleep(self.every);
        }
    }
}

/// serve's wait for a run it starts (the app-start-runs spec's Decision 5: 30 seconds).
pub const START_WAIT: StartWait = StartWait { every: Duration::from_millis(500), limit: Duration::from_secs(30) };

/// How a started run's wait ended (ProjectRuns::start).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Started {
    /// The run saved its state.json: it is among the project's runs.
    Listed,
    /// `run` exited without saving one, as it does on an error before it claims the run (an unknown workflow, a role
    /// the workflow lacks, a refused run id); what its pane showed last.
    Exited(String),
    /// StartWait's limit passed with neither: it may still save one.
    Unlisted,
}

/// A run POST /run is asked to start: its body, as RunRequest::parse reads it, each field `run`'s flag or argument
/// of the same meaning. An unset one is left to `run`'s default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunRequest {
    /// What to build: run's task. Never empty.
    pub task: String,
    /// --workflow; None for run's default.
    pub workflow: Option<String>,
    /// --agent, one of HARNESS_NAMES.
    pub agent: Option<String>,
    /// --model.
    pub model: Option<String>,
    /// Each --role-agent, (role, kind), in the body's order; kind one of HARNESS_NAMES.
    pub role_agents: Vec<(String, String)>,
    /// Each --role-model, (role, model), in the body's order.
    pub role_models: Vec<(String, String)>,
    /// --no-pr.
    pub no_pr: bool,
    /// --worktree.
    pub worktree: bool,
}

/// The body's fields, as the doc comment of RunRequest::parse gives them.
const FIELDS: [&str; 8] = ["task", "workflow", "agent", "model", "role_agents", "role_models", "no_pr", "worktree"];

impl RunRequest {
    /// The run body asks for: a JSON object with "task", a string that is not empty, and any of "workflow", "agent"
    /// and "model", each a string that is not empty, "agent" one of HARNESS_NAMES; "role_agents" and "role_models",
    /// each an object of role to harness (one of HARNESS_NAMES) or to model, each role and model not empty and a role
    /// without `=`; and "no_pr" and "worktree", each a boolean; null for any of them but "task" is leaving it out.
    /// Errors: why the body is not so, in words a client can show, naming the first field that is wrong.
    pub fn parse(body: &[u8]) -> Result<RunRequest, String> {
        let parsed = std::str::from_utf8(body).ok().and_then(|text| serde_json::from_str::<Value>(text).ok());
        let Some(Value::Object(fields)) = parsed else { return Err("the body must be a JSON object".to_string()) };
        if let Some(unknown) = fields.keys().find(|k| !FIELDS.contains(&k.as_str())) {
            return Err(format!("unknown field {unknown:?}; the fields are {}", FIELDS.join(", ")));
        }
        if fields.iter().any(|(name, json)| holds_nul(name) || json_holds_nul(json)) {
            return Err("the body's strings must hold no NUL character, which no command's argument can".to_string());
        }
        let given = |name: &str| fields.get(name).filter(|v| !v.is_null());
        let text = |name: &str| -> Result<Option<String>, String> {
            match given(name) {
                None => Ok(None),
                Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
                Some(_) => Err(format!("{name} must be a string that is not empty")),
            }
        };
        let flag = |name: &str| match given(name) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(format!("{name} must be true or false")),
        };
        let task = text("task")?.ok_or_else(|| "task must be a string that is not empty".to_string())?;
        let agent = text("agent")?;
        if let Some(kind) = agent.as_deref().filter(|k| !HARNESS_NAMES.contains(k)) {
            return Err(format!("agent {kind:?} is not one of {}", HARNESS_NAMES.join(", ")));
        }
        Ok(RunRequest {
            task,
            workflow: text("workflow")?,
            agent,
            model: text("model")?,
            role_agents: by_role(given("role_agents"), "role_agents", Some(&HARNESS_NAMES))?,
            role_models: by_role(given("role_models"), "role_models", None)?,
            no_pr: flag("no_pr")?,
            worktree: flag("worktree")?,
        })
    }

    /// The command that starts the run in the project target names, as program, the orchestrator's binary, takes it:
    /// `program run --run-id=RUN_ID`, then target (`--machine` and `--cwd`, as target_args gives them), the request's
    /// flags, each as one `--flag=value` word so that a value starting with `-` stays a value, and `--` and the task.
    pub fn argv(&self, program: &str, run_id: &str, target: &[String]) -> Vec<String> {
        let mut argv = vec![program.to_string(), "run".to_string(), format!("--run-id={run_id}")];
        argv.extend(target.iter().cloned());
        let flags = [("workflow", &self.workflow), ("agent", &self.agent), ("model", &self.model)];
        for (name, given) in flags {
            argv.extend(given.as_ref().map(|setting| format!("--{name}={setting}")));
        }
        argv.extend(self.role_agents.iter().map(|(role, kind)| format!("--role-agent={role}={kind}")));
        argv.extend(self.role_models.iter().map(|(role, model)| format!("--role-model={role}={model}")));
        for (on, flag) in [(self.no_pr, "--no-pr"), (self.worktree, "--worktree")] {
            if on {
                argv.push(flag.to_string());
            }
        }
        argv.extend(["--".to_string(), self.task.clone()]);
        argv
    }
}

/// What GET /workflows answers: {"workflows", "agents"}. workflows is each workflow `run --workflow` offers from
/// config (workflow_names), in that order, as {"name", "description", "roles"}, roles being each of its roles in pane
/// order as {"role", "label", "agent", "model"}: the harness the role runs in unless the run names another, its
/// workflow's or DEFAULT_HARNESS, and its workflow's model, null for the harness's own; one that does not load, as a
/// workflow file may not, is {"name", "error"} instead, its error as `run --workflow` would give it
/// (named_workflow). agents is HARNESS_NAMES. The workflows are worded by the built-in prompts, which change no role,
/// harness or model. Errors: the workflows directory cannot be listed.
pub fn offered_workflows(config: &ConfigDir) -> Result<Value, OrchestratorError> {
    let built_in = Prompts::default();
    let mut workflows = vec![];
    for name in workflow_names(config)? {
        let entry = match named_workflow(config, &name, &built_in) {
            Ok(pipeline) => json!({"name": name, "description": pipeline.description(), "roles": roles(&pipeline)}),
            Err(e) => json!({"name": name, "error": e.0}),
        };
        workflows.push(entry);
    }
    Ok(json!({"workflows": workflows, "agents": HARNESS_NAMES}))
}

/// pipeline's roles, as offered_workflows gives them.
fn roles(pipeline: &Pipeline) -> Vec<Value> {
    pipeline
        .roles()
        .iter()
        .map(|(role, label)| {
            let agent = pipeline.agents().get(role).map_or(DEFAULT_HARNESS, String::as_str);
            json!({"role": role, "label": label, "agent": agent, "model": pipeline.models().get(role)})
        })
        .collect()
}

fn holds_nul(text: &str) -> bool {
    text.contains('\0')
}

/// Whether a string of json, or a key of an object in it, holds a NUL.
fn json_holds_nul(json: &Value) -> bool {
    match json {
        Value::String(s) => holds_nul(s),
        Value::Array(items) => items.iter().any(json_holds_nul),
        Value::Object(fields) => fields.iter().any(|(k, v)| holds_nul(k) || json_holds_nul(v)),
        _ => false,
    }
}

/// The (role, setting) pairs of field, the body's field name, in its order; none when it is not given. Each setting
/// must be one of allowed when given. Errors: field is not an object of such pairs.
fn by_role(field: Option<&Value>, name: &str, allowed: Option<&[&str]>) -> Result<Vec<(String, String)>, String> {
    let Some(field) = field else { return Ok(vec![]) };
    let shape = || match allowed {
        Some(kinds) => format!("{name} must be an object of role to one of {}", kinds.join(", ")),
        None => format!("{name} must be an object of role to a string that is not empty"),
    };
    // A role with `=` would split elsewhere than its own end in --role-agent's and --role-model's ROLE=VALUE.
    let role_ok = |role: &str| !role.is_empty() && !role.contains('=');
    let setting_ok = |v: &str| !v.is_empty() && allowed.is_none_or(|a| a.contains(&v));
    let pairs = field.as_object().ok_or_else(shape)?;
    pairs
        .iter()
        .map(|(role, v)| match v.as_str() {
            Some(v) if role_ok(role) && setting_ok(v) => Ok((role.clone(), v.to_string())),
            _ => Err(shape()),
        })
        .collect()
}

#[cfg(test)]
mod tests;
