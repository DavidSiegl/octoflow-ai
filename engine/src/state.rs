//! RunState: what a run has done so far, saved as state.json in its run directory.

use crate::constants::{
    DEFAULT_HARNESS, DEFAULT_MAX_QUALITY_ROUNDS, DEFAULT_MAX_ROUNDS, DEFAULT_TURN_TIMEOUT,
    LOCAL_REFS_PREFIX, RUNS_DIR,
};
use crate::error::OrchestratorError;
use crate::herdr::HerdrServer;
use crate::prompts::{saved_prompts, Prompts};
use crate::json::Object;
use crate::map::Map;
use crate::pipeline::{Pipeline, DONE, QUALITY};
use crate::workflow_file::run_pipeline;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a run has done so far, saved as state.json in its run directory.
///
/// state.json is one line of JSON, an object: "format", STATE_FORMAT, then these fields in this order, then the fields
/// of unknown. Every field that is not an Option must be there, but verdicts, which a state saved by an earlier
/// release lacks; a missing Option is None. The records a field holds as JSON are kept as they were read, key order
/// included; the agent and turn records are typed (AgentRecord, TurnRecord).
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
    /// The pull request, by URL, that refresh run by hand found open and merging cleanly, merged or closed when it
    /// emptied conflicts, and that no drain has taken up since: its spec, held back by the conflict, may go to review
    /// or be settled by the pull request's state, though conflicts no longer shows why it was held
    /// (crate::cli::drain::forget_cleared). None for a run whose conflicts no such refresh
    /// cleared, whose clearing a drain has taken up, or that a release before this field saved.
    pub conflict_cleared: Option<String>,
    pub error: Option<String>,
    /// Per role that has started an agent, its last one, in the order the roles first started theirs.
    pub agents: Map<AgentRecord>,
    pub max_rounds: u32,
    /// Seconds a turn may take.
    pub turn_timeout: u64,
    /// As --permission-mode names it; each harness's agent gets its own equivalent (permission_args).
    pub permission_mode: Option<String>,
    pub models: Map<String>,
    /// Per role: the harness its next agent starts in, which may differ from the one its agent runs in (harness).
    /// A role missing here runs in DEFAULT_HARNESS. Saved as "agent_kinds", the name earlier releases gave it.
    #[serde(rename = "agent_kinds")]
    pub next_harnesses: Map<String>,
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
    /// The quality gate's name, LOCAL_GATE (crate::local_gate), for a run with the gate; None for a run without it. A
    /// release before 0.72.0 also saved a Jenkins job's name here, which a resume replaces (cli::start::resumed_state).
    pub quality_job: Option<String>,
    /// True once resume --no-quality-gate turned the gate off, for good: quality_job is None then, and the quality
    /// files written until then stay the run's (turn_quality_gate_off).
    pub quality_gate_disabled: bool,
    pub max_quality_rounds: u32,
    /// The quality round the round is in: 0 until the Builder's first report, then q from quality round q on. A
    /// quality-fix turn that ends after resume --no-quality-gate leaves it one past the last quality round, which
    /// never runs.
    pub quality_round: u32,
    /// Empty but in a state.json a release before 0.72.0 saved, which kept a Jenkins quality build in flight here; a
    /// resume drops it (cli::start::resumed_state). Still written, as those releases read no state.json without it.
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
    pub turns: Vec<TurnRecord>,
    /// One record per quality round judged, in order: {"round", "quality_round", "gate", "conditions",
    /// "fingerprints_by_check"}, the last two the round's GateFailures, as read_report reads them from its build's
    /// quality.json, or gate_failures from its quality file where there is none (Workflow::quality), with the
    /// orchestrator's checks' added. Fingerprints, never the findings' text, so that state.json stays far smaller
    /// than the quality files, though a round records one fingerprint per failing finding, without their limit.
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
    /// changes, {"round", "written", "file", "finding", "demonstrated", "said"} and "moved" when true
    /// (ProofRun::moved), in the order they ran: round is the round it ran in, written the round whose proof step wrote
    /// it, file its basename in proofs_dir(written), finding the first number in that name, demonstrated whether it
    /// failed against the change, and said how it exited and the last PROOF_TAIL_LINES lines it printed. A proof first
    /// runs in the round that wrote it (Workflow::judge_proofs), then again in every later round until it does not fail
    /// or its file is gone (Workflow::rerun_proofs, Workflow::carried_proofs).
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

