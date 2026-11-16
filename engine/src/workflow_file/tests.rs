use super::*;
use crate::pipeline::workflow_shape;
use crate::prompts::Prompt;
use crate::test_support::{locking, oracle, quick, replaced, roles, TempDir};

fn built_in() -> Prompts {
    Prompts::default()
}

fn toml(text: &str) -> Value {
    parse_toml(text).unwrap()
}

/// A configuration directory with a workflows directory, and a writer of workflow files into it.
struct Config {
    dir: TempDir,
    config: ConfigDir,
}

impl Config {
    fn new() -> Self {
        let dir = TempDir::new();
        std::fs::create_dir(dir.join("workflows")).unwrap();
        let config = ConfigDir::at(&dir.join(""));
        Config { dir, config }
    }

    /// The path of the workflow file name.toml written with text.
    fn file(&self, name: &str, text: &str) -> String {
        let path = self.dir.join(&format!("workflows/{name}.toml"));
        std::fs::write(&path, text).unwrap();
        path
    }
}

/// The built-in workflow name, worded by the built-in prompts.
fn built_in_workflow(name: &str) -> Pipeline {
    find_workflow(name, &built_in()).unwrap()
}

// WORKFLOWS is built at compile time, so this is the only run of built_in_names that coverage sees.
#[test]
fn the_built_in_names_are_the_default_then_the_embedded_files() {
    let files: Vec<&str> = BUILT_IN_FILES.iter().map(|(name, _)| *name).collect();
    assert_eq!(built_in_names()[0], "default");
    assert_eq!(built_in_names()[1..], files[..]);
}

#[test]
fn the_built_in_workflows() {
    assert_eq!(WORKFLOWS, ["default", "design", "product-team", "quick"]);
    assert!(WORKFLOWS.is_sorted(), "WORKFLOWS is in name order");
    for name in WORKFLOWS {
        let p = built_in_workflow(name);
        assert_eq!(p.name(), name);
        assert!(is_built_in(&p, &built_in()), "{name}");
    }
    assert_eq!(built_in_workflow("default"), *DEFAULT_WORKFLOW);
    assert!(!is_built_in(&quick(), &built_in()), "the fixture named quick is not the built-in");
    assert_eq!(
        find_workflow("nope", &built_in()).unwrap_err().0,
        "unknown workflow nope; this orchestrator has default, design, product-team, quick"
    );
}

#[test]
fn a_built_in_workflow_is_worded_by_the_prompts_its_steps_take_from_the_default() {
    let mut overrides = Map::new();
    overrides.insert("BUILD_PROMPT", "Build {spec_path} in {cwd}; report to {build_path}.".to_string());
    overrides.insert("FIX_PROMPT", "Fix {findings}; report to {build_path}.".to_string());
    let prompts = Prompts::from_overrides(&overrides).unwrap();
    let default = find_workflow("default", &prompts).unwrap();
    assert_eq!(default.step("build").unwrap().prompt, "Build {spec_path} in {cwd}; report to {build_path}.");
    assert!(is_built_in(&default, &prompts));
    assert!(!is_built_in(&default, &built_in()));
    // quick's build step has a prompt of its own and takes again from the default's, as its file would.
    let quick = find_workflow("quick", &prompts).unwrap();
    let build = quick.step("build").unwrap();
    assert!(build.prompt.starts_with("You are the Builder, the first of two roles"), "{}", build.prompt);
    assert_eq!(build.again.as_deref(), Some(prompts.text(Prompt::FixPrompt)));
    assert!(is_built_in(&quick, &prompts));
    // product-team words every text of the steps it uses itself, so no prompt file changes it.
    assert_eq!(find_workflow("product-team", &prompts).unwrap(), built_in_workflow("product-team"));
}

#[test]
fn a_workflow_file_cannot_take_a_built_in_name() {
    let config = Config::new();
    let path = config.file("quick", &workflow_toml(&quick()));
    assert_eq!(
        load_workflow_file(&path, &built_in()).unwrap_err().0,
        format!(
            "{path}: workflow quick is built in, so no file may take its name; \
             rename the file to keep it as a workflow of its own, or delete it and run the built-in"
        )
    );
    // The name finds the built-in, which the file does not shadow.
    assert_eq!(named_workflow(&config.config, "quick", &built_in()).unwrap(), built_in_workflow("quick"));
    assert_eq!(workflow_names(&config.config).unwrap(), WORKFLOWS);
}

