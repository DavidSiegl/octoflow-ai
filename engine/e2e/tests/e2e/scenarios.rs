//! The scenarios: each a function that scripts a World and runs the engine in it, listed in main.rs's scenarios!.
//!
//! A scenario says only what happens: what each role's agent writes when prompted, what gh answers, and which engine
//! commands run, in which order, with which stopped midway. What the engine did in return is the world's record,
//! compared with the recording by main.rs.

use std::time::Instant;

use octoflow_e2e::{array_at, checkout, object, object_at};
use serde_json::Value;
use octoflow_engine::json::Object;

use crate::app_recording::Batch;
use crate::client;
use crate::world::World;

// -- What the agents write ------------------------------------------------------

const SPEC: &str = "# Add a subtract function\n\nAdd `subtract(a, b)` to `src/calc.py`, returning a - b, with a test.\n";
const CALC_WITH_SUBTRACT: &str = "def add(a, b):\n    \"\"\"The sum of a and b.\"\"\"\n    return a + b\n\n\n\
                                  def subtract(a, b):\n    \"\"\"a minus b.\"\"\"\n    return a - b\n";
const TEST_WITH_SUBTRACT: &str = "from calc import add, subtract\n\n\ndef test_add():\n    assert add(1, 2) == 3\n\n\n\
                                  def test_subtract():\n    assert subtract(3, 2) == 1\n";
const TASK: &str = "Add a subtract function";

/// An agent's action (octoflow_e2e::fakes says what one holds), built up from `writes`.
struct Action(Object);

impl Action {
    /// Also make these changes to the working tree, path to text, before writing the action's file.
    fn edit(mut self, files: &[(&str, &str)]) -> Action {
        let edits: Object = files.iter().map(|(path, text)| (path.to_string(), Value::from(*text))).collect();
        self.0.insert("edit".into(), Value::Object(edits));
        self
    }

    /// Do it only once the world releases hold.
    fn hold(mut self, hold: &str) -> Action {
        self.0.insert("hold".into(), Value::from(hold));
        self
    }

    /// Exit the agent afterwards.
    fn exit(mut self) -> Action {
        self.0.insert("exit".into(), Value::Bool(true));
        self
    }
}

impl From<Action> for Value {
    fn from(action: Action) -> Value {
        Value::Object(action.0)
    }
}

/// The action of an agent that writes text to file in the run directory.
fn writes(file: &str, text: &str) -> Action {
    Action(octoflow_engine::json::object([("write", object([(file, Value::from(text))])), ("edit", object([]))]))
}

/// An agent that exits without doing anything.
fn exits() -> Value {
    object([("exit", Value::Bool(true))])
}

fn spec_written() -> Action {
    writes("spec.md", SPEC)
}

/// The Builder's report of round n, answering quality round q when q is not 0, having added subtract and its test.
fn built(n: u32, q: u32) -> Action {
    let file = if q == 0 { format!("build-{n}.md") } else { format!("build-{n}-q{q}.md") };
    writes(&file, &format!("Added subtract and its test (round {n}). `pytest` passes.\n"))
        .edit(&[("src/calc.py", CALC_WITH_SUBTRACT), ("tests/test_calc.py", TEST_WITH_SUBTRACT)])
}

fn reviewed(n: u32, verdict: &str, findings: &str) -> Action {
    writes(&format!("review-{n}.md"), &format!("VERDICT: {verdict}\n\n{findings}\n"))
}

fn approved(n: u32) -> Action {
    reviewed(n, "APPROVE", "None.")
}

/// The product team's Product Manager accepting the change of round n.
fn accepted(n: u32) -> Action {
    writes(&format!("acceptance-{n}.md"), "VERDICT: APPROVE\n\nNone.\n")
}

/// The path of the example workflow name in this checkout.
fn example(name: &str) -> String {
    checkout().join("examples/workflows").join(name).to_string_lossy().into_owned()
}

/// The default workflow's three roles, approved in round 1.
fn approved_roles(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0)]);
    w.agent("review", [approved(1)]);
}

// -- The default workflow ------------------------------------------------------------

/// The default workflow, approved in its first round, ending in a pull request.
pub fn default_approved(w: &mut World) {
    approved_roles(w);
    w.run(&["run", "--", TASK]);
}

/// The Reviewer requests changes in round 1 and approves round 2's fix.
pub fn default_changes_then_approved(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0).edit(&[("src/calc.py", CALC_WITH_SUBTRACT)]), built(2, 0)]);
    w.agent("review", [reviewed(1, "CHANGES_REQUESTED", "1. tests/test_calc.py:1: subtract has no test."), approved(2)]);
    w.run(&["run", "--", TASK]);
}

/// Changes requested in every round until --max-rounds: a draft pull request, exit status 3.
pub fn default_max_rounds(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0), built(2, 0)]);
    w.agent(
        "review",
        (1..=2).map(|n| reviewed(n, "CHANGES_REQUESTED", &format!("1. src/calc.py:5: still wrong in round {n}."))),
    );
    w.run(&["run", "--max-rounds", "2", "--", TASK]);
}

/// The Builder's first report is BLOCKED: the human is asked, and the Builder's next report ends the turn.
pub fn blocked_build_answered(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [writes("build-1.md", "BLOCKED: Should subtract accept floats too?\n"), built(1, 0)]);
    w.agent("review", [approved(1)]);
    w.run(&["run", "--", TASK]);
}