/// A role's agent as the run started it (RunState::agents). Saved as {"name", "pane", "kind"}, and "session" once
/// known, then the fields this engine does not know.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    /// The agent's herdr name, `<role>-<key>`.
    pub name: String,
    /// The herdr pane it runs in; empty once the run's workspace was found gone, until the role's next agent starts.
    pub pane: String,
    /// The harness it runs in, never empty; one of HARNESS_NAMES when this engine started it, though a run may hold a
    /// harness this engine no longer runs, which Workflow::next_harness refuses once the role's agent restarts.
    #[serde(rename = "kind", deserialize_with = "harness_name")]
    pub harness: String,
    /// The harness's session id, as herdr reports it; None until it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The fields of the record this engine does not know, as read, written after its own.
    #[serde(flatten)]
    pub unknown: Object,
}

/// The record of one handoff file's turn (RunState::turns), as Workflow::turn_record opens it and end_turn closes it.
/// Saved with every field, None as null, in this order, then the fields this engine does not know.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    /// The phase the turn was in: its step's id, or QUALITY for a quality analysis.
    pub step: String,
    /// The role whose turn it was; QUALITY for a quality analysis.
    pub role: String,
    /// The run's review round (RunState::round) when the turn started.
    pub round: u32,
    /// The quality round the turn answered or judged; 0 for a turn of a step the gate does not judge.
    pub quality_round: u32,
    /// The handoff file's basename: the file the turn wrote, or the name it was moved to once rejected or blocked.
    pub file: String,
    /// The harness the turn's agent ran in, never empty; None for a quality analysis, which runs no agent.
    #[serde(rename = "agent", deserialize_with = "optional_harness_name")]
    pub harness: Option<String>,
    /// The role's model, as RunState::models has it; None for the harness's default and a quality analysis.
    #[serde(deserialize_with = "Option::deserialize")]
    pub model: Option<String>,
    /// The agent's session for the whole turn; None when that is not known, as for a turn that ended in another
    /// session than it started in.
    #[serde(deserialize_with = "Option::deserialize")]
    pub session: Option<String>,
    /// The tokens of the turn, as crate::summary::turn_tokens gives them, which count_tokens counts once the run is
    /// done; null until then, and always for a quality analysis.
    pub tokens: Value,
    /// Whether the human paced the turn as much as its agent: a human-paced step's, or a blocked turn's wait for the
    /// human's answer and the rewrite after it.
    pub human_paced: bool,
    /// When the turn started, as iso_utc writes it; None when not known, as for a file written while no orchestrator
    /// had prompted for it.
    #[serde(deserialize_with = "Option::deserialize")]
    pub started_at: Option<String>,
    /// When its file was written (its mtime, never before started_at); None while the turn is under way.
    #[serde(deserialize_with = "Option::deserialize")]
    pub ended_at: Option<String>,
    /// ended_at less started_at, in seconds; None while either is not known.
    #[serde(deserialize_with = "Option::deserialize")]
    pub seconds: Option<i64>,
    /// The fields of the record this engine does not know, as read, written after its own.
    #[serde(flatten)]
    pub unknown: Object,
}

impl AgentRecord {
    /// The record of an agent just started as name, in pane, in harness: its session not yet known.
    pub fn new(name: &str, pane: &str, harness: &str) -> AgentRecord {
        let (name, pane, harness) = (name.into(), pane.into(), harness.into());
        AgentRecord { name, pane, harness, session: None, unknown: Object::new() }
    }
}

/// Reads a harness's name, refusing an empty one: the engine never writes it, and a reader would take it for no
/// harness (a quality analysis's) and skip the turn's tokens.
fn harness_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let name = String::deserialize(d)?;
    if name.is_empty() {
        return Err(serde::de::Error::custom("an empty harness name"));
    }
    Ok(name)
}

