//! TestHarnesses, the Workflow's and resumable_state's cases of TestHarnessFlags, and TestWorkflowFileAgents'
//! test_saved_with_the_run: each role's agent started, resumed and saved in its harness.
//!
//! Not here: TestHarnessFlags' cases of parse_args, new_state and main (test_precedence, test_flags_reach_the_workflow,
//! test_role_agent_for_a_role_the_workflow_lacks, test_unknown_kinds_are_argparse_errors), crate::cli's tests' and
//! crate::cli::args'; and TestWorkflowFileAgents' test_workflows_prints_it, crate::cli::workflows'.
//! TestWorkflowFileAgents' other cases, and TestHarnessFlags' of launch_args and of reading a 0.3.0 state, are
//! crate::workflow_file's, crate::harness's and crate::state's (tickets 04 and 06).

use super::worktree::worktree_run;
use crate::test_support::resumable;
use super::*;

const KINDS: &[&str] = &["agy", "claude", "codex", "opencode", "pi"];
const MODES: &[&str] = &["default", "acceptEdits", "bypassPermissions", "plan", "auto", "dontAsk"];

/// The spec's table of what --permission-mode becomes in each harness; None is dropped.
fn permissions(kind: &str, mode: &str) -> Option<Vec<String>> {
    let args: Option<&[&str]> = match (kind, mode) {
        ("claude", _) => return Some(strings(&["--permission-mode", mode])),
        ("agy", "default") => Some(&[]),
        ("agy", "acceptEdits") => Some(&["--mode", "accept-edits"]),
        ("agy", "bypassPermissions") => Some(&["--dangerously-skip-permissions"]),
        ("agy", "plan") => Some(&["--mode", "plan"]),
        ("codex", "default") => Some(&[]),
        ("codex", "acceptEdits") => Some(&["--full-auto"]),
        ("codex", "bypassPermissions") => Some(&["--dangerously-bypass-approvals-and-sandbox"]),
        ("codex", "plan") => Some(&["--sandbox", "read-only"]),
        _ => None,
    };
    args.map(strings)
}

/// The arguments that start an agent of harness kind on model m; opencode takes the model in its config, exported
/// before it starts (opencode_s_config_is_exported_before_it_starts).
fn on_model(kind: &str) -> Vec<String> {
    strings(if kind == "opencode" { &["--standalone"] } else { &["--model", "m"] })
}

/// Every role of the default workflow in harness kind.
fn every_role(kind: &str) -> Map<String> {
    FULL.iter().map(|role| (*role, kind.to_string())).collect()
}

/// AgentSettings of a permission mode, the models and the harnesses by role.
fn agent_settings(permission_mode: Option<&str>, models: &[(&str, &str)], kinds: Map<String>) -> AgentSettings {
    AgentSettings {
        permission_mode: permission_mode.map(String::from),
        models: models.iter().map(|(r, m)| (*r, m.to_string())).collect(),
        kinds,
    }
}

/// The default settings, with agents.
fn with_agents(agents: AgentSettings) -> RunSettings {
    RunSettings { agents, ..RunSettings::default() }
}

/// (role, harness, arguments) of each agent started, in order.
type Started = Vec<(String, String, Vec<String>)>;

/// A whole run of the default workflow with settings: the agents it started, and what it logged.
fn run_default(settings: RunSettings) -> (Started, Vec<String>) {
    let f = Fixture::new(approved());
    assert_eq!(f.workflow(settings).run(), verdict(APPROVE));
    let kinds = f.herdr.state().kinds.clone();
    let kind_of = |name: &str| kinds.iter().find(|(n, _)| n == name).map(|(_, kind)| kind.clone()).expect("a start's kind");
    let started = starts(&f).into_iter().map(|(name, _, args)| (role_of(&name), kind_of(&name), args));
    (started.collect(), f.told.lines())
}

/// The role of an agent named <role>-<key>.
fn role_of(name: &str) -> String {
    name.split('-').next().unwrap().to_string()
}

