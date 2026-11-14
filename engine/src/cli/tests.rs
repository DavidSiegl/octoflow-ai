//! main's dispatch on the fixture's fakes (run_command), its exit statuses (finish, ended_status) and the resume hint,
//! with the cases of main that start or resume a run: what the flags, a spec, a workflow file and the quality gate
//! hand the Workflow, and what stops a run before any agent. Parsing alone is crate::cli::args', the gate switch on
//! resume crate::workflow's gate_switch tests', the list crate::cli::listing's, the workflows command
//! crate::cli::workflows', reading a spec crate::spec_file's, and refusing outside herdr engine/tests/program.rs'.
//!
//! A case here starts the run on the fakes and stops it at its first prompt (fixture's Cli::started), reading what
//! reached the Workflow from the state.json it saved, or plays the run to its verdict (Cli::played). The program
//! itself, parsed arguments to exit status, is engine/tests/program.rs's.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::local_gate::LOCAL_GATE;
use crate::constants::APPROVE;
use crate::host::fake::FakeHost;
use serde_json::Value;
use crate::json::ValueExt;
use crate::workflow::fixture::{build_turn, saved_run, then_interrupted, Fixture};
use super::fixture::Cli;
use crate::constants::{CHANGES_REQUESTED, FINISHED};
use crate::test_support::TempDir;
use crate::workflow::fixture::{review_turn, solo_turn, spec_turn};
use crate::workflow_file::{load_workflow_file, workflow_definition, workflow_toml};

/// The command line argv, after the program's name.
fn command(argv: &[&str]) -> Command {
    parse_args(argv.iter().map(OsString::from), &ConfigDir::at("/nowhere")).expect("a command line parse_args takes")
}

/// A fixture whose /proj holds the run state, saved 10^4 s ago, and that state as saved.
fn saved_on_host(state: RunState) -> (Fixture, String) {
    let host = Rc::new(FakeHost::default());
    let text = state.to_text();
    host.state().files.insert(format!("{}/state.json", state.dir()), text.clone());
    host.state().ages.insert(state.run_id.clone(), 10_000);
    (Fixture::on(host, state, vec![]), text)
}

/// run_command of argv on f's fakes, a run's or a resume's.
fn run_on(f: &Fixture, argv: &[&str]) -> Result<Ran, Stop> {
    let config = ConfigDir::at("/nowhere");
    let world = Connected {
        with: f.collaborators(),
        config: &config,
        local_host: "here",
        stdout: &|_| {},
        stderr: &|_| {},
        workflow: &run_workflow,
    };
    match run_command(&command(argv), &world)? {
        Ended::Ran(ran) => Ok(*ran),
        Ended::Status(n) => panic!("{argv:?} ran no workflow, ending with {n}"),
    }
}

/// ended_status of a run that ended so.
fn ran_status(ran: Result<Ran, Stop>, command: &Command) -> i32 {
    ended_status(ran.map(|ran| Ended::Ran(Box::new(ran))), command, &|_| {})
}

#[test]
fn the_cli_refuses_with_exit_1_and_leaves_state_json_alone() {
    let cases = [
        (&["--quality-gate", LOCAL_GATE][..], "is a --no-pr run in /proj itself"),
        (&["--no-quality-gate"][..], "has no quality gate, so --no-quality-gate does not apply"),
    ];
    for (flags, why) in cases {
        let (f, saved) = saved_on_host(saved_run("build", 1, &["spec", "build"]));
        let argv: Vec<&str> = ["resume", "a1b2c3", "--cwd", "/proj"].iter().chain(flags).copied().collect();
        let ended = run_on(&f, &argv);
        let Err(Stop::Failed(e)) = &ended else { panic!("{flags:?} did not fail") };
        assert!(e.0.contains(why), "{e}");
        assert_eq!(ran_status(ended, &command(&argv)), EXIT_ERROR);
        // No Workflow ran, and nothing was written.
        assert!(f.herdr.state().calls.is_empty());
        assert_eq!(f.host.state().files.get(&format!("{}/state.json", f.state.dir())), Some(&saved));
    }
}

#[test]
fn the_cli_runs_the_workflow_with_the_gate_turned_on() {
    let mut state = saved_run("build", 1, &["spec", "build", "review"]);
    state.pull_request = true;
    state.base_branch = Some("main".into());
    state.branch = Some("orchestrator/x-a1b2c3".into());
    let (f, _) = saved_on_host(state);
    let f = f.gated();
    // The Builder, played by no turn, exits: the run fails once its Workflow has saved its state.
    let argv = ["resume", "a1b2c3", "--cwd", "/proj", "--quality-gate", LOCAL_GATE, "--max-quality-rounds", "2"];
    assert!(matches!(run_on(&f, &argv), Err(Stop::Failed(_))));
    let saved = f.saved();
    let gate = (Some(&Value::from(LOCAL_GATE)), Some(&Value::from(2)));
    assert_eq!((saved.get("quality_job"), saved.get("max_quality_rounds")), gate);
}