/// harness_name's, for a field that is null when no harness ran; the field must be present.
fn optional_harness_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    struct Name(#[serde(deserialize_with = "harness_name")] String);
    Ok(Option::<Name>::deserialize(d)?.map(|Name(n)| n))
}

impl TurnRecord {
    /// Whether the turn is under way: its file has not been found written.
    pub fn under_way(&self) -> bool {
        self.ended_at.is_none()
    }
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
            next_harnesses: Map::new(),
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

    /// The state of a saved state.json object that from_saved refuses, as far as it reads: each of its fields that
    /// reads as RunState's field of that name, every other field at new's defaults, and the fields RunState does not
    /// have in unknown. For showing a run of another format (SavedRun::shown), never for acting on it.
    fn from_readable_fields(saved: &Object) -> RunState {
        let Value::Object(mut fields) = RunState::new("", "", "").to_json() else { unreachable!("a run state is an object") };
        // RunState writes every field it has, format included, so a blank state's keys are all it knows.
        let known: std::collections::HashSet<String> = fields.keys().cloned().collect();
        fields.shift_remove("format");
        let reads = |fields: &Object| serde_json::from_value::<RunState>(Value::Object(fields.clone())).is_ok();
        // One field at a time, so a field that does not read leaves the others.
        for (key, value) in saved {
            if let Some(default) = fields.insert(key.clone(), value.clone()) {
                if !reads(&fields) {
                    fields.insert(key.clone(), default);
                }
            } else {
                fields.shift_remove(key);
            }
        }
        let Ok(mut state) = serde_json::from_value::<RunState>(Value::Object(fields)) else {
            unreachable!("every field is new's or one that reads")
        };
        state.unknown = saved.iter().filter(|(k, _)| !known.contains(k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        state
    }

    /// The harness the role's agent runs in: its started agent's, else the one its next agent starts in
    /// (next_harnesses), else DEFAULT_HARNESS.
    pub fn harness(&self, role: &str) -> &str {
        match self.agents.get(role) {
            Some(agent) => &agent.harness,
            None => self.next_harnesses.get(role).map_or(DEFAULT_HARNESS, String::as_str),
        }
    }

    /// Whether the run is done: phase DONE, though its orchestrator may still be ending it (crate::runs, health).
    pub fn done(&self) -> bool {
        self.phase == DONE
    }

    /// The run's workflow, from the definition and prompts it saved, as resume and list take it. Errors: the saved
    /// prompts cannot be used (prompts), or the workflow cannot be loaded (run_pipeline).
    pub fn pipeline(&self) -> Result<Pipeline, OrchestratorError> {
        run_pipeline(&self.workflow, self.workflow_definition.as_ref(), &self.prompts()?)
    }

    /// Whether the run, of workflow pipeline, is in its quality loop or in the step the gate judges, where it has a
    /// quality round.
    pub fn in_quality(&self, pipeline: &Pipeline) -> bool {
        self.phase == QUALITY || pipeline.gated().is_some_and(|gated| gated.id == self.phase)
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

    /// What a step's prompt tells the agent the change is: the diff against the base plus the untracked files, or for
    /// a project that is not a git repository (no base), the Builder's report. Both git commands carry the flag that
    /// stops the user's git config from hiding something from the agent, as `git_diff` (host.rs) does for the engine's
    /// own diffs: `--no-relative` against `diff.relative` in a work_dir below the top level, and
    /// `--untracked-files=all` against `status.showUntrackedFiles = no` (with `all`, the files in a new directory are
    /// listed one by one).
    pub fn change_description(&self) -> String {
        match self.base.as_deref().filter(|b| !b.is_empty()) {
            Some(base) => format!(
                "`git diff --no-relative {base}` in {}, plus the untracked files `git status --porcelain --untracked-files=all` lists",
                self.work_dir()
            ),
            None => format!("the files the Builder's report lists, in {} (not a git repository)", self.work_dir()),
        }
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

    /// Quality round q of round n, as a quality file's intro names it: "round 1, quality round 2 of 3".
    pub fn quality_place(&self, n: u32, q: u32) -> String {
        format!("round {n}, quality round {q} of {}", self.max_quality_rounds)
    }

    /// The directory the local gate keeps quality round q of round n's jobs in, one `job-<k>` directory per start
    /// holding that job's reports and judging settings among its files (crate::local_gate::start), beside the
    /// round's quality file.
    pub fn gate_dir(&self, n: u32, q: u32) -> String {
        format!("{}/gate-{n}-{q}", self.dir())
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

/// A run's state.json as read back (Host::run_states): its age, and either its RunState or why its format cannot be
/// read. The commands that act on a run take state(), which refuses an unreadable one; list and show take shown(),
/// which shows one too. Not a Result of RunState and the problem: list would lose an unreadable run's task, phase
/// and round, which the desktop app types as never null (ui/src/api.ts).
#[derive(Clone, Debug, PartialEq)]
pub struct SavedRun {
    /// Seconds since the state.json was last written, by the clock of the host that wrote it.
    age: u64,
    /// The state as saved; for an unreadable run, as much of it as reads (RunState::from_readable_fields).
    state: RunState,
    /// Why the state cannot be read, naming the run and what to do with it; None for a readable one.
    problem: Option<String>,
}

impl SavedRun {
    /// The run whose state.json holds saved, written age seconds ago: unreadable when saved is not of STATE_FORMAT or
    /// a field of it does not read (RunState::from_saved). Errors: saved is not an object with a run_id, a non-empty
    /// string, so that it names no run to show or act on.
    pub fn read(age: u64, saved: Value) -> Result<SavedRun, OrchestratorError> {
        let saved = match saved {
            Value::Object(saved) if saved.get("run_id").and_then(Value::as_str).is_some_and(|id| !id.is_empty()) => saved,
            _ => return Err(OrchestratorError("a run state is a JSON object with its run_id".to_string())),
        };
        Ok(match RunState::from_saved(&saved) {
            Ok(state) => SavedRun { age, state, problem: None },
            Err(e) => SavedRun { age, state: RunState::from_readable_fields(&saved), problem: Some(e.0) },
        })
    }

    /// Seconds since the state.json was last written, by the clock of the host that wrote it.
    pub fn age(&self) -> u64 {
        self.age
    }

    /// The run's state, for a command that acts on the run. Errors: the run is unreadable, saying why (problem).
    pub fn state(&self) -> Result<&RunState, OrchestratorError> {
        match &self.problem {
            None => Ok(&self.state),
            Some(problem) => Err(OrchestratorError(problem.clone())),
        }
    }

    /// The run's state as far as it reads, for what list and show print: for an unreadable run, each field of its
    /// state.json that reads as that field, the rest at RunState::new's defaults. Never to act on.
    pub fn shown(&self) -> &RunState {
        &self.state
    }

    /// Why the run's state cannot be read, naming the run and what to do with it: format_problem's text, or the
    /// field from_saved could not read; None for a readable run.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }
}

/// The run of runs named by its full id, or else by its key (run_key), readable or not; cwd is the project, for the
/// error. Errors: none or several match.
pub fn find_run<'r>(runs: &'r [SavedRun], run_ref: &str, cwd: &str) -> Result<&'r SavedRun, OrchestratorError> {
    fn id(run: &SavedRun) -> &str {
        &run.state.run_id
    }
    let mut matches: Vec<_> = runs.iter().filter(|r| id(r) == run_ref).collect();
    if matches.is_empty() {
        matches = runs.iter().filter(|r| run_key(id(r)) == run_ref).collect();
    }
    match matches[..] {
        [] => Err(OrchestratorError(format!("no run {run_ref} under {cwd}/{RUNS_DIR}"))),
        [one] => Ok(one),
        _ => {
            let ids: Vec<&str> = matches.iter().map(|r| id(r)).collect();
            Err(OrchestratorError(format!("{run_ref} matches several runs ({}); give the full run id", ids.join(", "))))
        }
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