/// The starts of the default workflow's roles in order, all in harness kind, each with its args.
fn expected_starts(kind: &str, args: [Vec<String>; 3]) -> Started {
    FULL.iter().zip(args).map(|(role, args)| (role.to_string(), kind.to_string(), args)).collect()
}

/// The log lines that say a harness drops the permission mode.
fn dropped(logged: &[String]) -> Vec<String> {
    logged.iter().filter(|l| l.contains("no equivalent")).cloned().collect()
}

#[test]
fn a_new_agent_in_each_harness() {
    for kind in KINDS {
        let (started, _) = run_default(with_agents(agent_settings(None, &[("build", "m")], every_role(kind))));
        assert_eq!(started, expected_starts(kind, [vec![], on_model(kind), vec![]]), "{kind}");
    }
}

#[test]
fn each_permission_mode_in_each_harness() {
    for kind in KINDS {
        for mode in MODES {
            let (started, logged) = run_default(with_agents(agent_settings(Some(mode), &[("review", "m")], every_role(kind))));
            let expected = permissions(kind, mode);
            let args = expected.clone().unwrap_or_default();
            let review = [args.clone(), on_model(kind)].concat();
            assert_eq!(started, expected_starts(kind, [args.clone(), args, review]), "{kind} {mode}");
            let told: Vec<String> = match expected {
                None => FULL
                    .iter()
                    .map(|role| format!("[{role}] {kind} has no equivalent of --permission-mode {mode}; starting it without one"))
                    .collect(),
                Some(_) => vec![],
            };
            assert_eq!(dropped(&logged), told, "{kind} {mode}");
        }
    }
}

#[test]
fn without_a_harness_every_role_is_claude_as_before() {
    let (started, logged) = run_default(with_agents(agent_settings(Some("auto"), &[], Map::new())));
    let auto = strings(&["--permission-mode", "auto"]);
    assert_eq!(started, expected_starts("claude", [auto.clone(), auto.clone(), auto]));
    assert!(dropped(&logged).is_empty());
}

#[test]
fn a_dropped_mode_is_logged_once_per_role() {
    let mut state = saved_run("build", 2, FULL);
    state.agents.get_mut("build").unwrap().kind = "pi".to_string();
    let script = vec![("build", vec![build_turn(2)]), ("review", vec![review_turn(2, APPROVE)])];
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, script, &["review"], &[("build", "s-gone")]);
    f.herdr.state().lost_sessions.insert("s-gone".into());
    let settings = RunSettings { agents: agent_settings(Some("plan"), &[], every_role("pi")), ..f.as_saved() };
    assert_eq!(f.workflow(settings).run(), verdict(APPROVE));
    let args: Vec<Vec<String>> = starts(&f).into_iter().map(|(_, _, args)| args).collect();
    assert_eq!(args, [strings(&["--session", "s-gone"]), vec![]]);
    assert_eq!(dropped(&f.told.lines()).len(), 1);
}

#[test]
fn an_exited_agent_resumes_in_its_own_harness() {
    let resumed = |kind: &str| -> Vec<String> {
        strings(match kind {
            "claude" => &["--permission-mode", "acceptEdits", "--resume", "s-build", "--model", "m"],
            "agy" => &["--mode", "accept-edits", "--conversation", "s-build", "--model", "m"],
            "codex" => &["resume", "s-build", "--full-auto", "--model", "m"],
            "pi" => &["--session", "s-build", "--model", "m"],
            "opencode" => &["--session", "s-build", "--standalone"],
            _ => unreachable!("{kind}"),
        })
    };
    for kind in KINDS {
        let seen = seen();
        let mut state = saved_run("build", 2, FULL);
        state.prompted = Some("build-2.md".into());
        state.agents.get_mut("build").unwrap().kind = kind.to_string();
        let script = vec![("build", vec![recording(build_turn(2), &seen)]), ("review", vec![review_turn(2, APPROVE)])];
        let f = Fixture::resume(Rc::new(FakeHost::default()), state, script, &["review"], &[("build", "s-build")]);
        let settings = RunSettings { agents: agent_settings(Some("acceptEdits"), &[("build", "m")], every_role(kind)), ..f.as_saved() };
        let mut wf = f.workflow(settings);
        assert_eq!(wf.run(), verdict(APPROVE), "{kind}");
        assert_eq!(starts(&f), [("build-a1b2c3".into(), "w1:p2".into(), resumed(kind))], "{kind}");
        assert_eq!(f.herdr.state().kinds, [("build-a1b2c3".to_string(), kind.to_string())]);
        assert_eq!(*seen.borrow(), [continue_prompt(&wf.state().build_path(2))]);
        assert_eq!(wf.state().agents.get("build").unwrap().kind, *kind);
    }
}