#[test]
fn a_run_ends_with_its_verdict_and_its_pull_request() {
    let mut state = saved_run("done", 1, &["spec", "build", "review"]);
    state.verdict = Some(APPROVE.into());
    state.pr_url = Some("https://github.com/o/r/pull/7".into());
    let (f, _) = saved_on_host(state);
    let Ok(ran) = run_on(&f, &["resume", "a1b2c3", "--cwd", "/proj"]) else {
        panic!("a done run resumes to its verdict")
    };
    assert_eq!((ran.verdict.as_deref(), ran.state.pr_url.as_deref()), (Some(APPROVE), Some("https://github.com/o/r/pull/7")));
    assert_eq!(ran_status(Ok(ran), &command(&["resume", "a1b2c3"])), 0);
}

#[test]
fn a_ctrl_c_outside_the_workflow_is_the_interrupt() {
    // Before the run's state exists: the note names no run.
    let (f, _) = saved_on_host(saved_run("build", 1, &["spec", "build"]));
    f.interrupt.raise();
    let argv = ["resume", "b0b0b0", "--cwd", "/proj"];
    let ended = run_on(&f, &argv);
    assert!(matches!(ended, Err(Stop::Interrupted(None))));
    assert_eq!(ran_status(ended, &command(&argv)), EXIT_INTERRUPTED);
    // Once it exists, before its Workflow starts: the note names it.
    let ended = run_on(&f, &["resume", "a1b2c3", "--cwd", "/proj"]);
    let Err(Stop::Interrupted(Some(run_id))) = ended else { panic!("not interrupted with its run") };
    assert_eq!(run_id, "20260929-120000-a1b2c3");
    assert!(f.herdr.state().calls.is_empty());
}

#[test]
fn a_ctrl_c_in_the_workflow_names_the_run_it_stopped() {
    let (f, _) = saved_on_host(saved_run("build", 1, &["spec", "build"]));
    f.play("build", vec![then_interrupted(&f.interrupt, build_turn(1))]);
    let argv = ["resume", "a1b2c3", "--cwd", "/proj"];
    let Err(Stop::Interrupted(Some(run_id))) = run_on(&f, &argv) else {
        panic!("the run was not interrupted")
    };
    assert_eq!(run_id, "20260929-120000-a1b2c3");
    assert_eq!(f.saved().get("error"), Some(&Value::from("interrupted")));
}

#[test]
fn finish_names_the_verdict_and_the_exit_status() {
    let mut state = saved_run("done", 2, &[]);
    let pipeline = crate::pipeline::DEFAULT_WORKFLOW.clone();
    assert_eq!(finish(APPROVE, &state, &pipeline, &|_| {}), 0);
    assert_eq!(finish("FINISHED", &state, &pipeline, &|_| {}), 0);
    assert_eq!(finish("CHANGES_REQUESTED", &state, &pipeline, &|_| {}), EXIT_CHANGES_REQUESTED);
    assert_eq!(finish("QUALITY_GATE_FAILED", &state, &pipeline, &|_| {}), EXIT_CHANGES_REQUESTED);
    state.conflicts = vec!["calc.py".into()];
    assert_eq!(finish(APPROVE, &state, &pipeline, &|_| {}), EXIT_CONFLICT);
    assert_eq!(finish("CHANGES_REQUESTED", &state, &pipeline, &|_| {}), EXIT_CHANGES_REQUESTED);
}

#[test]
fn the_resume_hint() {
    let target = ["--machine".to_string(), "box".to_string(), "--cwd".to_string(), "~/my project".to_string()];
    assert_eq!(resume_command("20260929-120000-a1b2c3", &target), "orchestrator resume a1b2c3 --machine box --cwd '~/my project'");
    assert_eq!(
        interrupted_note(Some("20260929-120000-a1b2c3"), &[]),
        "interrupted; the role agents keep running in herdr; resume with: orchestrator resume a1b2c3"
    );
    assert_eq!(interrupted_note(None, &target), "interrupted; the role agents keep running in herdr");
    assert_eq!(command(&["run", "--machine", "box", "--cwd", "/p", "t"]).target().unwrap().flags(), target[..2].iter().cloned().chain(["--cwd".into(), "/p".into()]).collect::<Vec<_>>());
}


