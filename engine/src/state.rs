//! RunState: what a run has done so far, saved as state.json in its run directory.

use crate::constants::{
    DEFAULT_MAX_QUALITY_ROUNDS, DEFAULT_MAX_ROUNDS, DEFAULT_TURN_TIMEOUT, LOCAL_REFS_PREFIX, RUNS_DIR,
};
use crate::error::OrchestratorError;
use crate::herdr::HerdrServer;
use crate::prompts::{saved_prompts, Prompts};
use crate::json::Object;
use crate::map::Map;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a run has done so far, saved as state.json in its run directory.
///
/// state.json is one line of JSON, an object: "format", STATE_FORMAT, then these fields in this order, then the fields
/// of unknown. Every field that is not an Option must be there, but verdicts, which a state saved by an earlier
/// release lacks; a missing Option is None. The records a field holds as JSON are kept as they were read, key order
/// included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub run_id: String,
    pub task: String,
    /// The project's checkout, which holds the run directory (dir) and .orchestrator/.gitignore.
    pub cwd: String,
    /// The herdr machine the agents run on; None for this one.
    pub machine: Option<String>,
    /// The herdr server of this machine the run's agents run in: the one the engine that started the run, or last
    /// resumed it, reached, where it finds or starts each agent it comes to (one a resume has not come to yet may
    /// still be in an earlier server). serve reaches them there, from whatever environment it runs in. None for a
    /// run on a machine, whose herdr serves it, and for one whose server could not be told
    /// (HerdrServer::of_environment): serve then reaches the server its own environment picks.
    pub herdr_server: Option<HerdrServer>,
    /// The current step's id, or quality, publish or done. main starts a run at its workflow's first step.
    pub phase: String,
    pub round: u32,
    pub workspace_id: String,
    /// The first role's pane, which the second role's pane is split from.
    pub root_pane: String,
    /// The commit the change is judged against, fixed before the first editing step.
    pub base: Option<String>,
    pub base_branch: Option<String>,
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    /// The run's verdict: CHANGES_REQUESTED from a verdict step's request for changes on, APPROVE once the last
    /// verdict step approved a round, which every earlier verdict step approved before it (Workflow::take_turn); so
    /// while a round is between verdict steps it is still the verdict of the round before. For a workflow without a
    /// verdict step, FINISHED or QUALITY_GATE_FAILED once its last step is done. None before either.
    pub verdict: Option<String>,
    /// Each verdict a verdict step gave, in the order given, a round's in the order of its verdict steps. Empty for a
    /// run saved by a release before there were these records, whose one verdict step's last verdict is verdict.
    #[serde(default)]
    pub verdicts: Vec<StepVerdict>,
    /// The files that conflicted with the base branch when the pull request was opened, as a draft; empty again once
    /// refresh found the pull request merging cleanly, merged or closed (refresh_conflict).
    pub conflicts: Vec<String>,
    /// The pull request, by URL, that refresh run by hand found open and merging cleanly when it emptied conflicts, and
    /// that no drain has taken up since: its spec, held back by the conflict, may go to review, though conflicts no
    /// longer shows why it was held (crate::cli::drain::forget_cleared). None for a run whose conflicts no such refresh
    /// cleared, whose clearing a drain has taken up, or that a release before this field saved.
    pub conflict_cleared: Option<String>,
    pub error: Option<String>,
    /// Per role: the agent's name, its pane, the harness it runs in (kind) and, once herdr reports it, its session id.
    /// A record without a kind is a DEFAULT_AGENT agent's.
    pub agents: Map<Map<String>>,
    pub max_rounds: u32,
    /// Seconds a turn may take.
    pub turn_timeout: u64,
    /// As --permission-mode names it; each harness's agent gets its own equivalent (permission_args).
    pub permission_mode: Option<String>,
    pub models: Map<String>,
    /// Per role: the harness its next agent starts in. A role missing here runs in DEFAULT_AGENT.
    pub agent_kinds: Map<String>,
    /// Whether the run works on a branch of its own and ends in a pull request; false for a --no-pr run.
    pub pull_request: bool,
    /// Basename of the handoff file whose prompt was delivered, so a resume does not prompt for it again.
    pub prompted: Option<String>,
    /// Basenames of the verdict files whose one retry was used, and the one whose retry turn is under way, so a
    /// resume neither grants a second retry nor sends the step's own prompt in place of the retry prompt.
    pub retried: Vec<String>,
    pub retrying: Option<String>,
    /// The orchestrator process driving the run, {"host", "pid", "started_at"}; None while none does.
    pub owner: Option<Object>,
    pub heartbeat_at: Option<String>,
    /// The Jenkins job that runs the quality gate; None for a run without the gate.
    pub quality_job: Option<String>,
    /// True once resume --no-quality-gate turned the gate off, for good: quality_job is None then, and the quality
    /// files written until then stay the run's (turn_quality_gate_off).
    pub quality_gate_disabled: bool,
    pub max_quality_rounds: u32,
    /// The quality round the round is in: 0 until the Builder's first report, then q from quality round q on. A
    /// quality-fix turn that ends after resume --no-quality-gate leaves it one past the last quality round, which
    /// never runs.
    pub quality_round: u32,
    /// The quality build in flight, {ref, sha, queue_url, build}, each saved as soon as it is known. rejected lists
    /// builds of ref that ended without a verdict, so a resume does not adopt them again.
    pub ci: Object,
    /// The name of the run's workflow.
    pub workflow: String,
    /// The workflow_definition of one that is not built in, so a resume does not depend on its file.
    pub workflow_definition: Option<Value>,
    /// Prompts::overrides of the prompts the run started with (load_prompts), so a resume does not depend on the
    /// prompt files either; empty for the built-in texts.
    pub prompt_overrides: Map<String>,
    /// What prune did with branch once the pull request was done: "deleted", or "kept: <why>". Written by
    /// prune_branches, never by the run itself.
    pub branch_cleanup: Option<String>,
    /// A run --worktree run's own git worktree, by its absolute path, from the run's start on: round 0 creates it
    /// there. None for a run in the project's checkout, cwd. cwd stays the project, where the run directory is.
    pub worktree: Option<String>,
    /// What close did, one line such as "workspace closed; worktree deleted; branch deleted". Written by close_run,
    /// never by the run itself; a closed run is not resumed.
    pub closed: Option<String>,
    /// One record per handoff file, in the order the turns started (Workflow::turn_record). A turn's record opens
    /// when its prompt is delivered, or a quality analysis starts, and closes when its file is found.
    pub turns: Vec<Object>,
    /// One record per quality round judged, in order: {"round", "quality_round", "gate", "conditions",
    /// "fingerprints_by_check"}, the last two as gate_failures reads them from its quality file. Fingerprints, never
    /// the findings' text, so that state.json stays small.
    pub quality_rounds: Vec<Object>,
    /// The BLOCKED report the current turn waits on the human for, {"file", "question", "k"}: file is the handoff
    /// file's basename, and the report was moved to its k-th blocked name (Workflow::blocked_path). None otherwise.
    pub blocked: Option<Object>,
    /// Per locking step (Step::locks), by id: the paths its last turn locked, as Host::lock gives them. Their locked
    /// versions are in the commit at lock_ref(step id).
    pub locked: Map<Vec<String>>,
    /// One record per editing turn that changed locked files, {"round", "file", "paths"}, in the order of the turns:
    /// file is the basename of the diff of what the turn changed in them, in the run directory, and paths are the
    /// files, by the id of the step that locked them. The files were restored once the record was saved.
    pub restored: Vec<Object>,
    /// One record per run of a proof the proof step (Pipeline::proof_step) wrote with a review that requested
    /// changes, {"round", "written", "file", "finding", "demonstrated", "said"}, in the order they ran: round is the
    /// round it ran in, written the round whose proof step wrote it, file its basename in proofs_dir(written),
    /// finding the first number in that name, demonstrated whether it failed against the change, and said how it
    /// exited and the last PROOF_TAIL_LINES lines it printed. A proof first runs in the round that wrote it
    /// (Workflow::judge_proofs), then again in every later round until it does not fail (Workflow::rerun_proofs).
    pub proofs: Vec<Object>,
    /// The fields of a state.json that this engine does not know, as they were read, in their order, so that a state
    /// a later release of the same format wrote keeps them through a save. to_json writes them, never over a field of
    /// the state's own.
    #[serde(skip)]
    pub unknown: Object,
}