#[test]
fn a_saved_run_on_gemini_is_refused_naming_agy() {
    let mut state = saved_run("build", 2, FULL);
    state.prompted = Some("build-2.md".into());
    state.agents.get_mut("build").unwrap().kind = "gemini".to_string();
    let script = vec![("build", vec![build_turn(2)])];
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, script, &["review"], &[("build", "s-build")]);
    let settings = RunSettings { agents: agent_settings(None, &[], every_role("gemini")), ..f.as_saved() };
    let Err(Stopped::Failed(e)) = f.workflow(settings).run() else { panic!("a run on gemini went on") };
    assert!(e.0.contains("agent gemini is not a harness this orchestrator supports any more; agy replaced it"), "{e:?}");
    assert!(f.herdr.state().kinds.is_empty());
}

#[test]
fn a_changed_harness_starts_fresh() {
    // The saved record without a kind is a claude agent's, as 0.3.0 saved it.
    for (saved_kind, kind) in [(None, "codex"), (Some("claude"), "pi"), (Some("gemini"), "claude")] {
        let seen = seen();
        let mut state = saved_run("build", 2, FULL);
        state.prompted = Some("build-2.md".into());
        if let Some(saved_kind) = saved_kind {
            state.agents.get_mut("build").unwrap().kind = saved_kind.to_string();
        }
        let script = vec![("build", vec![recording(build_turn(2), &seen)]), ("review", vec![review_turn(2, APPROVE)])];
        let f = Fixture::resume(Rc::new(FakeHost::default()), state, script, &["review"], &[("build", "s-build")]);
        let kinds = [("build", kind.to_string())].into_iter().collect();
        let settings = RunSettings { agents: agent_settings(None, &[], kinds), ..f.as_saved() };
        assert_eq!(f.workflow(settings).run(), verdict(APPROVE), "{kind}");
        assert_eq!(starts(&f), [("build-a1b2c3".into(), "w1:p2".into(), vec![])]);
        assert_eq!(f.herdr.state().kinds, [("build-a1b2c3".to_string(), kind.to_string())]);
        let first = &seen.borrow()[0];
        assert!(first.contains("You are the Builder"), "{first}");
        assert!(first.contains("This is round 2, and you are a fresh session"), "{first}");
        let told = format!(
            "[build] session s-build is {}'s, and the role runs in {kind} now; starting a fresh one",
            saved_kind.unwrap_or("claude")
        );
        assert!(f.told.lines().contains(&told), "{:?}", f.told.lines());
    }
}

#[test]
fn a_live_agent_keeps_its_harness() {
    let script = vec![("review", vec![review_turn(1, APPROVE)])];
    let f = Fixture::resume(Rc::new(FakeHost::default()), saved_run("review", 1, FULL), script, &["review"], &[]);
    f.put(&f.state.build_path(1), "report 1");
    let settings = RunSettings { agents: agent_settings(None, &[], every_role("codex")), ..f.as_saved() };
    assert_eq!(f.workflow(settings).run(), verdict(APPROVE));
    assert!(f.herdr.state().kinds.is_empty());
    assert!(called(&f, &Call::Prompt { name: "review-a1b2c3".into() }));
}