/// A review without a VERDICT line is moved aside and asked for once more.
pub fn verdict_retried(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0)]);
    w.agent("review", [writes("review-1.md", "Looks right to me.\n"), approved(1)]);
    w.run(&["run", "--", TASK]);
}

/// run --no-pr: the change stays uncommitted in the checkout and the workspace stays open.
pub fn no_pr_run(w: &mut World) {
    approved_roles(w);
    w.run(&["run", "--no-pr", "--", TASK]);
}

/// The Builder exits without writing its report: the run fails, keeping its workspace and branch.
pub fn agent_exits(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [exits()]);
    w.run(&["run", "--", TASK]);
}

// -- The other workflows ------------------------------------------------------------

/// examples/workflows/quick.toml by --workflow-file: Builder and Reviewer, the task as the contract.
pub fn workflow_quick(w: &mut World) {
    w.agent("build", [built(1, 0)]);
    w.agent("review", [approved(1)]);
    w.run(&["run", "--workflow-file", &example("quick.toml"), "--", TASK]);
}

/// examples/workflows/spec-build-review.toml installed as a workflow file and run by --workflow, with --role-model and
/// --agent.
pub fn workflow_spec_build_review(w: &mut World) {
    w.write("config/ai-agents-orchestrator/workflows/sbr.toml", &read(&example("spec-build-review.toml")));
    approved_roles(w);
    w.run(&[
        "run", "--workflow", "sbr", "--agent", "codex", "--role-model", "review=opus", "--permission-mode", "acceptEdits",
        "--", TASK,
    ]);
}

const TESTS_WEAKENED: &str = "from calc import add, subtract\n\n\ndef test_add():\n    assert add(1, 2) == 3\n";

/// examples/workflows/product-team.toml, approved in round 1 by the Tech Lead and accepted by the Product Manager, its
/// Engineer making engineer_edits.
fn product_team(w: &mut World, engineer_edits: &[(&str, &str)]) {
    w.agent("spec", [spec_written(), accepted(1)]);
    w.agent("design", [writes("design.md", "# Design\n\nAdd subtract beside add in src/calc.py.\n")]);
    w.agent(
        "tests",
        [writes("tests.md", "Acceptance test: tests/test_calc.py::test_subtract.\n")
            .edit(&[("tests/test_calc.py", TEST_WITH_SUBTRACT)])],
    );
    w.agent("build", [built(1, 0).edit(engineer_edits)]);
    w.agent("review", [approved(1)]);
    w.run(&["run", "--workflow-file", &example("product-team.toml"), "--", TASK]);
}

/// examples/workflows/product-team.toml: five roles, the QA Engineer's tests locked, the Engineer leaving them.
pub fn workflow_product_team(w: &mut World) {
    product_team(w, &[("src/calc.py", CALC_WITH_SUBTRACT)]);
}

/// The product team's Engineer weakens the locked acceptance test: it is restored, its diff kept, and the Tech Lead
/// told.
pub fn locks_restored(w: &mut World) {
    product_team(w, &[("src/calc.py", CALC_WITH_SUBTRACT), ("tests/test_calc.py", TESTS_WEAKENED)]);
}

const PROOF_WORKFLOW: &str = r#"description = "Builder <-> Reviewer, the Reviewer's findings backed by proofs"
proof_command = "sh {proofs}"

[roles]
build = "Builder"
review = "Reviewer"

[[steps]]
use = "build"
prompt = "Implement {task} in {cwd}. As your last step, write a report to {build_path}."

[[steps]]
use = "review"
prompt = """Review {change} against the task, {task}. Back each finding with a proof in {proofs_dir}. \
As your last step, write {review_path}, its first line VERDICT: {approve} or VERDICT: {changes}."""
again = "Review the change again; the Builder's report is {build_path}. Write {review_path} as before."
fresh_note = "You are a fresh session; the earlier reviews are {earlier_review_paths}."
"#;
const PROOF: &str = "grep -q 'return a - b' src/calc.py || exit 1\n";

/// A workflow with a proof_command: round 1's review backs its finding with a proof that fails, the proof is run again
/// before round 2's review and passes.
pub fn proofs_run(w: &mut World) {
    let path = w.write("workflows/proofs.toml", PROOF_WORKFLOW);
    let calc_wrong = CALC_WITH_SUBTRACT.replace("return a - b", "return b - a");
    w.agent("build", [built(1, 0).edit(&[("src/calc.py", &calc_wrong)]), built(2, 0)]);
    let review = "VERDICT: CHANGES_REQUESTED\n\n1. src/calc.py:7: the sign is wrong.\n";
    let proof = object([(
        "write",
        object([("proofs-1/test_1_sign.sh", Value::from(PROOF)), ("review-1.md", Value::from(review))]),
    )]);
    w.agent("review", [proof, approved(2).into()]);
    w.run(&["run", "--workflow-file", &path, "--", TASK]);
}

// -- The quality gate ----------------------------------------------------------------

const QUALITY_FAILED: &str = "GATE: ERROR\n\n## Failed conditions\n\n- 1 findings of the `ruff` check; each is listed below.\n\n\
                              ## Findings\n\n1. `src/calc.py:7` error ruff F821: undefined name `c`\n";
/// QUALITY_FAILED's quality.json.
fn quality_failed_json() -> String {
    let finding = ("src/calc.py", 7, "ruff F821", "undefined name `c`");
    report_json("ERROR", &["1 findings of the `ruff` check; each is listed below."], &[finding])
}