/// One verdict a verdict step gave (RunState::verdicts).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepVerdict {
    /// The round it judged.
    pub round: u32,
    /// The verdict step's id.
    pub step: String,
    /// APPROVE or CHANGES_REQUESTED, as its file's VERDICT line says.
    pub verdict: String,
}

/// Whether verdicts, a run's (RunState::verdicts), came from more than one verdict step: whether what the run
/// publishes and lists names each verdict by its step (verdict_line, run_outcome), rather than giving the run's one
/// verdict, as for a workflow with one verdict step or a run saved without the records.
pub fn judged_by_several(verdicts: &[StepVerdict]) -> bool {
    verdicts.iter().any(|v| v.step != verdicts[0].step)
}

/// The format of state.json this engine reads and writes, which its "format" field holds. A state.json written by a
/// release before STATE_FORMAT_SINCE has no format, and is refused rather than read (RunState::from_saved): it was
/// written in the Python engine's format.
pub const STATE_FORMAT: u64 = 2;

/// The release that first wrote STATE_FORMAT.
pub const STATE_FORMAT_SINCE: &str = "0.54.0";

/// A run id's key: its random suffix, the part after its last `-` (all of it without one).
pub fn run_key(run_id: &str) -> &str {
    run_id.rsplit('-').next().unwrap_or(run_id)
}