#[test]
fn a_run_saved_from_a_workflow_file_of_a_built_in_name_keeps_its_definition() {
    // Before quick was built in, a run of the quick.toml file saved its definition under that name: say, a Reviewer
    // on another model. It resumes with that definition, not the built-in's.
    let mut saved = workflow_definition(&built_in_workflow("quick"));
    saved["roles"]["review"]["model"] = Value::from("opus");
    let p = run_pipeline("quick", Some(&saved), &built_in()).unwrap();
    assert_eq!(p.models(), &roles(&[("review", "opus")]));
    assert_ne!(p, built_in_workflow("quick"));
    assert!(!is_built_in(&p, &built_in()), "so a resumed run saves it again");
}

#[test]
fn every_workflow_parses_to_the_definition_workflows_json_records() {
    let cases = oracle("workflows.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 12, "workflows.json holds {} workflows", cases.len());
    for case in cases {
        let (name, from) = (case.get("name").and_then(Value::as_str).unwrap(), case.get("from").unwrap().to_string());
        let text = case.get("toml").and_then(Value::as_str).unwrap();
        let outcome = case.get("outcome").unwrap();
        let parsed = parse_workflow(name, &toml(text), &built_in());
        match outcome.get("error").and_then(Value::as_str) {
            Some(error) => assert_eq!(parsed.unwrap_err(), error, "{from}"),
            None => {
                let p = parsed.unwrap_or_else(|e| panic!("{from}: {e}"));
                let field = |key: &str| outcome.get(key).and_then(Value::as_str).unwrap();
                // The keys and values workflows.json records, its spacing aside.
                let definition: Value = serde_json::from_str(field("definition")).unwrap();
                assert_eq!(workflow_definition(&p), definition, "{from}");
                assert_eq!(workflow_toml(&p), field("toml"), "{from}");
                assert_eq!(workflow_shape(&p), field("shape"), "{from}");
                assert_eq!(Some(is_built_in(&p, &built_in())), outcome.get("built_in").and_then(Value::as_bool), "{from}");
                // The definition as state.json saves it gives the workflow back too.
                let saved = serde_json::from_str::<Value>(field("definition")).unwrap();
                assert_eq!(run_pipeline(name, Some(&saved), &built_in()).unwrap(), p, "{from}");
            }
        }
    }
}

#[test]
fn the_built_in_files_load() {
    let default = &*DEFAULT_WORKFLOW;
    assert_eq!(workflow_shape(&built_in_workflow("quick")), "build -> review (back to build)");
    let team = built_in_workflow("product-team");
    assert_eq!(team.name(), "product-team");
    let shape = "spec -> design -> tests -> build -> review (back to build) -> accept (back to build)";
    assert_eq!(workflow_shape(&team), shape);
    assert_eq!(
        team.roles(),
        &roles(&[("spec", "Product Manager"), ("design", "Architect"), ("tests", "QA Engineer"), ("build", "Engineer"), ("review", "Tech Lead")])
    );
    assert_eq!(team.models(), &roles(&[("tests", "sonnet")]));
    let [spec, design, tests, build, review, accept] = team.steps() else { panic!("six steps") };
    // use keeps every field the file does not set: the interview stays human-paced, the Engineer quality-gated, the
    // Tech Lead the verdict step looping back to the Engineer.
    let mut as_default = spec.clone();
    as_default.prompt = default.step("spec").unwrap().prompt.clone();
    assert_eq!(&as_default, default.step("spec").unwrap());
    assert!(spec.prompt.contains("first line is a `# ` heading"));
    for st in [build, review] {
        let without_texts = |st: &Step| {
            let mut fields = step_fields(st);
            for key in ["prompt", "again", "fresh_note"] {
                fields.remove(key);
            }
            fields
        };
        assert_eq!(without_texts(st), without_texts(default.step(&st.id).unwrap()));
    }
    assert_eq!((design.file.as_str(), design.edits), ("design.md", false));
    assert_eq!((tests.role.as_str(), tests.file.as_str(), tests.edits, tests.locks), ("tests", "tests.md", true, true));
    // Every round of the Engineer and the Tech Lead, and a fresh session of either, hears of the design and the
    // locked acceptance tests, not only the first.
    for st in [build, review] {
        for text in [Some(&st.prompt), st.again.as_ref(), st.fresh_note.as_ref()] {
            let text = text.unwrap();
            assert!(text.contains("{design_path}") && text.contains("{tests_path}"), "{text}");
        }
    }
    assert!(tests.prompt.contains("{design_path}"));
    // The Product Manager who ran the interview accepts after the Tech Lead, sending the change back to the Engineer
    // too, and judges it against the requirements alone.
    let fields = (accept.role.as_str(), accept.file.as_str(), accept.loop_to.as_deref(), accept.edits, accept.human_paced);
    assert_eq!(fields, ("spec", "acceptance-{n}.md", Some("build"), false, false));
    for text in [Some(&accept.prompt), accept.again.as_ref(), accept.fresh_note.as_ref()] {
        assert!(text.unwrap().contains("{spec_path}"));
    }
    // The summary for the Product Manager, which stood in for the acceptance, is gone from every text of the review.
    for text in [Some(&review.prompt), review.again.as_ref(), review.fresh_note.as_ref()] {
        assert!(!text.unwrap().contains("summary"));
    }
}

#[test]
fn default_round_trips_through_its_file() {
    let text = workflow_toml(&DEFAULT_WORKFLOW);
    assert_eq!(parse_workflow("default", &toml(&text), &built_in()).unwrap(), *DEFAULT_WORKFLOW);
}

#[test]
fn the_change_budget_is_the_workflows() {
    let config = Config::new();
    let step = "id = \"build\"\nfile = \"b-{n}.md\"\nprompt = \"{path}\"\nedits = true\n";
    let p = load_workflow_file(&config.file("small", &format!("max_files = 1\nmax_lines = 100\n[[steps]]\n{step}")), &built_in()).unwrap();
    assert_eq!((p.max_files(), p.max_lines()), (1, 100));
    assert_eq!(parse_workflow("small", &toml(&workflow_toml(&p)), &built_in()).unwrap(), p);
    assert_eq!(workflow_definition(&p).get("max_lines"), Some(&Value::from(100)));
    assert!(workflow_definition(&DEFAULT_WORKFLOW).get("max_files").is_none());
    for (value, message) in [
        ("0", "workflow bad: max_lines must be at least 1"),
        ("-1", "workflow bad: max_lines must be at least 1"),
        ("\"3\"", "max_lines must be a whole number"),
        ("true", "max_lines must be a whole number"),
        ("1.5", "max_lines must be a whole number"),
    ] {
        let path = config.file("bad", &format!("max_lines = {value}\n[[steps]]\n{step}"));
        let e = load_workflow_file(&path, &built_in()).unwrap_err();
        assert!(e.0.contains(message), "{value}: {e}");
    }
}

#[test]
fn any_text_round_trips() {
    let odd = [
        "ends in a quote '", "has ''' inside\nand a newline", "tab\there", "del\x7f", "ünïcode 🦀\n",
        "\nstarts with a newline", "back\\slash \"quoted\"\n",
    ];
    let steps = odd.iter().enumerate().map(|(i, text)| Step::new(&format!("s{i}"), "b", &format!("s{i}.md"), &format!("{text} {{path}}"))).collect();
    let settings = PipelineSettings { models: roles(&[("b", "m")]), description: "with ''' and '".into(), ..PipelineSettings::default() };
    let p = Pipeline::with("odd", roles(&[("b", "Bob's \"pane\"")]), steps, settings).unwrap();
    assert_eq!(parse_workflow("odd", &toml(&workflow_toml(&p)), &built_in()).unwrap(), p);
}

#[test]
fn roles_default_from_the_steps() {
    let config = Config::new();
    let p = load_workflow_file(&config.file("mine", r#"
        [[steps]]
        id = "test_writer"
        file = "tests.md"
        prompt = "Write tests for {task}; report to {path}."
        edits = true

        [[steps]]
        use = "build"
        prompt = "Make {test_writer_path} pass; report to {build_path}."
        again = "Fix them; report to {build_path}."
        fresh_note = "Earlier: {earlier_build_paths}."
        quality_gated = false
    "#), &built_in()).unwrap();
    assert_eq!(p.roles(), &roles(&[("test_writer", "Test Writer"), ("build", "Builder")]));
    assert_eq!(p.steps()[0].role, "test_writer");
    let st = &p.steps()[1];
    assert_eq!((st.edits, st.quality_gated, st.fresh_repeats_again), (true, false, true));
    assert!(p.models().is_empty());
    assert_eq!(default_label("a-b__c-"), "A B C ");
}

#[test]
fn panes_follow_the_steps_not_the_roles_table() {
    let config = Config::new();
    let p = load_workflow_file(&config.file("mine", r#"
        roles.review = { model = "opus" }
        roles.build = "Implementer"
        [[steps]]
        use = "spec"
        [[steps]]
        use = "build"
        [[steps]]
        use = "review"
    "#), &built_in()).unwrap();
    assert_eq!(p.roles(), &roles(&[("spec", "Spec Collector"), ("build", "Implementer"), ("review", "Reviewer")]));
    assert_eq!(p.roles().keys().collect::<Vec<_>>(), ["spec", "build", "review"]);
    assert_eq!(p.models(), &roles(&[("review", "opus")]));
}

#[test]
fn invalid_files_name_the_problem() {
    let config = Config::new();
    let step = "id = \"build\"\nfile = \"b-{n}.md\"\nprompt = \"{path}\"\nedits = true\n";
    let mut cases: Vec<(String, String)> = [
        ("step 1 (build): unknown key edit; did you mean edits?", format!("[[steps]]\n{step}edit = true\n")),
        ("the file: unknown key step; did you mean steps?", format!("[[step]]\n{step}")),
        ("it has no [[steps]]", "description = \"x\"\n".into()),
        ("step 1 (build): file is missing", "[[steps]]\nid = \"build\"\nprompt = \"x\"\n".into()),
        ("step 1 (build): use names no step of the default workflow; it has spec, build, review", format!("[[steps]]\nuse = \"test\"\n{step}")),
        ("step 1 (build): edits must be true or false", format!("[[steps]]\n{}", step.replace("true", "\"yes\""))),
        (
            "step 1 (build): prompt: } at line 1, column 3 closes no placeholder; write a literal brace as }}",
            "[[steps]]\nid = \"build\"\nfile = \"b-{n}.md\"\nprompt = \"a } b\"\nedits = true\n".into(),
        ),
        ("role build: give a label, or a table with label, model and agent", format!("roles.build = 3\n[[steps]]\n{step}")),
        ("role build: unknown key modle; did you mean model?", format!("roles.build = {{ modle = \"x\" }}\n[[steps]]\n{step}")),
        ("step 1 (a b): id may use only letters, digits, - and _", format!("[[steps]]\n{}", step.replace("\"build\"", "\"a b\""))),
        ("workflow bad: duplicate step id build", format!("[[steps]]\n{step}[[steps]]\n{step}")),
        ("workflow bad: role y has no step", format!("[roles]\ny = {{ model = \"m\" }}\n[[steps]]\n{step}")),
        ("(at line 1, column 6)", "this is not toml\n".into()),
        ("roles must be a table", format!("roles = 3\n[[steps]]\n{step}")),
        ("description must be a string", format!("description = 3\n[[steps]]\n{step}")),
        ("role a b: a role key may use only letters, digits, - and _", format!("roles.\"a b\" = \"x\"\n[[steps]]\n{step}")),
        ("role build: label must be a string", format!("roles.build = {{ label = 3 }}\n[[steps]]\n{step}")),
        ("step 1 must be a table", "steps = [3]\n".into()),
        ("step 1 (build): prompt must be a string", format!("[[steps]]\n{}", step.replace("\"{path}\"", "3"))),
        // Placeholders that are all known names, but written as more than a name.
        (
            "step 1 (build): prompt: placeholder {task:d} at line 1, column 1 has a format spec; a placeholder takes none",
            format!("[[steps]]\n{}", step.replace("\"{path}\"", "\"{task:d}\"")),
        ),
        (
            "step 1 (build): prompt: placeholder {task!z} at line 1, column 1 has a conversion; a placeholder takes none",
            format!("[[steps]]\n{}", step.replace("\"{path}\"", "\"{task!z}\"")),
        ),
        (
            "step 1 (build): again: placeholder {diff.upper} at line 1, column 1 has an attribute; a placeholder takes none",
            format!("[[steps]]\n{step}again = \"{{diff.upper}}\"\n"),
        ),
    ]
    .into_iter()
    .map(|(m, t)| (m.to_string(), t))
    .collect();
    for (name, why) in [("b-{n!s}.md", "{n!s} at line 1, column 3 has a conversion"), ("b-{n:d}.md", "{n:d} at line 1, column 3 has a format spec")] {
        cases.push((
            format!("step 1 (build): file: placeholder {why}; a placeholder takes none"),
            format!("[[steps]]\n{}", step.replace("b-{n}.md", name)),
        ));
    }
    for name in ["state.json", "summary.md", "metrics.json", "quality-{n}-1.md", "proofs-{n}", "../b-{n}.md", "/tmp/b-{n}.md", ".."] {
        cases.push((
            format!(
                "step build: handoff file {name} must be a plain file name, and not state.json, summary.md, \
                 metrics.json, quality-*, locked-* or proofs-*, which the run writes itself"
            ),
            format!("[[steps]]\n{}", step.replace("b-{n}.md", name)),
        ));
    }
    for (message, text) in cases {
        let path = config.file("bad", &text);
        let e = load_workflow_file(&path, &built_in()).unwrap_err();
        assert!(e.0.starts_with(&format!("{path}: ")) && e.0.contains(&message), "{message}: {e}");
    }
}

#[test]
fn a_table_with_two_problems_names_the_first_in_the_file() {
    let config = Config::new();
    let step = "id = \"build\"\nfile = \"b-{n}.md\"\nprompt = \"{path}\"\nedits = true\n";
    for (message, text) in [
        ("step 1 (build): unknown key zzz; it takes again,", format!("[[steps]]\n{step}zzz = 1\naaa = 2\n")),
        ("role zz: give a label, or a table with label, model and agent", format!("[roles]\nzz = 3\naa = 4\n[[steps]]\n{step}")),
    ] {
        let path = config.file("bad", &text);
        let e = load_workflow_file(&path, &built_in()).unwrap_err();
        assert!(e.0.contains(message), "{message}: {e}");
    }
}

#[test]
fn a_saved_definition_with_an_unreadable_template_or_corrupt_is_refused() {
    let Value::Object(mut definition) = workflow_definition(&quick()) else { panic!("an object") };
    let Some(Value::Array(steps)) = definition.get_mut("steps") else { panic!("steps") };
    let Value::Object(first) = &mut steps[0] else { panic!("a step") };
    first.insert("prompt".into(), Value::from("{task:d}"));
    let e = run_pipeline("quick", Some(&Value::Object(definition)), &built_in()).unwrap_err();
    assert!(e.0.starts_with("the definition of workflow quick saved with the run is invalid: step 1 (build): prompt: placeholder {task:d} at line 1, column 1 has a format spec"), "{e}");
    let e = run_pipeline("quick", Some(&serde_json::from_str::<Value>(r#"{"steps": []}"#).unwrap()), &built_in()).unwrap_err();
    assert_eq!(e.0, "the definition of workflow quick saved with the run is invalid: it has no [[steps]]");
    assert!(run_pipeline("nosuch", None, &built_in()).unwrap_err().0.starts_with("unknown workflow nosuch"));
}

#[test]
fn a_file_cannot_replace_a_built_in_workflow_or_take_a_name_no_workflow_can() {
    let config = Config::new();
    let path = config.file("default", "[[steps]]\nuse = \"spec\"\n");
    assert!(load_workflow_file(&path, &built_in()).unwrap_err().0.ends_with("workflow default is built in, so no file may take its name; rename the file to keep it as a workflow of its own, or delete it and run the built-in"));
    // The name is wrong whatever the file holds, so it is what the error names, not a syntax error a fix would not
    // get the file past.
    let path = config.file("quick", "[[steps]\n");
    assert!(load_workflow_file(&path, &built_in()).unwrap_err().0.contains("workflow quick is built in"));
    let path = config.file("my flow", "[[steps]]\nuse = \"spec\"\n");
    assert!(load_workflow_file(&path, &built_in()).unwrap_err().0.contains("a workflow's name, its file's, may use only letters"));
}

#[test]
fn a_missing_or_undecodable_file() {
    assert_eq!(
        load_workflow_file("/nope/x.toml", &built_in()).unwrap_err().0,
        "cannot read workflow file /nope/x.toml: No such file or directory"
    );
    let config = Config::new();
    let path = config.dir.join("workflows/latin.toml");
    std::fs::write(&path, b"description = \"\xe9\"\n").unwrap();
    assert!(load_workflow_file(&path, &built_in()).unwrap_err().0.starts_with(&format!("{path}: ")));
}

#[test]
fn files_are_offered_by_name() {
    let config = Config::new();
    config.file("team", &workflow_toml(&built_in_workflow("product-team")));
    config.file("broken", "nonsense\n");
    std::fs::write(config.dir.join("workflows/notes.txt"), "not a workflow").unwrap();
    assert_eq!(workflow_names(&config.config).unwrap(), ["broken", "default", "design", "product-team", "quick", "team"]);
    assert_eq!(named_workflow(&config.config, "team", &built_in()).unwrap().name(), "team");
    assert_eq!(named_workflow(&config.config, "default", &built_in()).unwrap(), *DEFAULT_WORKFLOW);
    assert_eq!(
        named_workflow(&config.config, "nope", &built_in()).unwrap_err().0,
        "unknown workflow nope; there are broken, default, design, product-team, quick, team"
    );
    let none = ConfigDir::at(&config.dir.join("elsewhere"));
    assert!(workflow_files(&none).unwrap().is_empty());
}

#[test]
fn an_unlistable_workflows_directory() {
    let dir = TempDir::new();
    std::fs::write(dir.join("workflows"), "a file, not a directory").unwrap();
    let config = ConfigDir::at(&dir.join(""));
    assert!(workflow_names(&config).unwrap_err().0.starts_with(&format!("cannot list {}", dir.join("workflows"))));
}

#[test]
fn agents_are_parsed_and_round_tripped() {
    let p = parse_workflow("mine", &toml(
        "roles.review = { agent = \"codex\", model = \"o3\" }\nroles.build = { label = \"Implementer\", agent = \"pi\" }\n\
         [[steps]]\nuse = \"spec\"\n[[steps]]\nuse = \"build\"\n[[steps]]\nuse = \"review\"\n",
    ), &built_in()).unwrap();
    assert_eq!(p.agents(), &roles(&[("review", "codex"), ("build", "pi")]));
    let definition = workflow_definition(&p);
    assert_eq!(
        definition.get("roles").unwrap().to_string(),
        r#"{"spec":{"label":"Spec Collector"},"build":{"label":"Implementer","agent":"pi"},"review":{"label":"Reviewer","model":"o3","agent":"codex"}}"#
    );
    let text = workflow_toml(&p);
    assert!(text.contains("[roles.review]\nlabel = \"Reviewer\"\nmodel = \"o3\"\nagent = \"codex\"\n"), "{text}");
    assert_eq!(parse_workflow("mine", &toml(&text), &built_in()).unwrap(), p);
    assert_eq!(parse_workflow("mine", &definition, &built_in()).unwrap(), p);
}

#[test]
fn invalid_agents_name_the_role() {
    let steps = "[[steps]]\nuse = \"spec\"\n[[steps]]\nuse = \"build\"\n[[steps]]\nuse = \"review\"\n";
    assert_eq!(parse_workflow("mine", &toml(steps), &built_in()).unwrap().steps(), DEFAULT_WORKFLOW.steps());
    for (text, message) in [
        ("roles.review = { agent = \"cursor\" }\n", "role review: agent cursor is not a harness this orchestrator supports; it takes agy, claude, codex, opencode, pi"),
        ("roles.review = { agent = 3 }\n", "role review: agent must be a string"),
        ("roles.review = { agnet = \"pi\" }\n", "role review: unknown key agnet; did you mean agent?"),
    ] {
        let e = parse_workflow("mine", &toml(&format!("{text}{steps}")), &built_in()).unwrap_err();
        assert!(e.contains(message), "{message}: {e}");
    }
}

#[test]
fn locks_round_trip_and_are_checked_in_a_file() {
    let text = workflow_toml(&locking());
    assert!(text.contains("locks = true"));
    assert_eq!(parse_workflow("locking", &toml(&text), &built_in()).unwrap(), locking());
    let step = "id = \"tests\"\nfile = \"t.md\"\nprompt = \"{path}\"\nlocks = true\n";
    let e = parse_workflow("bad", &toml(&format!("[[steps]]\n{step}")), &built_in()).unwrap_err();
    assert!(e.ends_with("step tests locks the files it changes, so it must edit"), "{e}");
    let e = parse_workflow("bad", &toml(&format!("[[steps]]\n{}", step.replace("true", "\"yes\""))), &built_in()).unwrap_err();
    assert!(e.ends_with("locks must be true or false"), "{e}");
}

#[test]
fn proof_command_in_a_file() {
    let p = parse_workflow("proving", &toml(r#"
proof_command = "python -m pytest -q {proofs}"

[[steps]]
use = "spec"

[[steps]]
use = "build"

[[steps]]
use = "review"
prompt = "Back each finding with a test in {proofs_dir}; write {review_path}."
again = "Again, with tests in {proofs_dir}; write {review_path}."
"#), &built_in()).unwrap();
    assert_eq!(p.proof_command(), Some("python -m pytest -q {proofs}"));
    assert!(p.step("review").unwrap().prompt.contains("{proofs_dir}"));
    let proving = replaced(&quick(), |s| s.proof_command = Some("sh {proofs}".into())).unwrap();
    let text = workflow_toml(&proving);
    assert!(text.contains("proof_command = \"sh {proofs}\""));
    assert_eq!(parse_workflow("quick", &toml(&text), &built_in()).unwrap(), proving);
    assert!(!workflow_toml(&quick()).contains("proof_command"));
    let Value::Object(mut definition) = workflow_definition(&quick()) else { panic!("an object") };
    definition.insert("proof_command".into(), Value::from(1));
    assert_eq!(parse_workflow("bad", &Value::Object(definition), &built_in()).unwrap_err(), "proof_command must be a string");
}

// -- Editing a workflow: read_workflow, check_workflow, save_workflow, delete_workflow -------------------------------

/// A workflow file of default's steps by use, with a comment: a workflow not in canonical form.
fn by_use() -> String {
    let steps: Vec<String> = DEFAULT_WORKFLOW.steps().iter().map(|st| format!("[[steps]]\nuse = \"{}\"\n", st.id)).collect();
    format!("# default's steps, by name\n{}", steps.join("\n"))
}

#[test]
fn a_saved_definition_is_the_workflow_run_loads_and_reads_back_canonical() {
    let config = Config::new();
    let definition = workflow_definition(&built_in_workflow("product-team"));
    let path = save_workflow(&config.config, "mine", &definition, None).unwrap();
    assert_eq!(path, config.dir.join("workflows/mine.toml"));
    let loaded = named_workflow(&config.config, "mine", &built_in()).unwrap();
    assert_eq!(workflow_definition(&loaded), definition);
    let read = read_workflow(&config.config, "mine").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(read, EditableWorkflow { definition, file: Some(DefinitionFile { path, text, hand_written: false }) });
}

#[test]
fn a_file_with_a_comment_and_uses_reads_as_hand_written_and_saves_canonical() {
    let config = Config::new();
    let path = config.file("mine", &by_use());
    let read = read_workflow(&config.config, "mine").unwrap();
    assert_eq!(read.definition, workflow_definition(&DEFAULT_WORKFLOW));
    let file = read.file.unwrap();
    assert!(file.hand_written);
    assert_eq!(file.text, by_use());
    save_workflow(&config.config, "mine", &read.definition, Some(&file.text)).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(saved, workflow_toml(&named_workflow(&config.config, "mine", &built_in()).unwrap()));
    assert!(!read_workflow(&config.config, "mine").unwrap().file.unwrap().hand_written);
}

#[test]
fn a_built_in_workflow_reads_without_a_file_and_cannot_be_saved_or_deleted() {
    let config = Config::new();
    let read = read_workflow(&config.config, "default").unwrap();
    assert_eq!(read, EditableWorkflow { definition: workflow_definition(&DEFAULT_WORKFLOW), file: None });
    let refused = save_workflow(&config.config, "default", &read.definition, None).unwrap_err();
    let error = "workflow default is built in, so no file may take its name".to_string();
    assert_eq!(refused, WorkflowEditError::Invalid { error, path: None, text: None });
    assert!(matches!(delete_workflow(&config.config, "default", ""), Err(WorkflowEditError::BuiltIn(_))));
    assert!(!std::path::Path::new(&config.dir.join("workflows/default.toml")).exists());
}

#[test]
fn reading_an_unknown_workflow_or_one_that_does_not_load() {
    let config = Config::new();
    let unknown = read_workflow(&config.config, "nope").unwrap_err();
    assert_eq!(unknown, WorkflowEditError::Unknown("no workflow nope".into()));
    let path = config.file("broken", "not toml [");
    match read_workflow(&config.config, "broken").unwrap_err() {
        WorkflowEditError::Invalid { error, path: Some(at), text } => {
            assert_eq!((at, text.as_deref()), (path.clone(), Some("not toml [")));
            assert!(error.starts_with(&format!("{path}: ")), "{error}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_save_or_delete_is_refused_when_the_file_is_not_the_text_it_started_from() {
    let config = Config::new();
    let definition = workflow_definition(&DEFAULT_WORKFLOW);
    let changed = |r: Result<String, WorkflowEditError>| match r {
        Err(WorkflowEditError::Changed(why)) => why,
        other => panic!("{other:?}"),
    };
    // A new workflow under a name a file already has.
    let path = config.file("mine", &by_use());
    assert_eq!(changed(save_workflow(&config.config, "mine", &definition, None)), format!("{path} already exists"));
    // A file changed since it was read.
    let opened = read_workflow(&config.config, "mine").unwrap().file.unwrap().text;
    std::fs::write(&path, format!("{opened}\n# edited elsewhere\n")).unwrap();
    let why = changed(save_workflow(&config.config, "mine", &definition, Some(&opened)));
    assert_eq!(why, format!("{path} changed since it was opened"));
    changed(delete_workflow(&config.config, "mine", &opened));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("{opened}\n# edited elsewhere\n"));
    // A file removed since it was read.
    std::fs::remove_file(&path).unwrap();
    let why = changed(save_workflow(&config.config, "mine", &definition, Some(&opened)));
    assert_eq!(why, format!("{path} was removed since it was opened"));
    assert!(!std::path::Path::new(&path).exists());
}

#[test]
fn a_write_that_fails_leaves_the_old_file_and_nothing_aside() {
    use std::os::unix::fs::PermissionsExt;
    let config = Config::new();
    let path = config.file("mine", &by_use());
    let dir = config.dir.join("workflows");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let saved = save_workflow(&config.config, "mine", &workflow_definition(&quick()), Some(&by_use()));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    if saved.is_ok() {
        return; // Run as root, which writes anywhere.
    }
    match saved.unwrap_err() {
        WorkflowEditError::Failed(e) => assert!(e.0.starts_with(&format!("cannot write {path}: ")), "{}", e.0),
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), by_use());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn a_save_makes_the_workflows_directory() {
    let dir = TempDir::new();
    let config = ConfigDir::at(&dir.join("config"));
    let path = save_workflow(&config, "mine", &workflow_definition(&quick()), None).unwrap();
    assert_eq!(path, dir.join("config/workflows/mine.toml"));
    assert_eq!(named_workflow(&config, "mine", &built_in()).unwrap().steps(), quick().steps());
}

#[test]
fn a_delete_removes_the_file_it_was_given() {
    let config = Config::new();
    let path = config.file("mine", &by_use());
    assert_eq!(delete_workflow(&config.config, "mine", &by_use()).unwrap(), path);
    assert!(!std::path::Path::new(&path).exists());
    let gone = delete_workflow(&config.config, "mine", &by_use()).unwrap_err();
    assert_eq!(gone, WorkflowEditError::Unknown("no workflow file mine".into()));
}

#[test]
fn a_check_names_the_first_problem_and_still_lists_the_placeholders() {
    let definition = serde_json::json!({"steps": [
        {"use": "spec"},
        {"id": "build", "file": "build-{n}.md", "prompt": "Build {spec_path}; {nope}.", "edits": true},
    ]});
    let checked = check_workflow("mine", &definition);
    assert!(checked.problem.as_deref().is_some_and(|p| p.contains("nope")), "{checked:?}");
    assert_eq!(checked.placeholders, crate::pipeline::prompt_placeholders(["spec", "build"]));
    let fine = check_workflow("mine", &workflow_definition(&DEFAULT_WORKFLOW));
    assert_eq!(fine.problem, None);
    let named = |name: &str| check_workflow(name, &workflow_definition(&DEFAULT_WORKFLOW)).problem.unwrap();
    assert_eq!(named("default"), "workflow default is built in, so no file may take its name");
    assert!(named("my flow").starts_with("a workflow's name, its file's, may use only letters"));
    assert!(named("../up").starts_with("a workflow's name"));
}

#[test]
fn a_file_that_does_not_load_can_be_deleted_by_the_text_its_read_gave() {
    let config = Config::new();
    let path = config.file("broken", "steps = [");
    let Err(WorkflowEditError::Invalid { text: Some(text), .. }) = read_workflow(&config.config, "broken") else {
        panic!("a broken file reads as Invalid with its text");
    };
    assert_eq!(delete_workflow(&config.config, "broken", &text).unwrap(), path);
    // Not UTF-8: its text has U+FFFD in place of the bad byte, and is still its base.
    std::fs::write(&path, b"description = \"caf\xe9\"\n").unwrap();
    let Err(WorkflowEditError::Invalid { text: Some(text), .. }) = read_workflow(&config.config, "broken") else {
        panic!("a file that is not UTF-8 reads as Invalid with its text");
    };
    assert_eq!(text, "description = \"caf\u{fffd}\"\n");
    assert_eq!(delete_workflow(&config.config, "broken", &text).unwrap(), path);
}

#[test]
fn concurrent_saves_of_one_workflow_leave_one_whole_file_and_nothing_aside() {
    let config = Config::new();
    let definition = workflow_definition(&DEFAULT_WORKFLOW);
    let path = save_workflow(&config.config, "mine", &definition, None).unwrap();
    let base = std::fs::read_to_string(&path).unwrap();
    let saves: Vec<_> = (0..32)
        .map(|_| {
            let (config, definition, base) = (config.config.clone(), definition.clone(), base.clone());
            std::thread::spawn(move || save_workflow(&config, "mine", &definition, Some(&base)))
        })
        .collect();
    let saved: Vec<_> = saves.into_iter().map(|t| t.join().unwrap()).collect();
    // The saves write the same text, so each later one finds the base it was given.
    assert!(saved.iter().all(Result::is_ok), "{saved:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), base);
    assert_eq!(std::fs::read_dir(config.dir.join("workflows")).unwrap().count(), 1);
}