/// A quality.json of the gate's schema/quality-v1.json, of gate, its failed conditions and the findings that fail
/// it, (path, line, rule, message) each, with no failing tests; of the fields the engine does not read, only those
/// that say what was judged.
fn report_json(gate: &str, conditions: &[&str], findings: &[(&str, u64, &str, &str)]) -> String {
    let findings: Vec<Value> = findings
        .iter()
        .map(|&(path, line, rule, message)| {
            let fingerprint = octoflow_engine::quality::finding_fingerprint(Some(path), rule, message);
            serde_json::json!({"path": path, "line": line, "severity": "error", "rule": rule,
                               "check": rule.split(' ').next(), "message": message, "fails": true,
                               "fingerprint": fingerprint})
        })
        .collect();
    let report = serde_json::json!({"schema_version": 1, "gate": gate, "base": "<base>", "head": "<head>",
                                    "failed_conditions": conditions, "findings_total": findings.len(),
                                    "findings": findings, "failing_tests": null});
    serde_json::to_string_pretty(&report).expect("a quality.json")
}

const PYPROJECT_WITH_ATTRS: &str =
    "[project]\nname = \"parity-project\"\nversion = \"0.1.0\"\ndependencies = [\"attrs>=25\"]\n";

/// `run --quality-gate local`, the gate World::local_gate pins.
const GATED: [&str; 3] = ["run", "--quality-gate", "local"];

/// GATED, then args.
fn gated<'a>(args: &[&'a str]) -> Vec<&'a str> {
    GATED.iter().copied().chain(args.iter().copied()).collect()
}

/// A judgement of the local gate the scenario scripts (World::local_gate), built up from `judged_ok` or
/// `judged_failed`.
struct Judgement(Object);

impl Judgement {
    /// Keep the gate running until the world releases hold, writing nothing meanwhile.
    fn hold(mut self, hold: &str) -> Judgement {
        self.0.insert("hold".into(), Value::from(hold));
        self
    }
}

impl From<Judgement> for Value {
    fn from(judgement: Judgement) -> Value {
        Value::Object(judgement.0)
    }
}

/// A judgement that writes quality and its quality.json, json, and exits 0, a gate that passed, or 1.
fn judgement(passed: bool, quality: &str, json: &str) -> Judgement {
    Judgement(octoflow_engine::json::object([
        ("quality", Value::from(quality)),
        ("quality_json", Value::from(json)),
        ("exit", Value::from(if passed { 0 } else { 1 })),
    ]))
}

/// The gate passes.
fn judged_ok() -> Judgement {
    judgement(true, "GATE: OK\n\nEvery condition holds.\n", &report_json("OK", &[], &[]))
}

/// The gate fails as QUALITY_FAILED.
fn judged_failed() -> Judgement {
    judgement(false, QUALITY_FAILED, &quality_failed_json())
}

/// --quality-gate local: the first quality round passes; the Builder added a dependency the spec names, which the
/// orchestrator's checks look up on PyPI.
pub fn quality_passes(w: &mut World) {
    w.local_gate([judged_ok()]);
    attrs_added(w);
    w.run(&gated(&["--", TASK]));
}

/// Agents for a run whose spec names attrs, which the Builder adds as a dependency, and whose review approves; attrs
/// on PyPI, where the orchestrator's checks look it up.
fn attrs_added(w: &mut World) {
    let attrs = serde_json::from_str::<Value>(
        r#"{"info": {"version": "25.3.0", "license_expression": "MIT"},
            "releases": {"15.0.0": [{"upload_time_iso_8601": "2015-01-29T00:00:00Z"}],
                         "25.3.0": [{"upload_time_iso_8601": "2025-03-13T00:00:00Z"}]},
            "urls": [{"upload_time_iso_8601": "2025-03-13T00:00:00Z"}]}"#,
    )
    .expect("attrs's PyPI document");
    w.pypi_project("attrs", attrs);
    w.agent("spec", [writes("spec.md", &format!("{SPEC}\nUse attrs for the result type.\n"))]);
    w.agent("build", [built(1, 0).edit(&[("src/calc.py", CALC_WITH_SUBTRACT), ("pyproject.toml", PYPROJECT_WITH_ATTRS)])]);
    w.agent("review", [approved(1)]);
}

/// Quality round 1 fails, the Builder answers it, and quality round 2 passes; nothing is pushed for either.
pub fn quality_fails_then_passes(w: &mut World) {
    w.local_gate([judged_failed(), judged_ok()]);
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0), built(1, 1)]);
    w.agent("review", [approved(1)]);
    w.run(&gated(&["--", TASK]));
}

/// Quality rounds 1 and 2 fail on the same finding: the loop stops and the Reviewer is told it is stuck.
pub fn quality_stuck(w: &mut World) {
    w.local_gate([judged_failed(), judged_failed()]);
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0), built(1, 1)]);
    w.agent("review", [reviewed(1, "CHANGES_REQUESTED", "1. src/calc.py:7: the gate still fails."), built(2, 0)]);
    w.run(&gated(&["--max-rounds", "1", "--", TASK]));
}

/// A workflow without a verdict step whose last quality round fails: QUALITY_GATE_FAILED, exit status 3.
pub fn quality_workflow_without_review(w: &mut World) {
    let path = solo_workflow(w);
    w.local_gate([judged_failed()]);
    w.agent("build", [built(1, 0)]);
    w.run(&gated(&["--max-quality-rounds", "1", "--workflow-file", &path, "--", TASK]));
}

