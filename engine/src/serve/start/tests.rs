//! POST /run's body and the command it is.

use super::*;

fn parsed(body: &str) -> Result<RunRequest, String> {
    RunRequest::parse(body.as_bytes())
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

#[test]
fn a_body_with_only_its_task_leaves_every_flag_to_run() {
    assert_eq!(parsed(r#"{"task": "t"}"#), Ok(RunRequest { task: "t".into(), ..RunRequest::default() }));
    let nulls = r#"{"task": "t", "workflow": null, "agent": null, "model": null, "role_agents": null,
                    "role_models": null, "no_pr": null, "worktree": null}"#;
    assert_eq!(parsed(nulls), Ok(RunRequest { task: "t".into(), ..RunRequest::default() }));
}

#[test]
fn every_field_of_a_body() {
    let body = r#"{"task": "t", "workflow": "quick", "agent": "pi", "model": "m",
                   "role_agents": {"review": "codex", "build": "agy"}, "role_models": {"spec": "s"},
                   "no_pr": true, "worktree": false}"#;
    let expected = RunRequest {
        task: "t".into(),
        workflow: Some("quick".into()),
        agent: Some("pi".into()),
        model: Some("m".into()),
        role_agents: pairs(&[("review", "codex"), ("build", "agy")]),
        role_models: pairs(&[("spec", "s")]),
        no_pr: true,
        worktree: false,
    };
    assert_eq!(parsed(body), Ok(expected));
}

#[test]
fn a_body_of_another_shape_is_refused_naming_what_is_wrong() {
    let cases = [
        ("", "the body must be a JSON object"),
        ("[]", "the body must be a JSON object"),
        ("\u{ff}", "the body must be a JSON object"),
        ("{}", "task must be a string that is not empty"),
        (r#"{"task": 1}"#, "task must be a string that is not empty"),
        (r#"{"task": "t", "model": ""}"#, "model must be a string that is not empty"),
        (r#"{"task": "t", "agent": "vim"}"#, "agent \"vim\" is not one of agy, claude, codex, opencode, pi"),
        (r#"{"task": "t", "no_pr": "yes"}"#, "no_pr must be true or false"),
        (r#"{"task": "t", "role_agents": {"build": "vim"}}"#, "role_agents must be an object of role to one of agy, claude, codex, opencode, pi"),
        (r#"{"task": "t", "role_agents": ["build"]}"#, "role_agents must be an object of role to one of agy, claude, codex, opencode, pi"),
        (r#"{"task": "t", "role_models": {"a=b": "m"}}"#, "role_models must be an object of role to a string that is not empty"),
        (r#"{"task": "t", "role_models": {"": "m"}}"#, "role_models must be an object of role to a string that is not empty"),
        (r#"{"task": "t", "role_models": {"build": ""}}"#, "role_models must be an object of role to a string that is not empty"),
        (r#"{"task": "t\u0000"}"#, "the body's strings must hold no NUL character, which no command's argument can"),
        (r#"{"task": "t", "role_models": {"b\u0000": "m"}}"#, "the body's strings must hold no NUL character, which no command's argument can"),
        (r#"{"task": "t", "spec": "/s.md"}"#,"unknown field \"spec\"; the fields are task, workflow, agent, model, role_agents, role_models, no_pr, worktree"),
    ];
    for (body, why) in cases {
        assert_eq!(parsed(body), Err(why.to_string()), "{body}");
    }
}

#[test]
fn the_command_is_run_with_the_id_the_target_and_each_flag_as_one_word_then_the_task() {
    let mut request = RunRequest { task: "-t".into(), ..RunRequest::default() };
    let target = ["--cwd".to_string(), "~/p".to_string()];
    assert_eq!(request.argv("/bin/o", "20261103-120000-abcdef", &target), [
        "/bin/o", "run", "--run-id=20261103-120000-abcdef", "--cwd", "~/p", "--", "-t"
    ]);
    request = RunRequest {
        task: "t".into(),
        workflow: Some("w".into()),
        agent: Some("pi".into()),
        model: Some("--m".into()),
        role_agents: pairs(&[("b", "codex")]),
        role_models: pairs(&[("b", "x=y")]),
        no_pr: true,
        worktree: true,
    };
    assert_eq!(request.argv("o", "i", &[]), [
        "o", "run", "--run-id=i", "--workflow=w", "--agent=pi", "--model=--m", "--role-agent=b=codex",
        "--role-model=b=x=y", "--no-pr", "--worktree", "--", "t"
    ]);
}

#[test]
fn run_reads_the_command_back_as_the_request() {
    let request = RunRequest {
        task: "--x".into(),
        workflow: Some("default".into()),
        agent: Some("pi".into()),
        model: Some("-m".into()),
        role_agents: pairs(&[("build", "codex")]),
        role_models: pairs(&[("review", "x=y")]),
        no_pr: true,
        worktree: true,
    };
    let argv = request.argv("orchestrator", "20261103-120000-abcdef", &["--cwd".to_string(), "/p".to_string()]);
    let parsed = crate::cli::parse_args(argv[1..].iter().map(std::ffi::OsString::from), &ConfigDir::at("/nowhere"));
    let Ok(crate::cli::Command::Run(args)) = parsed else { panic!("not a run: {parsed:?}") };
    assert_eq!((args.task.as_deref(), args.run_id.as_deref()), (Some("--x"), Some("20261103-120000-abcdef")));
    assert_eq!((args.no_pr, args.starts.worktree, args.starts.workflow.as_str()), (true, true, "default"));
    assert_eq!((args.settings.agent.as_deref(), args.settings.model.as_deref()), (Some("pi"), Some("-m")));
    assert_eq!((args.settings.role_agent, args.settings.role_model), (pairs(&[("build", "codex")]), pairs(&[("review", "x=y")])));
    assert_eq!(args.target.cwd.as_deref(), Some("/p"));
}
