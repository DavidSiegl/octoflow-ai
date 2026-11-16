//! The workflows command, through main as herdrless_command runs it: listing the workflows and the prompt files,
//! printing one as a file, and saying why the prompt files would stop a run.

use crate::cli::fixture::Cli;
use crate::env::ConfigDir;
use crate::pipeline::DEFAULT_WORKFLOW;
use crate::prompts::Prompts;
use crate::test_support::{built_in_as_file, TempDir};
use crate::workflow_file::{find_workflow, parse_toml, parse_workflow};

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

/// What `workflows` with argv prints, having exited 0 without asking herdr anything.
fn printed(cli: &Cli, argv: &[&str]) -> String {
    let argv: Vec<&str> = ["workflows"].iter().chain(argv).copied().collect();
    assert_eq!(cli.main(&argv), 0, "{argv:?}");
    assert!(cli.f.herdr.state().calls.is_empty());
    cli.take_out()
}

#[test]
fn workflows_lists_them() {
    let d = TempDir::new();
    let solo = "description = \"Builder alone\"\n[[steps]]\nid = \"build\"\nfile = \"b-{n}.md\"\nprompt = \"{path}\"\nedits = true\n";
    let cli = configured(&d, &[
        ("workflows/solo.toml", solo),
        ("workflows/broken.toml", "[[steps]]\nid = 3\n"),
        ("workflows/quick.toml", "[[steps]]\nuse = \"build\"\n"),
    ]);
    let workflows = cli.config.workflows_dir();
    assert_eq!(printed(&cli, &[]).lines().collect::<Vec<_>>(), [
        "default  built in".to_string(),
        "    spec -> build -> review (back to build)".into(),
        "product-team  built in".into(),
        "    spec -> design -> tests -> build -> review (back to build) -> accept (back to build)".into(),
        "    Product Manager -> Architect -> QA Engineer -> Engineer <-> Tech Lead, then the Product Manager's acceptance"
            .into(),
        "quick  built in".into(),
        "    build -> review (back to build)".into(),
        "    Builder <-> Reviewer, with the task as the contract: no interview".into(),
        format!("broken  {workflows}/broken.toml"),
        "    error: step 1: file is missing".into(),
        // A file of a built-in's name, which the built-in replaced: the list says how to fix it.
        format!("quick  {workflows}/quick.toml"),
        "    error: workflow quick is built in; delete this file to run the built-in, or rename it to keep it as a \
         workflow of its own"
            .into(),
        format!("solo  {workflows}/solo.toml"),
        "    build".into(),
        "    Builder alone".into(),
        String::new(),
        format!(
            "Add one as {workflows}/NAME.toml; `octoflow workflows default` prints the default as a file to \
             start from."
        ),
    ]);
}

#[test]
fn workflows_prints_one_as_a_file() {
    let d = TempDir::new();
    let cli = configured(&d, &[]);
    let path = built_in_as_file(&d, "product-team", "team");
    for (reference, name) in [("product-team", "product-team"), ("quick", "quick"), (path.as_str(), "team")] {
        let text = printed(&cli, &[reference]);
        let printed = parse_workflow(name, &parse_toml(&text).unwrap(), &Prompts::default()).unwrap();
        let built_in = if name == "team" { "product-team" } else { name };
        assert_eq!(printed.steps(), find_workflow(built_in, &Prompts::default()).unwrap().steps(), "{reference}");
    }
}

#[test]
fn workflows_prints_a_files_agent() {
    let d = TempDir::new();
    let coded = "roles.review = { agent = \"codex\" }\n[[steps]]\nuse = \"spec\"\n[[steps]]\nuse = \"build\"\n\
                 [[steps]]\nuse = \"review\"\n";
    let cli = configured(&d, &[("workflows/coded.toml", coded)]);
    let text = printed(&cli, &["coded"]);
    assert!(text.contains("[roles.review]\nlabel = \"Reviewer\"\nagent = \"codex\"\n"), "{text}");
}

#[test]
fn an_unknown_or_broken_workflow_is_an_error() {
    let d = TempDir::new();
    let cli = configured(&d, &[("workflows/broken.toml", "[[steps]]\nid = 3\n")]);
    assert_eq!(cli.error(&["workflows", "nosuch"]), "unknown workflow nosuch; there are broken, default, product-team, quick");
    let broken = cli.config.workflows_dir() + "/broken.toml";
    assert_eq!(cli.error(&["workflows", &broken]), format!("{broken}: step 1: file is missing"));
    assert_eq!(cli.main(&["workflows", "nosuch"]), crate::constants::EXIT_ERROR);
    assert_eq!(cli.take_out(), "");
}

#[test]
fn workflows_lists_the_prompt_files() {
    let d = TempDir::new();
    let cli = configured(&d, &[
        ("prompts/QUALITY_OFF_NOTE.md", "Off: {quality_paths}.\n"),
        ("prompts/BUILD_PROMPT.md", "Build {spec_path} in {cwd}; report to {build_path}.\n"),
    ]);
    let prompts = cli.config.prompts_dir();
    let out = printed(&cli, &[]);
    let expected = format!(
        "\nPrompt files in {prompts}, in place of the built-in texts:\n    BUILD_PROMPT  {prompts}/BUILD_PROMPT.md\n    \
         QUALITY_OFF_NOTE  {prompts}/QUALITY_OFF_NOTE.md\n"
    );
    assert!(out.ends_with(&expected), "{out}");
}

#[test]
fn workflows_says_why_the_prompt_files_would_stop_a_run() {
    let d = TempDir::new();
    let cli = configured(&d, &[("prompts/BUILD_PROMPT.md", "Build.\n")]);
    let prompts = cli.config.prompts_dir();
    let out = printed(&cli, &[]);
    let expected = format!(
        "\nPrompt files in {prompts}:\n    error: {prompts}/BUILD_PROMPT.md: placeholder {{build_path}} is missing; \
         BUILD_PROMPT takes {{build_path}}, {{cwd}}, {{spec_path}}\n"
    );
    assert!(out.ends_with(&expected), "{out}");
    // Without prompt files, the list ends at how to add a workflow.
    let cli = configured(&TempDir::new(), &[]);
    assert!(printed(&cli, &[]).ends_with("prints the default as a file to start from.\n"));
}

#[test]
fn workflows_prints_the_default_as_the_prompt_files_word_it() {
    let d = TempDir::new();
    let build = "Build {spec_path} in {cwd}; report to {build_path}.";
    let cli = configured(&d, &[("prompts/BUILD_PROMPT.md", &format!("{build}\n"))]);
    let text = printed(&cli, &["default"]);
    let default = parse_workflow("default", &parse_toml(&text).unwrap(), &Prompts::default()).unwrap();
    assert_eq!(default.step("build").unwrap().prompt, build);
    assert_ne!(default, *DEFAULT_WORKFLOW);
}