/// Write a workflow of the Builder alone, with no verdict step; its path.
fn solo_workflow(w: &mut World) -> String {
    w.write(
        "workflows/solo.toml",
        "description = \"Builder alone\"\n\n[roles]\nbuild = \"Builder\"\n\n[[steps]]\nuse = \"build\"\n\
         prompt = \"Implement {task} in {cwd}; report to {build_path}.\"\n\
         again = \"Do it again; report to {build_path}.\"\nfresh_note = \"You are a fresh session.\"\n",
    )
}

/// A run started without the quality gate, killed in its build, resumed with --quality-gate local: the Builder's
/// report is judged before the Reviewer starts.
pub fn gate_turned_on_by_resume(w: &mut World) {
    w.local_gate([judged_ok()]);
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0).hold("build")]);
    w.agent("review", [approved(1)]);
    killed_and_resumed_with(w, "build", &["run", "--", TASK], &["--quality-gate", "local"]);
}

/// A run killed while the local gate judges its first quality round, resumed with --no-quality-gate: the job the kill
/// left is stopped, and the Reviewer is told nothing judged the change.
pub fn gate_turned_off_by_resume(w: &mut World) {
    w.local_gate([judged_ok().hold("quality")]);
    approved_roles(w);
    killed_held_and_resumed(w, "quality", &gated(&["--", TASK]), &["--no-quality-gate"]);
}

// -- Where the run works -------------------------------------------------------------

/// run --worktree: the run builds in a worktree of its own from origin's main, which is removed once the pull request is
/// open; the checkout itself, dirty, is left alone.
pub fn worktree_run(w: &mut World) {
    w.write("project/notes.txt", "the human's own uncommitted work\n");
    approved_roles(w);
    w.run(&["run", "--worktree", "--", TASK]);
}

/// run --machine: herdr is addressed by the machine, and every file and git command goes over ssh, which the fake runs
/// here.
pub fn machine_run(w: &mut World) {
    w.machine("box");
    approved_roles(w);
    let project = w.project.to_string_lossy().into_owned();
    w.run(&["run", "--machine", "box", "--cwd", &project, "--", TASK]);
}

/// origin's main changes the same lines while the run reviews: publishing finds the conflict, pushes the branch without
/// the merge and opens a draft pull request, exit status 4.
pub fn publish_with_conflict(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0)]);
    w.agent("review", [approved(1).hold("review")]);
    let run = w.start(&["run", "--", TASK], true);
    w.wait_reached("review");
    let upstream = "def add(a, b):\n    \"\"\"a plus b.\"\"\"\n    return b + a\n\n\ndef subtract(a, b):\n    return -(b - a)\n";
    w.push_to_origin(&[("src/calc.py", upstream)], "Add subtract upstream");
    w.release("review");
    w.invocation(run).wait();
}

// -- Stopped and resumed ------------------------------------------------------------

/// Run the engine with args until it reaches hold, kill it there, then let hold go and resume the run with --force,
/// which a run whose orchestrator died a moment ago needs: its heartbeat is still fresh.
fn killed_and_resumed(w: &mut World, hold: &str, args: &[&str]) {
    killed_and_resumed_with(w, hold, args, &[]);
}

/// killed_and_resumed, the resume given resume_flags before the run's key.
fn killed_and_resumed_with(w: &mut World, hold: &str, args: &[&str], resume_flags: &[&str]) {
    let run = w.start(args, true);
    w.wait_reached(hold);
    w.invocation(run).kill();
    w.release(hold);
    let key = w.run_key(1);
    w.run(&[&["resume", "--force"][..], resume_flags, &[key.as_str()]].concat());
}

/// killed_and_resumed_with, hold never released: what waits on it still waits when the resume starts, as the local
/// gate's job, which runs detached from the engine, does after the kill, where releasing it would race the resume.
fn killed_held_and_resumed(w: &mut World, hold: &str, args: &[&str], resume_flags: &[&str]) {
    let run = w.start(args, true);
    w.wait_reached(hold);
    w.invocation(run).kill();
    let key = w.run_key(1);
    w.run(&[&["resume", "--force"][..], resume_flags, &[key.as_str()]].concat());
}

/// Killed while the Spec Collector interviews; resumed, the interview's file is found written.
pub fn kill_in_spec(w: &mut World) {
    w.agent("spec", [spec_written().hold("spec")]);
    w.agent("build", [built(1, 0)]);
    w.agent("review", [approved(1)]);
    killed_and_resumed(w, "spec", &["run", "--", TASK]);
}

/// Killed while the Builder works.
pub fn kill_in_build(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0).hold("build")]);
    w.agent("review", [approved(1)]);
    killed_and_resumed(w, "build", &["run", "--", TASK]);
}

/// Killed while the Reviewer works, in round 2.
pub fn kill_in_review(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0), built(2, 0)]);
    w.agent("review", [reviewed(1, "CHANGES_REQUESTED", "1. src/calc.py:5: no docstring."), approved(2).hold("review")]);
    killed_and_resumed(w, "review", &["run", "--", TASK]);
}

/// Killed while gh opens the pull request, after the commit and the push; resumed, publishing finishes.
pub fn kill_in_publish(w: &mut World) {
    w.hold_pr_create("publish");
    approved_roles(w);
    killed_and_resumed(w, "publish", &["run", "--", TASK]);
}

/// Killed while the local gate judges the quality round's snapshot; resumed, the run stops the job the kill left and
/// judges the round again.
pub fn kill_in_quality(w: &mut World) {
    w.local_gate([judged_ok().hold("quality"), judged_ok()]);
    approved_roles(w);
    killed_held_and_resumed(w, "quality", &gated(&["--", TASK]), &[]);
}