/// Where the run of run_id keeps its local refs, `refs/orchestrator/<key>/`, in the project's repository. Nothing
/// pushes them; the run's end in done (Workflow::clean_up_locks) and its close (cli::close::close_run) delete them.
pub fn refs_prefix(run_id: &str) -> String {
    format!("{LOCAL_REFS_PREFIX}{}/", run_key(run_id))
}

impl RunState {
    /// A new run's state: every field but these at its default.
    pub fn new(run_id: &str, task: &str, cwd: &str) -> RunState {
        RunState {
            run_id: run_id.to_string(),
            task: task.to_string(),
            cwd: cwd.to_string(),
            machine: None,
            herdr_server: None,
            phase: "spec".to_string(),
            round: 0,
            workspace_id: String::new(),
            root_pane: String::new(),
            base: None,
            base_branch: None,
            branch: None,
            pr_url: None,
            verdict: None,
            verdicts: Vec::new(),
            conflicts: Vec::new(),
            conflict_cleared: None,
            error: None,
            agents: Map::new(),
            max_rounds: DEFAULT_MAX_ROUNDS,
            turn_timeout: DEFAULT_TURN_TIMEOUT,
            permission_mode: None,
            models: Map::new(),
            agent_kinds: Map::new(),
            pull_request: false,
            prompted: None,
            retried: Vec::new(),
            retrying: None,
            owner: None,
            heartbeat_at: None,
            quality_job: None,
            quality_gate_disabled: false,
            max_quality_rounds: DEFAULT_MAX_QUALITY_ROUNDS,
            quality_round: 0,
            ci: Object::new(),
            workflow: "default".to_string(),
            workflow_definition: None,
            prompt_overrides: Map::new(),
            branch_cleanup: None,
            worktree: None,
            closed: None,
            turns: Vec::new(),
            quality_rounds: Vec::new(),
            blocked: None,
            locked: Map::new(),
            restored: Vec::new(),
            proofs: Vec::new(),
            unknown: Object::new(),
        }
    }