// -- Runs started through main ---------------------------------------------------------------------------------------

/// The repository's example workflow file of name.
fn example(name: &str) -> String {
    format!("{}/../examples/workflows/{name}.toml", env!("CARGO_MANIFEST_DIR"))
}

/// A Cli whose configuration directory, d, holds files, each a (path in it, text) pair.
fn configured(d: &TempDir, files: &[(&str, &str)]) -> Cli {
    for (path, text) in files {
        let path = d.join(path);
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut cli = Cli::new();
    cli.config = ConfigDir::at(&d.path());
    cli
}

/// The state.json of the new run that `run` with argv started on cli's fakes, stopped at its first prompt.
fn started(cli: &Cli, argv: &[&str]) -> Value {
    match cli.started(argv) {
        (Err(Stop::Failed(e)), Some(saved)) if e.0.contains("at its first prompt") => saved,
        (ended, saved) => panic!("{argv:?} did not start a run: {:?}, {saved:?}", ended.err().map(|e| match e {
            Stop::Failed(e) => e.0,
            Stop::Interrupted(_) => "interrupted".into(),
        })),
    }
}

/// The value of key in saved, as JSON text.
fn field(saved: &Value, key: &str) -> String {
    saved.get(key).unwrap_or(&Value::Null).to_string()
}

/// The error a run of argv in /proj stops with before it starts any agent.
fn refused(cli: &Cli, argv: &[&str]) -> String {
    let argv: Vec<&str> = ["run", "--cwd", "/proj"].iter().chain(argv).copied().collect();
    let e = cli.error(&argv);
    assert!(cli.f.herdr.state().calls.is_empty(), "{argv:?} reached herdr");
    assert_eq!(cli.main(&argv), EXIT_ERROR);
    e
}

const EVERY_ROLE_CLAUDE: &str = r#"{"spec":"claude","build":"claude","review":"claude"}"#;

#[test]
fn permission_mode_and_models_reach_the_workflow() {
    let cli = Cli::new();
    let saved = started(&cli, &["task", "--permission-mode", "auto"]);
    assert_eq!(field(&saved, "permission_mode"), r#""auto""#);
    let saved = started(&cli, &["task", "--permission-mode", "auto", "--model", "sonnet", "--review-model", "opus"]);
    assert_eq!(field(&saved, "models"), r#"{"spec":"sonnet","build":"sonnet","review":"opus"}"#);
    assert_eq!(field(&saved, "permission_mode"), r#""auto""#);
}

#[test]
fn role_models() {
    let cases: [(&[&str], &str); 4] = [
        (&[], "{}"),
        (&["--model", "claude-opus-5-5"], r#"{"spec":"claude-opus-5-5","build":"claude-opus-5-5","review":"claude-opus-5-5"}"#),
        (&["--model", "sonnet", "--review-model", "opus"], r#"{"spec":"sonnet","build":"sonnet","review":"opus"}"#),
        (&["--build-model", "sonnet"], r#"{"build":"sonnet"}"#),
    ];
    for (flags, models) in cases {
        let argv: Vec<&str> = flags.iter().copied().chain(["task"]).collect();
        assert_eq!(field(&started(&Cli::new(), &argv), "models"), models, "{flags:?}");
    }
}

#[test]
fn a_model_reaches_a_role_without_its_own_flag() {
    let d = TempDir::new();
    let cli = configured(&d, &[("four.toml", &workflow_toml(&crate::test_support::four()))]);
    let saved = started(&cli, &["--workflow-file", &d.join("four.toml"), "--model", "X", "--build-model", "Y", "task"]);
    assert_eq!(field(&saved, "models"), r#"{"spec":"X","tests":"X","build":"Y","review":"X"}"#);
}

#[test]
fn no_pr_flag_is_saved_with_the_run() {
    let cli = Cli::new();
    assert_eq!(field(&started(&cli, &["task"]), "pull_request"), "true");
    assert_eq!(field(&started(&cli, &["task", "--no-pr"]), "pull_request"), "false");
}

#[test]
fn run_exit_code_when_changes_remain() {
    let cli = Cli::new();
    let script = vec![
        ("spec", vec![spec_turn()]),
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, CHANGES_REQUESTED)]),
    ];
    let ended = cli.played(&["--no-pr", "--max-rounds", "1", "task"], script);
    let argv = ["run", "task"];
    assert_eq!(cli.status(ended, &argv), EXIT_CHANGES_REQUESTED);
    let out = cli.take_out();
    assert!(out.starts_with("CHANGES_REQUESTED: /proj/.orchestrator/runs/") && out.ends_with("/review-1.md\n"), "{out}");
}

#[test]
fn a_run_without_a_verdict_step_finishes_with_its_last_file() {
    // With --no-pr this run publishes nothing, so its last line names the Builder's file.
    let d = TempDir::new();
    let cli = configured(&d, &[("solo.toml", &workflow_toml(&crate::test_support::solo()))]);
    let ended = cli.played(&["--no-pr", "--workflow-file", &d.join("solo.toml"), "task"], vec![("build", vec![solo_turn()])]);
    assert!(matches!(&ended, Ok(Ended::Ran(ran)) if ran.verdict.as_deref() == Some(FINISHED)));
    assert_eq!(cli.status(ended, &["run", "task"]), 0);
    let out = cli.take_out();
    assert!(out.starts_with("FINISHED: /proj/.orchestrator/runs/") && out.ends_with("/build.md\n"), "{out}");
}

#[test]
fn a_failed_gate_exits_as_changes_requested() {
    // A run that ends QUALITY_GATE_FAILED: its last line and status are finish's.
    let state = saved_run("done", 1, &[]);
    let pipeline = crate::pipeline::DEFAULT_WORKFLOW.clone();
    let line = RefCell::new(String::new());
    let status = finish(crate::constants::QUALITY_GATE_FAILED, &state, &pipeline, &|t| line.borrow_mut().push_str(t));
    assert_eq!(status, EXIT_CHANGES_REQUESTED);
    assert_eq!(*line.borrow(), format!("QUALITY_GATE_FAILED: {}/review-1.md\n", state.dir()));
}

#[test]
fn resume_runs_the_saved_state() {
    let saved = saved_run("review", 1, &["spec", "build", "review"]);
    let (f, _) = saved_on_host(saved.clone());
    f.play("review", vec![review_turn(1, APPROVE)]);
    let cli = Cli::of(f);
    assert_eq!(cli.main(&["resume", "a1b2c3", "--cwd", "/proj"]), 0);
    assert_eq!(cli.take_out(), format!("{APPROVE}: {}/review-1.md\n", saved.dir()));
}

#[test]
fn finished_run_exit_status() {
    let url = "https://github.com/o/r/pull/7";
    let cases = [
        (APPROVE, vec![], 0, format!("{APPROVE}: {url}")),
        (APPROVE, vec!["a.py"], EXIT_CONFLICT, format!("{APPROVE}: {url} (a draft: it conflicts with main)")),
        (CHANGES_REQUESTED, vec!["a.py"], EXIT_CHANGES_REQUESTED, format!("{CHANGES_REQUESTED}: {url} (a draft: it conflicts with main)")),
    ];
    for (verdict, conflicts, status, line) in cases {
        let mut state = saved_run("done", 1, &["spec", "build", "review"]);
        (state.pull_request, state.base_branch, state.verdict) = (true, Some("main".into()), Some(verdict.into()));
        (state.pr_url, state.conflicts) = (Some(url.into()), conflicts.iter().map(|c| c.to_string()).collect());
        let cli = Cli::of(saved_on_host(state).0);
        assert_eq!(cli.main(&["resume", "a1b2c3", "--cwd", "/proj"]), status, "{verdict} {conflicts:?}");
        assert_eq!(cli.take_out(), format!("{line}\n"));
    }
}

#[test]
fn an_interrupted_run_names_its_resume_command() {
    let cli = Cli::new();
    let interrupted = then_interrupted(&cli.f.interrupt, spec_turn());
    let ended = cli.played(&["task"], vec![("spec", vec![interrupted])]);
    let Err(Stop::Interrupted(Some(run_id))) = &ended else { panic!("not interrupted with its run") };
    let key = run_id.rsplit('-').next().unwrap();
    assert_eq!(interrupted_note(Some(run_id), &[]), format!("interrupted; the role agents keep running in herdr; resume with: orchestrator resume {key}"));
    assert_eq!(cli.status(ended, &["run", "task"]), EXIT_INTERRUPTED);
}

#[test]
fn a_resume_on_another_branch_exits_1() {
    let mut state = saved_run("review", 1, &["spec", "build", "review"]);
    (state.pull_request, state.base_branch) = (true, Some("main".into()));
    let branch = "orchestrator/add-a-rate-limiter-a1b2c3";
    state.branch = Some(branch.into());
    let (f, _) = saved_on_host(state);
    let cli = Cli::of(f);
    let e = cli.error(&["resume", "a1b2c3", "--cwd", "/proj"]);
    assert!(e.contains(&format!("check out {branch} and resume")), "{e}");
    assert_eq!(cli.main(&["resume", "a1b2c3", "--cwd", "/proj"]), EXIT_ERROR);
    let saved = cli.f.saved();
    assert!(saved.get("error").map(Value::text).unwrap_or_default().contains("check out"));
    assert_eq!(saved.get("owner"), Some(&Value::Null));
    assert!(cli.f.herdr.state().calls.is_empty());
}

#[test]
fn the_default_workflow_is_the_default() {
    let cli = Cli::new();
    let plain = started(&cli, &["task", "--model", "m"]);
    let named = started(&cli, &["task", "--model", "m", "--workflow", "default"]);
    let same = |saved: &Value| {
        ["workflow", "phase", "models", "agent_kinds", "workflow_definition", "pull_request", "max_rounds"].map(|k| field(saved, k))
    };
    assert_eq!(same(&plain), same(&named));
    assert_eq!((field(&plain, "workflow"), field(&plain, "phase")), (r#""default""#.into(), r#""spec""#.into()));
}

#[test]
fn resuming_an_unknown_workflow() {
    let mut state = saved_run("build", 1, &[]);
    state.workflow = "nope".into();
    let cli = Cli::of(saved_on_host(state).0);
    let e = cli.error(&["resume", "a1b2c3", "--cwd", "/proj"]);
    assert!(e.contains("unknown workflow nope; this orchestrator has default"), "{e}");
}

#[test]
fn resume_limit_against_the_quality_round_of_another_gated_step() {
    let mut state = saved_run("impl", 1, &[]);
    (state.quality_job, state.quality_round) = (Some(LOCAL_GATE.into()), 2);
    let imp = crate::test_support::impl_();
    state.workflow = "impl".into();
    state.workflow_definition = Some(workflow_definition(&imp));
    let cli = Cli::of(saved_on_host(state).0);
    let e = cli.error(&["resume", "a1b2c3", "--cwd", "/proj", "--max-quality-rounds", "2"]);
    assert!(e.contains("already in quality round 2 of its impl phase"), "{e}");
}

#[test]
fn a_workflow_file_is_offered_and_started() {
    let d = TempDir::new();
    let cli = configured(&d, &[("workflows/quick.toml", &workflow_toml(&crate::test_support::quick()))]);
    let saved = started(&cli, &["task", "--workflow", "quick", "--model", "X"]);
    assert_eq!((field(&saved, "workflow"), field(&saved, "phase")), (r#""quick""#.into(), r#""build""#.into()));
    assert_eq!(field(&saved, "models"), r#"{"build":"X","review":"X"}"#);
}

/// A Cli whose configuration has the example product-team workflow file.
fn with_product_team(d: &TempDir) -> Cli {
    configured(d, &[("workflows/product-team.toml", &std::fs::read_to_string(example("product-team")).unwrap())])
}

#[test]
fn run_a_workflow_file_by_name_or_path() {
    let d = TempDir::new();
    let cli = with_product_team(&d);
    let saved = started(&cli, &["task", "--workflow", "product-team"]);
    assert_eq!((field(&saved, "workflow"), field(&saved, "phase")), (r#""product-team""#.into(), r#""spec""#.into()));
    let team = load_workflow_file(&example("product-team"), &crate::prompts::Prompts::default()).unwrap();
    assert_eq!(saved.get("workflow_definition"), Some(&workflow_definition(&team)));
    assert_eq!(field(&saved, "models"), r#"{"tests":"sonnet"}"#);
    let saved = started(&cli, &["task", "--workflow-file", &example("quick")]);
    assert_eq!((field(&saved, "workflow"), field(&saved, "phase")), (r#""quick""#.into(), r#""build""#.into()));
}

#[test]
fn model_flags_override_the_files_models() {
    let d = TempDir::new();
    let cli = with_product_team(&d);
    let saved = started(&cli, &["task", "--workflow", "product-team", "--model", "X"]);
    // The file's models first, then the flags', a flag's replacing the file's for the same role.
    assert_eq!(field(&saved, "models"), r#"{"tests":"X","spec":"X","design":"X","build":"X","review":"X"}"#);
    let flags = ["task", "--workflow", "product-team", "--role-model", "tests=haiku", "--role-model", "spec=opus", "--build-model", "Y"];
    assert_eq!(field(&started(&cli, &flags), "models"), r#"{"tests":"haiku","spec":"opus","build":"Y"}"#);
}

#[test]
fn a_role_flag_for_a_role_the_workflow_lacks() {
    let cli = Cli::new();
    assert_eq!(
        refused(&cli, &["task", "--role-model", "tests=haiku"]),
        "--role-model names role tests, which the workflow does not have; its roles are spec, build, review"
    );
    assert_eq!(
        refused(&cli, &["task", "--role-agent", "tests=pi"]),
        "--role-agent names role tests, which the workflow does not have; its roles are spec, build, review"
    );
}

#[test]
fn resume_takes_role_model() {
    let team = load_workflow_file(&example("product-team"), &crate::prompts::Prompts::default()).unwrap();
    let mut state = saved_run("build", 1, &[]);
    state.models = [("tests", "sonnet".to_string())].into_iter().collect();
    state.workflow = "product-team".into();
    state.workflow_definition = Some(workflow_definition(&team));
    let Value::Object(saved) = state.to_json() else { unreachable!("a state is saved as an object") };
    let resumed = crate::test_support::resumable(&["--role-model", "review=opus"], &saved).unwrap();
    assert_eq!(serde_json::to_string(&resumed.models).unwrap(), r#"{"tests":"sonnet","review":"opus"}"#);
}

/// A given spec with CRLF and trailing blanks, which the run's copy keeps.
const GIVEN_SPEC: &str = "# Add a token-bucket rate limiter\r\n\r\n## Goal\r\nlimit requests  \n\n";

/// The spec the run of saved was seeded with, as the host holds it.
fn seeded(cli: &Cli, saved: &Value) -> Option<String> {
    let dir = format!("/proj/.orchestrator/runs/{}", saved.get("run_id").map(Value::text).unwrap_or_default());
    cli.f.host.state().files.get(&format!("{dir}/spec.md")).cloned()
}

#[test]
fn the_spec_reaches_the_workflow() {
    let d = TempDir::new();
    std::fs::write(d.join("spec.md"), GIVEN_SPEC).unwrap();
    let cli = Cli::new();
    let saved = started(&cli, &["--spec", &d.join("spec.md")]);
    // Round 0 as new_state makes it, which the Workflow saved as round 1 of build when it started that step.
    let fields = ["task", "phase", "round"].map(|k| field(&saved, k));
    assert_eq!(fields, [r#""Add a token-bucket rate limiter""#, r#""build""#, "1"]);
    assert_eq!(seeded(&cli, &saved).as_deref(), Some(GIVEN_SPEC));
    // A given task is used unchanged.
    let saved = started(&cli, &["  the task\nas typed", "--spec", &d.join("spec.md")]);
    assert_eq!(field(&saved, "task"), r#""  the task\nas typed""#);
    // An untitled spec names the task after its file.
    std::fs::write(d.join("rate-limiter.v2.md"), "Limit the requests.\n").unwrap();
    assert_eq!(field(&started(&cli, &["--spec", &d.join("rate-limiter.v2.md")]), "task"), r#""rate-limiter.v2""#);
    // Without --spec, the run starts at the first step, with no spec of its own.
    let saved = started(&cli, &["task"]);
    assert_eq!((field(&saved, "phase"), seeded(&cli, &saved)), (r#""spec""#.into(), None));
}

#[test]
fn a_relative_spec_path_is_the_shells_not_cwds() {
    let d = TempDir::new();
    std::fs::write(d.join("spec.md"), GIVEN_SPEC).unwrap();
    // The spec's path from the test's own current directory, by way of the root.
    let here = std::env::current_dir().unwrap();
    let up = "../".repeat(here.components().count() - 1);
    let relative = format!("{up}{}", d.join("spec.md").trim_start_matches('/'));
    let cli = Cli::new();
    // started's --cwd /proj, overridden, is not where the path is read from either. The fake checkout at /elsewhere
    // has the run's own files as changes, so the run stops before its first prompt, its spec copied.
    let (_, saved) = cli.started(&["--spec", &relative, "--cwd", "/elsewhere"]);
    let saved = saved.expect("the run saved its state");
    assert_eq!(field(&saved, "cwd"), r#""/elsewhere""#);
    let dir = format!("/elsewhere/.orchestrator/runs/{}", saved.get("run_id").unwrap().text());
    assert_eq!(cli.f.host.state().files.get(&format!("{dir}/spec.md")).map(String::as_str), Some(GIVEN_SPEC));
}

#[test]
fn the_spec_combines_with_the_other_run_flags() {
    let d = TempDir::new();
    std::fs::write(d.join("spec.md"), GIVEN_SPEC).unwrap();
    let spec = d.join("spec.md");
    let example = example("spec-build-review");
    let gate = format!(r#""{LOCAL_GATE}""#);
    let cases: [(&[&str], &str, &str); 5] = [
        (&["--no-pr"], "pull_request", "false"),
        (&["--quality-gate", LOCAL_GATE], "quality_job", &gate),
        (&["--machine", "m", "--cwd", "/proj"], "machine", r#""m""#),
        (&["--workflow", "default"], "workflow", r#""default""#),
        (&["--workflow-file", &example], "workflow", r#""spec-build-review""#),
    ];
    for (flags, key, value) in cases {
        let cli = Cli::gated();
        let argv: Vec<&str> = ["--spec", &spec].iter().copied().chain(flags.iter().copied()).collect();
        let saved = started(&cli, &argv);
        assert_eq!((field(&saved, "phase"), field(&saved, key)), (r#""build""#.to_string(), value.to_string()), "{flags:?}");
        let dir = format!("{}/.orchestrator/runs/{}", saved.get("cwd").unwrap().text(), saved.get("run_id").unwrap().text());
        assert_eq!(cli.f.host.state().files.get(&format!("{dir}/spec.md")).map(String::as_str), Some(GIVEN_SPEC), "{flags:?}");
    }
}

#[test]
fn an_unreadable_or_empty_spec_is_an_error() {
    let d = TempDir::new();
    let cli = Cli::new();
    let missing = d.join("nosuch.md");
    assert_eq!(refused(&cli, &["--spec", &missing]), format!("cannot read spec file {missing}: No such file or directory"));
    assert_eq!(refused(&cli, &["--spec", &d.path()]), format!("cannot read spec file {}: Is a directory", d.path()));
    std::fs::write(d.join("spec.bin"), b"\xff\xfe# spec").unwrap();
    assert!(refused(&cli, &["--spec", &d.join("spec.bin")]).starts_with(&format!("spec file {} is not UTF-8 text", d.join("spec.bin"))));
    for text in ["", " \n\t\r\n"] {
        std::fs::write(d.join("blank.md"), text).unwrap();
        assert_eq!(refused(&cli, &["--spec", &d.join("blank.md")]), format!("spec file {} is empty", d.join("blank.md")));
    }
}

#[test]
fn a_spec_needs_a_workflow_with_a_contract_step() {
    let d = TempDir::new();
    std::fs::write(d.join("spec.md"), GIVEN_SPEC).unwrap();
    let e = refused(&Cli::new(), &["--spec", &d.join("spec.md"), "--workflow-file", &example("quick")]);
    assert!(e.contains("workflow quick starts with step build"), "{e}");
    assert!(e.contains("--spec needs a first step that writes the spec"), "{e}");
}

#[test]
fn harness_precedence() {
    let d = TempDir::new();
    let coded = "roles.review = { agent = \"codex\" }\n[[steps]]\nuse = \"spec\"\n[[steps]]\nuse = \"build\"\n\
                 [[steps]]\nuse = \"review\"\n";
    let cli = configured(&d, &[("workflows/coded.toml", coded)]);
    let every = |kind: &str| format!(r#"{{"spec":"{kind}","build":"{kind}","review":"{kind}"}}"#);
    let cases: [(&[&str], String); 8] = [
        (&[], EVERY_ROLE_CLAUDE.into()),
        (&["--agent", "codex"], every("codex")),
        (&["--agent", "agy", "--role-agent", "review=pi"], r#"{"spec":"agy","build":"agy","review":"pi"}"#.into()),
        (&["--role-agent", "build=opencode"], r#"{"spec":"claude","build":"opencode","review":"claude"}"#.into()),
        (&["--workflow", "coded"], r#"{"spec":"claude","build":"claude","review":"codex"}"#.into()),
        (&["--workflow", "coded", "--agent", "agy"], every("agy")),
        (&["--workflow", "coded", "--role-agent", "review=pi"], r#"{"spec":"claude","build":"claude","review":"pi"}"#.into()),
        (&["--workflow", "coded", "--agent", "pi", "--role-agent", "spec=claude"], r#"{"spec":"claude","build":"pi","review":"pi"}"#.into()),
    ];
    for (flags, kinds) in cases {
        let argv: Vec<&str> = flags.iter().copied().chain(["task"]).collect();
        assert_eq!(field(&started(&cli, &argv), "agent_kinds"), kinds, "{flags:?}");
    }
    let saved = started(&cli, &["task", "--agent", "codex", "--role-agent", "spec=claude"]);
    assert_eq!(field(&saved, "agent_kinds"), r#"{"spec":"claude","build":"codex","review":"codex"}"#);
}

#[test]
fn the_gate_reaches_the_workflow() {
    let cli = Cli::gated();
    let plain = started(&cli, &["task"]);
    assert_eq!(field(&plain, "quality_job"), "null");
    let gated = started(&cli, &["task", "--quality-gate", LOCAL_GATE, "--max-quality-rounds", "2"]);
    let gate = (format!(r#""{LOCAL_GATE}""#), "2".to_string());
    assert_eq!((field(&gated, "quality_job"), field(&gated, "max_quality_rounds")), gate);
}

#[test]
fn the_worktree_flag_reaches_the_workflow() {
    let cli = Cli::new();
    let saved = started(&cli, &["task", "--worktree"]);
    let key = saved.get("run_id").unwrap().text().rsplit('-').next().unwrap().to_string();
    assert_eq!(field(&saved, "worktree"), format!(r#""/proj/.orchestrator/worktrees/{key}""#));
    assert_eq!(field(&started(&cli, &["task"]), "worktree"), "null");
}

#[test]
fn bad_prompt_files_stop_a_run_before_any_agent() {
    let cases = [
        ("BUILDER_PROMPT.md", "Build {spec_path}.\n", "BUILDER_PROMPT names no built-in prompt; did you mean BUILD_PROMPT.md?"),
        ("BUILD_PROMPT.md", "Build {spec_path} in {cwd}.\n", "placeholder {build_path} is missing; BUILD_PROMPT takes {build_path}, {cwd}, {spec_path}"),
    ];
    for (name, text, message) in cases {
        let d = TempDir::new();
        let path = format!("prompts/{name}");
        let cli = configured(&d, &[(&path, text)]);
        let e = refused(&cli, &["task"]);
        assert!(e.starts_with(&format!("{}: {message}", d.join(&path))), "{e}");
    }
}

#[test]
fn list_names_a_non_default_workflow() {
    let cli = Cli::new();
    let done = |id: &str, workflow: &str| {
        let mut state = saved_run("done", 1, &[]);
        (state.run_id, state.verdict, state.workflow) = (id.into(), Some(APPROVE.into()), workflow.into());
        state.to_json()
    };
    cli.save(&done("20260930-070000-c0ffe1", "default"), 1_000_000);
    cli.save(&done("20260930-070000-c0ffe2", "default"), 1_000_000);
    cli.save(&done("20260930-070000-c0ffe3", "quick"), 1_000_000);
    let mut impl_run = saved_run("impl", 1, &[]);
    (impl_run.run_id, impl_run.quality_round, impl_run.workflow) = ("20260930-070000-c0ffe4".into(), 2, "impl".into());
    impl_run.workflow_definition = Some(workflow_definition(&crate::test_support::impl_()));
    cli.save(&impl_run.to_json(), 1_000_000);
    assert_eq!(cli.main(&["list", "--cwd", "/proj"]), 0);
    let lines: Vec<String> = cli.lines().into_iter().step_by(2).collect();
    assert_eq!(lines[..3], [
        format!("20260930-070000-c0ffe1  done     round 1  {APPROVE}"),
        format!("20260930-070000-c0ffe2  done     round 1  {APPROVE}"),
        format!("20260930-070000-c0ffe3  done     round 1  {APPROVE}  [quick workflow]"),
    ]);
    assert!(lines[3].starts_with("20260930-070000-c0ffe4  impl     round 1 q2  stale"), "{}", lines[3]);
    assert!(lines[3].ends_with("  [impl workflow]"), "{}", lines[3]);
}

#[test]
fn record_in_state_refuses_a_changed_file_naming_what_it_does_not_record() {
    let path = "/proj/.orchestrator/runs/r1/state.json";
    let read = crate::state::RunState::new("r1", "a task", "/proj");
    let mut now = read.clone();
    now.round = 3;
    let host = FakeHost::default();
    host.state().files.insert(path.to_string(), now.to_text());
    type Change = dyn Fn(&mut crate::state::RunState);
    // (change, the end of the error)
    let cases: [(&Change, &str); 3] = [
        (&|_| {}, "nothing is recorded"),
        (&|s| s.closed = Some("closed".into()), "closed is not recorded"),
        (&|s| { s.closed = Some("closed".into()); s.conflicts = vec!["a".into()]; }, "conflicts and closed are not recorded"),
    ];
    for (change, end) in cases {
        let e = record_in_state(&host, path, &read, change, "it was closed").unwrap_err();
        assert_eq!(e.0, format!("{path} changed while it was closed; {end}"));
    }
    assert!(host.state().writes.is_empty());
}