/// examples/workflows/product-team.toml killed in step hold, design or tests, and resumed.
fn product_team_killed(w: &mut World, hold: &str) {
    let held = |action: Action, step: &str| if hold == step { action.hold(step) } else { action };
    w.agent("spec", [spec_written(), accepted(1)]);
    w.agent("design", [held(writes("design.md", "# Design\n\nAdd subtract beside add in src/calc.py.\n"), "design")]);
    let tests = writes("tests.md", "Acceptance test: tests/test_calc.py::test_subtract.\n")
        .edit(&[("tests/test_calc.py", TEST_WITH_SUBTRACT)]);
    w.agent("tests", [held(tests, "tests")]);
    w.agent("build", [built(1, 0).edit(&[("src/calc.py", CALC_WITH_SUBTRACT)])]);
    w.agent("review", [approved(1)]);
    killed_and_resumed(w, hold, &["run", "--workflow-file", &example("product-team.toml"), "--", TASK]);
}

/// The product team's run killed while the Architect works, before the run has a branch.
pub fn kill_in_design(w: &mut World) {
    product_team_killed(w, "design");
}

/// The product team's run killed while the QA Engineer writes the locked tests, its first editing step.
pub fn kill_in_tests(w: &mut World) {
    product_team_killed(w, "tests");
}

/// The Builder exits without its report and the run fails; resume restarts it in its saved session, which it is
/// prompted to continue in.
pub fn resume_after_agent_exit(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [exits(), built(1, 0).into()]);
    w.agent("review", [approved(1)]);
    w.run(&["run", "--", TASK]);
    let key = w.run_key(1);
    w.run(&["resume", &key]);
}

/// resume --force while the run's orchestrator is alive: the new one takes the run over, and the old one stops at its
/// next save without saving.
pub fn takeover(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [built(1, 0).hold("build")]);
    // Held, so that the second still owns the run when the first next looks, rather than having finished it.
    w.agent("review", [approved(1).hold("review")]);
    let first = w.start(&["run", "--", TASK], true);
    w.wait_reached("build");
    let key = w.run_key(1);
    let second = w.start(&["resume", "--force", &key], true);
    let second_pid = i64::from(w.invocation(second).pid());
    w.wait_for(
        || w.run_state(&key).get("owner").and_then(|o| o.get("pid")).and_then(Value::as_i64) == Some(second_pid),
        "the second orchestrator to claim the run",
    );
    // The second has claimed it; once it waits on the Builder too, let the Builder finish.
    w.wait_for(
        || {
            w.ledger.events().iter().any(|e| {
                let by_second = e.get("by").and_then(Value::as_str) == Some("2");
                let words: Vec<&str> =
                    e.get("herdr").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                by_second && words.starts_with(&["agent", "get"])
            })
        },
        "the second orchestrator to wait on the Builder",
    );
    w.release("build");
    w.invocation(first).wait();
    w.wait_reached("review");
    w.release("review");
    w.invocation(second).wait();
}

// -- The commands about runs ------------------------------------------------------------

/// A default run approved in round 1, its pull request open.
fn approved_run(w: &mut World) {
    approved_roles(w);
    w.run(&["run", "--", TASK]);
}