    /// The state a state.json's text holds. Errors: the text is not a JSON object, and as from_saved.
    pub fn from_text(text: &str) -> Result<RunState, OrchestratorError> {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(saved)) => RunState::from_saved(&saved),
            Ok(_) => Err(OrchestratorError("a run state is a JSON object".into())),
            Err(e) => Err(OrchestratorError(format!("a run state that is not JSON: {e}"))),
        }
    }

    /// The state a saved state.json object holds. Errors, naming the run: format_problem's, and a field missing or
    /// holding a value of another type than its own.
    pub fn from_saved(saved: &Object) -> Result<RunState, OrchestratorError> {
        if let Some(problem) = format_problem(saved) {
            return Err(OrchestratorError(problem));
        }
        let run_id = saved.get("run_id").and_then(Value::as_str).unwrap_or("?");
        let mut fields = saved.clone();
        fields.shift_remove("format");
        // serde passes over the fields RunState does not have; unknown keeps them, in their order.
        let mut state: RunState = serde_json::from_value(Value::Object(fields.clone()))
            .map_err(|e| OrchestratorError(format!("run state {run_id} is invalid: {e}")))?;
        // RunState writes every field it has, so a blank state's keys are all it knows.
        let Value::Object(known) = RunState::new("", "", "").to_json() else { unreachable!("a run state is an object") };
        state.unknown = fields.into_iter().filter(|(k, _)| !known.contains_key(k)).collect();
        Ok(state)
    }

    /// The state as state.json holds it: "format", then the fields in RunState's order, then the unknown ones.
    pub fn to_json(&self) -> Value {
        let mut saved = Object::new();
        saved.insert("format".into(), Value::from(STATE_FORMAT));
        // A struct of strings, numbers and JSON values serializes as an object without fail.
        let Ok(Value::Object(fields)) = serde_json::to_value(self) else { unreachable!("a run state is an object") };
        saved.extend(fields);
        // Never over a field of its own, which a stale copy of the state's would otherwise replace.
        for (k, v) in &self.unknown {
            if !saved.contains_key(k) {
                saved.insert(k.clone(), v.clone());
            }
        }
        Value::Object(saved)
    }

    /// state.json's text: to_json on one line, and a newline. One line, because every release before
    /// STATE_FORMAT_SINCE reads a project's states a line each, and a run such a release saved is to be finished or
    /// closed with it (format_problem) in a project where this engine has saved runs too.
    pub fn to_text(&self) -> String {
        self.to_json().to_string() + "\n"
    }

    /// Record in verdicts that the verdict step of id step gave verdict in round, replacing a record of the same step
    /// and round, as a resume that takes that step's turn again gives. The run's verdict is left to the caller.
    pub fn record_verdict(&mut self, round: u32, step: &str, verdict: &str) {
        let given = StepVerdict { round, step: step.to_string(), verdict: verdict.to_string() };
        match self.verdicts.iter_mut().find(|v| v.round == round && v.step == step) {
            Some(earlier) => *earlier = given,
            None => self.verdicts.push(given),
        }
    }

    /// The prompts the run sends: those it started with (saved_prompts). Errors: saved_prompts', its prompt_overrides
    /// naming a prompt this orchestrator lacks or replacing one with a text it cannot fill in.
    pub fn prompts(&self) -> Result<Prompts, OrchestratorError> {
        saved_prompts(&self.prompt_overrides, &self.run_id)
    }

    /// The run id's random suffix, which names its agents and labels its workspace (run_key).
    pub fn key(&self) -> &str {
        run_key(&self.run_id)
    }

    /// The run directory, in the project's checkout.
    pub fn dir(&self) -> String {
        format!("{}/{RUNS_DIR}/{}", self.cwd, self.run_id)
    }

    /// Where the agents work and the run's git steps run: its worktree, or else the project's checkout.
    pub fn work_dir(&self) -> &str {
        self.worktree.as_deref().unwrap_or(&self.cwd)
    }

    /// Where the proof step of round n writes its proofs (Pipeline::proof_step, Pipeline::proof_command).
    pub fn proofs_dir(&self, n: u32) -> String {
        format!("{}/proofs-{n}", self.dir())
    }

    /// The local ref of the commit that holds the files the step locked.
    pub fn lock_ref(&self, step_id: &str) -> String {
        format!("{}locked/{step_id}", refs_prefix(&self.run_id))
    }

    /// The default workflow's spec; the quality files below are every workflow's.
    pub fn spec_path(&self) -> String {
        format!("{}/spec.md", self.dir())
    }

    pub fn build_path(&self, n: u32) -> String {
        format!("{}/build-{n}.md", self.dir())
    }

    pub fn review_path(&self, n: u32) -> String {
        format!("{}/review-{n}.md", self.dir())
    }

    /// Where gitleaks' report on the push named push ("snapshot" or "publish") goes; each scan replaces it.
    pub fn leaks_path(&self, push: &str) -> String {
        format!("{}/leaks-{push}.json", self.dir())
    }

    pub fn quality_path(&self, n: u32, q: u32) -> String {
        format!("{}/quality-{n}-{q}.md", self.dir())
    }

    /// The Builder's answer to quality-<n>-<q>.md.
    pub fn quality_build_path(&self, n: u32, q: u32) -> String {
        format!("{}/build-{n}-q{q}.md", self.dir())
    }

    /// The report of the Builder's turn in round n that answers quality round q, or none when q is 0.
    pub fn builder_report_path(&self, n: u32, q: u32) -> String {
        if q == 0 {
            self.build_path(n)
        } else {
            self.quality_build_path(n, q)
        }
    }

    /// Whether the run has the quality gate, or had it until resume turned it off: whether it may have quality
    /// files.
    pub fn had_quality_gate(&self) -> bool {
        self.quality_job.is_some() || self.quality_gate_disabled
    }

    /// The Builder's last report in round n, once its quality rounds are over: the one the Reviewer reads.
    pub fn last_report_path(&self, n: u32) -> String {
        self.builder_report_path(n, self.quality_round.saturating_sub(1))
    }
}

/// Why this engine does not read the saved state.json object saved, naming its run and what to do with it: it has no
/// format, as every state a release before STATE_FORMAT_SINCE wrote, or another format than STATE_FORMAT. None for a
/// state of STATE_FORMAT, whose fields may still be wrong (from_saved).
pub fn format_problem(saved: &Object) -> Option<String> {
    let run_id = saved.get("run_id").and_then(Value::as_str).unwrap_or("?");
    match saved.get("format") {
        None => Some(format!(
            "run {run_id} was saved by an octoflow before {STATE_FORMAT_SINCE}; finish or close it with that release, \
             or remove {RUNS_DIR}/{run_id}"
        )),
        Some(format) if format.as_u64() == Some(STATE_FORMAT) => None,
        Some(format) => Some(format!(
            "run {run_id} has state format {format}, which this octoflow does not read; it reads format {STATE_FORMAT}"
        )),
    }
}

#[cfg(test)]
mod tests;