#[test]
fn every_roles_harness_is_saved() {
    let f = Fixture::new(approved());
    let kinds = [("review", "pi".to_string())].into_iter().collect();
    f.workflow(with_agents(agent_settings(Some("plan"), &[], kinds))).run().unwrap();
    let saved = f.saved();
    assert_eq!(saved.get("agent_kinds").unwrap().to_string(), r#"{"spec":"claude","build":"claude","review":"pi"}"#);
    assert_eq!(text(&saved, "permission_mode").as_deref(), Some("plan"));
    assert!(!saved.contains_key("agent_args"));
    let agents = saved.get("agents").unwrap().as_object().unwrap();
    let recorded: Vec<(&str, &str)> = agents.iter().map(|(role, a)| (role.as_str(), a.get("kind").unwrap().as_str().unwrap())).collect();
    assert_eq!(recorded, [("spec", "claude"), ("build", "claude"), ("review", "pi")]);
}

// -- TestHarnessFlags, the Workflow's cases ------------------------------------------------------------------------

#[test]
fn role_agent_starts_that_role_in_its_harness() {
    // The harnesses new_state gives `run task --agent agy --role-agent review=pi`, as test_precedence (crate::cli's tests)
    // checks.
    let expected = [("spec", "agy"), ("build", "agy"), ("review", "pi")];
    let f = Fixture::new(approved());
    let kinds = expected.iter().map(|(r, k)| (*r, k.to_string())).collect();
    f.workflow(with_agents(agent_settings(None, &[], kinds))).run().unwrap();
    let started: Vec<(String, String)> = f.herdr.state().kinds.iter().map(|(name, kind)| (role_of(name), kind.clone())).collect();
    assert_eq!(started, expected.map(|(r, k)| (r.to_string(), k.to_string())));
}

#[test]
fn resume_keeps_or_overrides_the_saved_harnesses() {
    let mut saved = saved_run("build", 2, FULL).to_json().as_object().unwrap().clone();
    let kinds = r#"{"spec": "claude", "build": "codex", "review": "agy"}"#;
    saved.insert("agent_kinds".into(), serde_json::from_str::<Value>(kinds).unwrap());
    let cases: [(&[&str], Map<String>); 3] = [
        (&[], [("spec", "claude"), ("build", "codex"), ("review", "agy")].map(|(r, k)| (r, k.to_string())).into_iter().collect()),
        (&["--agent", "pi"], every_role("pi")),
        (
            &["--role-agent", "review=opencode"],
            [("spec", "claude"), ("build", "codex"), ("review", "opencode")].map(|(r, k)| (r, k.to_string())).into_iter().collect(),
        ),
    ];
    for (flags, kinds) in cases {
        assert_eq!(resumable(flags, &saved).unwrap().agent_kinds, kinds, "{flags:?}");
    }
}

#[test]
fn resume_with_a_new_harness_relaunches_in_it() {
    let saved = saved_run("review", 2, FULL).to_json().as_object().unwrap().clone();
    let state = resumable(&["--agent", "pi"], &saved).unwrap();
    let agents = agent_settings(state.permission_mode.as_deref(), &[], state.agent_kinds.clone());
    let script = vec![("review", vec![review_turn(2, APPROVE)])];
    let f = Fixture::resume(Rc::new(FakeHost::default()), state, script, &[], &[]);
    f.put(&f.state.build_path(2), "report 2");
    assert_eq!(f.workflow(RunSettings { agents, ..f.as_saved() }).run(), verdict(APPROVE));
    assert_eq!(f.herdr.state().kinds, [("review-a1b2c3".to_string(), "pi".to_string())]);
}

// -- TestWorkflowFileAgents ----------------------------------------------------------------------------------------

#[test]
fn saved_with_the_run() {
    let quick = crate::test_support::quick();
    let mut settings = quick.settings().clone();
    settings.agents.insert("review", "codex".to_string());
    let p = crate::pipeline::Pipeline::with("coded", quick.roles().clone(), quick.steps().to_vec(), settings).unwrap();
    let f = Fixture::on(Rc::new(FakeHost::default()), new_run(&p), vec![
        ("build", vec![build_turn(1)]),
        ("review", vec![review_turn(1, APPROVE)]),
    ]);
    let mut wf = f.workflow(RunSettings { pipeline: Some(p.clone()), pull_request: false, ..RunSettings::default() });
    assert_eq!(wf.run(), verdict(APPROVE));
    let definition = wf.state().workflow_definition.clone().unwrap();
    let review = definition.get("roles").and_then(|r| r.get("review")).unwrap();
    assert_eq!(review.to_string(), r#"{"label":"Reviewer","agent":"codex"}"#);
    let back = crate::workflow_file::run_pipeline("coded", Some(&definition), &crate::prompts::Prompts::default()).unwrap();
    assert_eq!(back, p);
}

#[test]
fn a_run_saved_with_gemini_resumes_only_moved_to_another_harness() {
    // A run 0.67 saved: its workflow's review role and its agent_kinds name gemini (GitHub issue #125).
    let quick = crate::test_support::quick();
    let mut settings = quick.settings().clone();
    settings.agents.insert("review", "codex".to_string());
    let p = crate::pipeline::Pipeline::with("coded", quick.roles().clone(), quick.steps().to_vec(), settings).unwrap();
    let mut definition = crate::workflow_file::workflow_definition(&p);
    definition["roles"]["review"]["agent"] = Value::from("gemini");
    let mut saved = saved_run("build", 2, FULL).to_json().as_object().unwrap().clone();
    saved.insert("workflow".into(), Value::from("coded"));
    saved.insert("workflow_definition".into(), definition);
    saved.insert("agent_kinds".into(), serde_json::from_str(r#"{"build": "claude", "review": "gemini"}"#).unwrap());
    // Its workflow reads, with agy in gemini's place, so that listings show it.
    let pipeline = RunState::from_saved(&saved).unwrap().pipeline().expect("the saved workflow reads");
    assert_eq!(pipeline.agents().get("review").map(String::as_str), Some("agy"));
    // A resume is refused before any turn unless the role moves.
    assert_eq!(
        resumable(&[], &saved).unwrap_err().0,
        format!(
            "run {}: role review: agent gemini is not a harness this orchestrator supports any more; agy replaced it; \
             resume it with --role-agent review=KIND",
            saved.get("run_id").and_then(Value::as_str).unwrap()
        )
    );
    let state = resumable(&["--role-agent", "review=agy"], &saved).unwrap();
    assert_eq!(state.agent_kinds.get("review").map(String::as_str), Some("agy"));
}

#[test]
fn run_dir_granted_in_worktree_runs_only() {
    // TestHarnessFlags.test_run_dir_granted_in_worktree_runs_only, its Workflow half; the launch half is
    // crate::harness's.
    let f = worktree_run(Rc::new(FakeHost::default()), None);
    f.workflow(RunSettings::default()).run().unwrap();
    let args: Vec<Vec<String>> = starts(&f).into_iter().map(|(_, _, args)| args).collect();
    assert_eq!(args, vec![strings(&["--add-dir", &f.state.dir()]); 3]);
    let f = Fixture::new(approved());
    f.workflow(RunSettings::default()).run().unwrap();
    let args: Vec<Vec<String>> = starts(&f).into_iter().map(|(_, _, args)| args).collect();
    assert_eq!(args, vec![Vec::<String>::new(); 3]);
}

#[test]
fn opencode_s_config_is_exported_before_it_starts() {
    let f = worktree_run(Rc::new(FakeHost::default()), None);
    let kinds = [("build", "opencode".to_string())].into_iter().collect();
    let agents = agent_settings(None, &[("build", "m")], kinds);
    f.workflow(RunSettings { agents, ..RunSettings::default() }).run().unwrap();
    let calls = f.herdr.state().calls.clone();
    let exported: Vec<&Call> = calls.iter().filter(|c| matches!(c, Call::Export { .. })).collect();
    let config = format!(r#"{{"model":"m","permission":{{"external_directory":{{"{}/*":"allow"}}}}}}"#, f.state.dir());
    let export = Call::Export { pane: "w1:p2".into(), vars: vec![("OPENCODE_CONFIG_CONTENT".into(), config)] };
    assert_eq!(exported, [&export]);
    let at = |name: &str| calls.iter().position(|c| matches!(c, Call::Start { name: n, .. } if n == name)).unwrap();
    let exported_at = calls.iter().position(|c| *c == export).unwrap();
    assert!(at("spec-a1b2c3") < exported_at && exported_at < at("build-a1b2c3"), "{calls:?}");
}