fn merged() -> [(&'static str, Value); 2] {
    [("state", Value::from("MERGED")), ("closedAt", Value::from("2026-01-02T00:00:00Z"))]
}

/// list, list --json with --pr-state, show and metrics, over a finished run and a failed one.
pub fn runs_listed_and_shown(w: &mut World) {
    approved_run(w);
    w.agent("spec", [exits()]);
    w.run(&["run", "--", "Add a multiply function"]);
    w.run(&["list"]);
    w.set_pr(1, &merged());
    w.run(&["list", "--json", "--pr-state"]);
    let (first, second) = (w.run_key(1), w.run_key(2));
    w.run(&["show", &first]);
    w.run(&["show", &second]);
    w.run(&["metrics"]);
}

/// prune once the pull request is merged: --dry-run first, then for real, locally and on origin; then a new run's
/// sweep, which finds nothing left.
pub fn branches_pruned(w: &mut World) {
    approved_run(w);
    w.run(&["prune"]);
    w.set_pr(1, &merged());
    w.run(&["prune", "--dry-run"]);
    w.run(&["prune"]);
    approved_roles(w);
    w.run(&["run", "--no-pr", "--", "Add a subtract function again"]);
}

/// close of a failed --worktree run: its workspace closed, its worktree and its branch, holding no commit, deleted;
/// then resume refuses it, and a second close says it is closed.
pub fn failed_run_closed(w: &mut World) {
    w.agent("spec", [spec_written()]);
    w.agent("build", [writes("build-1.md", "").edit(&[("src/calc.py", CALC_WITH_SUBTRACT)]).exit()]);
    w.run(&["run", "--worktree", "--", TASK]);
    let key = w.run_key(1);
    w.run(&["close", &key]);
    w.run(&["resume", &key]);
    w.run(&["close", &key]);
}

/// close of a finished --no-pr run: the workspace is closed and the checkout keeps the change.
pub fn finished_run_closed(w: &mut World) {
    approved_roles(w);
    w.run(&["run", "--no-pr", "--", TASK]);
    let key = w.run_key(1);
    w.run(&["close", &key]);
}

/// workflows: the list, with a workflow file and a prompt file installed; the default printed as a file; an example by
/// its path; and a workflow file that does not load.
pub fn workflows_listed(w: &mut World) {
    w.write("config/ai-agents-orchestrator/workflows/quick.toml", &read(&example("quick.toml")));
    w.write(
        "config/ai-agents-orchestrator/prompts/BUILD_PROMPT.md",
        "Build {spec_path} in {cwd}; follow the conventions. Report to {build_path}.\n",
    );
    w.run(&["workflows"]);
    w.run(&["workflows", "default"]);
    w.run(&["workflows", &example("product-team.toml")]);
    let broken = w.write("workflows/broken.toml", "[[steps]]\nuse = \"nothing\"\n");
    w.run(&["workflows", &broken]);
}

/// A run with a prompt file: the Builder gets its text, the run saves it, and the commit says so.
pub fn prompt_file_run(w: &mut World) {
    w.write(
        "config/ai-agents-orchestrator/prompts/BUILD_PROMPT.md",
        "Build what {spec_path} says, in {cwd}. As your last step, report to {build_path}.\n",
    );
    approved_run(w);
}

/// refresh after a pull request opened over conflicts: first GitHub still finds it conflicting (exit status 5), then
/// mergeable, so the warning goes from its description, it is marked ready, and the run forgets its conflicts.
pub fn conflict_refreshed(w: &mut World) {
    publish_with_conflict(w);
    w.set_pr(1, &[("mergeable", Value::from("CONFLICTING"))]);
    w.run(&["refresh"]);
    w.set_pr(1, &[("mergeable", Value::from("MERGEABLE"))]);
    let key = w.run_key(1);
    w.run(&["refresh", &key]);
    let pr = w.pr(1);
    w.note("pull request", pr);
}

// -- serve ------------------------------------------------------------------------------

/// The response headers a client of serve reads: content type, CORS and authentication.
const SERVE_HEADERS: [&str; 7] = [
    "Content-Type",
    "Access-Control-Allow-Origin",
    "Access-Control-Allow-Methods",
    "Access-Control-Allow-Headers",
    "Vary",
    "WWW-Authenticate",
    "Cache-Control",
];

/// One request to serve at address, with token as its bearer when given, as {"request", "status", "headers",
/// "body"}: the headers of SERVE_HEADERS it sent, and the body as served, its files sorted by name where it lists a
/// run's files, whose order is by modification time and so by the clock.
fn serve_request(address: &str, token: Option<&str>, target: &str, method: &str, headers: &[(&str, &str)]) -> Value {
    let bearer = token.map(|t| format!("Bearer {t}"));
    let mut sent: Vec<(&str, &str)> = headers.to_vec();
    if let Some(bearer) = &bearer {
        if !sent.iter().any(|(k, _)| *k == "Authorization") {
            sent.push(("Authorization", bearer));
        }
    }
    let response = client::request(address, method, target, &sent);
    let text = String::from_utf8_lossy(&response.body).into_owned();
    let body = if target.starts_with("/files") && response.status == 200 {
        let mut files = serde_json::from_str::<Value>(&text).unwrap_or_else(|e| panic!("serve's {target} is no JSON: {e}"));
        if let Value::Object(fields) = &mut files {
            if let Some(Value::Array(list)) = fields.get_mut("files") {
                list.sort_by(|a, b| str_of(a, "name").cmp(str_of(b, "name")));
            }
        }
        files
    } else {
        Value::from(text)
    };
    let kept = SERVE_HEADERS.iter().filter_map(|k| response.header(k).map(|v| (*k, Value::from(v))));
    object([
        ("request", Value::from(format!("{method} {target}"))),
        ("status", Value::from(response.status)),
        ("headers", Value::Object(octoflow_engine::json::object(kept))),
        ("body", body),
    ])
}

fn str_of<'a>(json: &'a Value, key: &str) -> &'a str {
    json.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Of events, the last record and the last files of each run, and the last problem: what a client that folds the
/// stream, as the desktop app does, holds after them. Files are sorted by name, as serve_request sorts them. An event
/// of any other kind is kept whole, in order, under "other".
fn last_by_run(events: &[(String, Value)]) -> Object {
    let mut out = octoflow_engine::json::object([("records", object([])), ("files", object([]))]);
    for (kind, payload) in events {
        let run = || str_of(payload, "run").to_string();
        match kind.as_str() {
            "record" => {
                let record = payload.get("record").cloned().unwrap_or(Value::Null);
                object_at(&mut out, "records").insert(run(), record);
            }
            "files" => {
                let mut files = payload.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
                files.sort_by(|a, b| str_of(a, "name").cmp(str_of(b, "name")));
                object_at(&mut out, "files").insert(run(), Value::Array(files));
            }
            "problem" => {
                out.insert("problem".into(), payload.get("error").cloned().unwrap_or(Value::Null));
            }
            _ => array_at(&mut out, "other").push(Value::Array(vec![Value::from(kind.as_str()), payload.clone()])),
        }
    }
    out
}

/// text with each character other than a letter, a digit or one of `_.-~/` written as `%XX` of its UTF-8 bytes.
fn quote(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || "_.-~/".contains(c) {
            out.push(c);
        } else {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// One checkpoint of the event stream: what it held, folded (last_by_run), while the engine waited on hold, or once
/// the run was done for None.
fn checkpoint(stream: &client::EventStream, hold: Option<&str>) -> Value {
    let mut point = octoflow_engine::json::object([("waiting on", hold.map_or(Value::Null, Value::from))]);
    for (k, v) in last_by_run(&stream.events()) {
        point.insert(k, v);
    }
    Value::Object(point)
}

/// Start `serve` with args and wait for its first line: the invocation's number, the host:port it listens on and
/// its token.
fn serve_started(w: &mut World, args: &[&str]) -> (usize, String, String) {
    let serve = w.start(&[&["serve"], args].concat(), true);
    w.wait_for(|| w.invocations[serve - 1].stdout().ends_with('\n'), "serve to print its address");
    let first = serde_json::from_str::<Value>(&w.invocation(serve).stdout()).expect("serve's first line is JSON");
    let address = str_of(&first, "url").strip_prefix("http://").expect("serve's URL is http").to_string();
    (serve, address, str_of(&first, "token").to_string())
}

/// End the serve that is invocation serve with a SIGTERM, wait for it and return its exit status.
fn stop_serve(w: &mut World, serve: usize) -> i32 {
    let pid = nix::unistd::Pid::from_raw(w.invocation(serve).pid());
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).unwrap_or_else(|e| panic!("stopping serve: {e}"));
    w.invocation(serve).wait()
}

/// serve: its endpoints and security checks, and an event stream over a run, read at each point where the run waits
/// on an agent, and once it is done.
pub fn serve_endpoints_and_events(w: &mut World) {
    let (serve, address, token) = serve_started(w, &["--origin", "tauri://localhost"]);
    w.normalizer.literal(&token, "<token>");
    w.normalizer.literal(&address, "<serve>");
    let cwd = quote(&w.project.to_string_lossy());
    // The request lines carry the world's root quoted, which is not the root World::new replaces when it holds a
    // character quote escapes, such as the @ of a Jenkins workspace's <job>@tmp, where the Test stage's TMPDIR is.
    let quoted_root = quote(&w.root().to_string_lossy());
    w.normalizer.literal(&quoted_root, "<world>");
    let port = address.rsplit(':').next().expect("serve's port");
    let runs = format!("/runs?cwd={cwd}");
    let tok = Some(token.as_str());
    let mut requests = vec![
        serve_request(&address, None, &runs, "GET", &[]),
        serve_request(&address, tok, &runs, "GET", &[("Host", &format!("evil.example:{port}"))]),
        serve_request(&address, tok, &runs, "GET", &[("Origin", "https://evil.example")]),
        serve_request(&address, None, &runs, "OPTIONS", &[("Origin", "tauri://localhost")]),
        serve_request(&address, tok, &runs, "GET", &[("Origin", "tauri://localhost")]),
        serve_request(&address, None, &format!("{runs}&token={token}"), "GET", &[("Host", &address)]),
        serve_request(&address, tok, "/runs", "GET", &[]),
        serve_request(&address, tok, &format!("/nothing?cwd={cwd}"), "GET", &[]),
    ];
    w.agent("spec", [spec_written().hold("spec")]);
    w.agent("build", [built(1, 0).hold("build")]);
    w.agent("review", [approved(1).hold("review")]);
    let mut stream = client::EventStream::open(&address, &format!("/events?cwd={cwd}&token={token}"));
    let mut checkpoints = Vec::new();
    let run = w.start(&["run", "--", TASK], true);
    for hold in ["spec", "build", "review"] {
        w.wait_reached(hold);
        let t = Instant::now();
        w.wait_for(|| stream.settled_since(t), &format!("the event stream to settle at {hold}"));
        checkpoints.push(checkpoint(&stream, Some(hold)));
        w.release(hold);
    }
    w.invocation(run).wait();
    let t = Instant::now();
    w.wait_for(|| stream.settled_since(t), "the event stream to settle at the end");
    checkpoints.push(checkpoint(&stream, None));
    stream.close();
    let run_id = str_of(&w.run_state(&w.run_key(1)), "run_id").to_string();
    let file = |name: &str| format!("/file?cwd={cwd}&run={run_id}&name={name}");
    for target in [
        runs.clone(),
        format!("/files?cwd={cwd}&run={run_id}"),
        file("spec.md"),
        file("state.json"),
        file("..%2Fx"),
        file("nothing.md"),
        format!("/files?cwd={cwd}"),
    ] {
        requests.push(serve_request(&address, tok, &target, "GET", &[]));
    }
    // Its exit status is in the scenario's record.
    stop_serve(w, serve);
    w.note("requests", Value::Array(requests));
    let content_type = stream.content_type.clone().map_or(Value::Null, Value::from);
    w.note(
        "event stream",
        object([
            ("status", Value::from(stream.status)),
            ("content type", content_type),
            ("checkpoints", Value::Array(checkpoints)),
        ]),
    );
}

// -- The desktop app's recording ---------------------------------------------------------

/// The task of the runs the desktop app's tests are shown.
const APP_TASK: &str = "add a token-bucket rate limiter to the API client";

/// The run the desktop app's tests replay (crate::app_recording): serve's event stream over a project whose first
/// run is finished and approved, with a pull request, when the stream opens, and whose second run goes from its
/// interview to its review, its Builder blocking once; then the second run's spec.md is edited and the first run
/// deleted. Returns what the stream sent after each of those steps, once it had settled there: the batches, in order,
/// as serve sent them. Both runs are over and serve is stopped when it returns.
pub fn app_event_stream(w: &mut World) -> Vec<Batch> {
    let spec = || writes("spec.md", "# Add a token-bucket rate limiter\n\n- 10 requests a second\n");
    w.agent("spec", [spec()]);
    w.agent("build", [built(1, 0)]);
    w.agent("review", [approved(1)]);
    w.run(&["run", "--", APP_TASK]);
    let (serve, address, token) = serve_started(w, &[]);
    let target = format!("/events?cwd={}&token={token}", quote(&w.project.to_string_lossy()));
    let mut stream = client::EventStream::open(&address, &target);

    let mut batches: Vec<Batch> = Vec::new();
    // Closes the batch after a step: the events since the batch before, once a poll has sent nothing more.
    let mut settle = |w: &World, after: &'static str| {
        let t = Instant::now();
        w.wait_for(|| stream.settled_since(t), &format!("the event stream to settle after: {after}"));
        let sent: usize = batches.iter().map(|b| b.events.len()).sum();
        batches.push(Batch { after, events: stream.events().split_off(sent) });
    };
    settle(w, "the stream opened");
    w.agent("spec", [spec().hold("spec")]);
    let question = writes("build-1.md", "BLOCKED: should the limit be per client or global?\n");
    w.agent("build", [question.hold("build"), built(1, 0).hold("answer")]);
    w.agent("review", [approved(1).hold("review")]);
    let run = w.start(&["run", "--", APP_TASK], true);
    for (hold, after) in [
        ("spec", "a run started its interview"),
        ("build", "the spec was written and the build started"),
        ("answer", "the Builder blocked"),
        ("review", "the build was reported and the review started"),
    ] {
        w.wait_reached(hold);
        settle(w, after);
        if hold != "review" {
            w.release(hold);
        }
    }
    let run_dir = |n: usize| octoflow_e2e::run_dir(&w.project, &w.run_key(n));
    let edited = run_dir(2).join("spec.md");
    std::fs::write(&edited, "# Add a token-bucket rate limiter\n\n- 20 requests a second\n")
        .unwrap_or_else(|e| panic!("editing {}: {e}", edited.display()));
    settle(w, "the spec was edited");
    let deleted = run_dir(1);
    std::fs::remove_dir_all(&deleted).unwrap_or_else(|e| panic!("deleting {}: {e}", deleted.display()));
    settle(w, "the finished run was deleted");

    // Nothing compares this world's record, where the other scenarios' exit statuses are held.
    w.release("review");
    assert_eq!(w.invocation(run).wait(), 0, "the second run's exit status");
    stream.close();
    assert_eq!(stop_serve(w, serve), 0, "serve's exit status");
    batches
}

// -- The other commands -----------------------------------------------------------------

/// A spec as drain reads one.
fn drain_spec(title: &str, blocked: &str, goal: &str) -> String {
    format!("# {title}\n\nStatus: ready-for-agent\n{blocked}\n## Goal\n\n{goal}\n")
}

/// drain over two specs, the second blocked by the first: --dry-run, then a run of the first, which ends in review,
/// leaving the second waiting (exit status 5); the specs' headers are noted.
pub fn specs_drained(w: &mut World) {
    let specs = [
        ("a-subtract", drain_spec(TASK, "", "Add `subtract(a, b)` to src/calc.py.")),
        ("b-multiply", drain_spec("Add a multiply function", "Blocked by: a-subtract\n", "Add `multiply(a, b)`.")),
    ];
    for (slug, text) in &specs {
        w.write(&format!("specs/{slug}/spec.md"), text);
    }
    w.agent("build", [built(1, 0)]);
    w.agent("review", [approved(1)]);
    let dir = format!("{}/specs", w.root().display());
    w.run(&["drain", "--dry-run", &dir]);
    w.run(&["drain", &dir]);
    w.set_pr(1, &merged());
    w.run(&["drain", "--dry-run", &dir]);
    let notes: Object =
        specs.iter().map(|(slug, _)| (slug.to_string(), Value::String(read(&format!("{dir}/{slug}/spec.md"))))).collect();
    w.note("specs", Value::Object(notes));
}

/// doctor here, with --cwd, and on a machine: what it asks herdr and gh, and its exit status. Its lines are noted
/// without the version of git, which is the host's.
pub fn doctor_checks(w: &mut World) {
    w.machine("box");
    let project = w.project.to_string_lossy().into_owned();
    for args in [
        &["doctor"][..],
        &["doctor", "--cwd", &project],
        &["doctor", "--machine", "box", "--cwd", &project],
        &["doctor", "--machine", "nowhere", "--cwd", &project],
    ] {
        let n = w.start(args, false);
        w.invocation(n).wait();
    }
    let lines: Vec<Value> =
        w.invocations.iter().map(|inv| octoflow_e2e::strings(&host_free(&inv.stdout()))).collect();
    w.note("lines", Value::Array(lines));
}

/// doctor's lines without those of the git and local quality gate checks, and their fixes, which say what
/// the host has.
fn host_free(report: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut skipping = false;
    for line in report.lines() {
        if !line.starts_with(' ') {
            let check = line.split(':').next().unwrap_or_default().split_whitespace().last().unwrap_or_default();
            skipping = ["git", "gate"].contains(&check);
        }
        if !skipping {
            lines.push(line.to_string());
        }
    }
    lines
}

/// run --spec: the spec is copied in as spec.md, the interview skipped, and the task taken from its title.
pub fn spec_file_run(w: &mut World) {
    let path = w.write("specs/subtract.md", SPEC);
    w.agent("build", [built(1, 0)]);
    w.agent("review", [approved(1)]);
    w.run(&["run", "--spec", &path]);
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}
