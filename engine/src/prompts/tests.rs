use std::os::unix::fs::PermissionsExt;

use super::*;
use serde_json::Value;
use crate::test_support::{oracle, TempDir};
use crate::env::ConfigDir;

#[test]
fn every_built_in_text_is_the_one_texts_json_records() {
    let texts = oracle("texts.json");
    let recorded = texts.get("prompts").and_then(Value::as_object).unwrap();
    let names: Vec<&str> = Prompt::ALL.iter().map(|p| p.name()).collect();
    assert_eq!(names, recorded.keys().collect::<Vec<_>>());
    for p in Prompt::ALL {
        assert_eq!(Some(p.built_in()), recorded.get(p.name()).and_then(Value::as_str), "{}", p.name());
    }
}

#[test]
fn the_builders_fix_prompt_names_the_findings_that_sent_the_round_back() {
    let names = placeholders(FIX_PROMPT).unwrap();
    assert!(names.iter().any(|n| n == "findings"), "{names:?}");
    assert!(!names.iter().any(|n| n == "prev_review_path"), "{names:?}");
    // A prompt file written for an earlier release is refused, naming the placeholder it may no longer use and the
    // ones it must.
    assert_eq!(
        prompt_problem("FIX_PROMPT", "Fix {prev_review_path}; report to {build_path}."),
        Some(
            "placeholder {prev_review_path} at line 1, column 5 is not one this template takes; FIX_PROMPT takes \
             {build_path}, {findings}"
                .into()
        )
    );
}

#[test]
fn every_built_in_prompt_is_overridable() {
    for p in Prompt::ALL {
        assert_eq!(Prompt::named(p.name()), Some(*p));
        assert_eq!(Prompts::default().text(*p), p.built_in());
        assert_eq!(prompt_problem(p.name(), p.built_in()), None, "{}", p.name());
    }
}

/// A configuration directory with these prompt files, each written as an editor would, with a newline at its end;
/// the files' paths are its join("prompts/<name>").
fn prompt_dir(files: &[(&str, &[u8])]) -> (TempDir, ConfigDir) {
    let dir = TempDir::new();
    std::fs::create_dir(dir.join("prompts")).unwrap();
    for (name, text) in files {
        let mut bytes = text.to_vec();
        bytes.push(b'\n');
        std::fs::write(dir.join(&format!("prompts/{name}")), bytes).unwrap();
    }
    let config = ConfigDir::at(&dir.join(""));
    (dir, config)
}

#[test]
fn bad_files_are_refused_naming_them() {
    let cases: [(&str, &[u8], &str); 8] = [
        ("BUILDER_PROMPT.md", b"Build {spec_path}.", "BUILDER_PROMPT names no built-in prompt; did you mean BUILD_PROMPT.md?"),
        ("README.md", b"notes", "README names no built-in prompt; the names are BLOCKED_NOTE, BLOCKED_PROMPT, "),
        (
            "BUILD_PROMPT.md",
            b"Build {spec_path} in {cwd} {nope}; report to {build_path}.",
            "placeholder {nope} at line 1, column 28 is not one this template takes; BUILD_PROMPT takes {build_path}, {cwd}, {spec_path}",
        ),
        (
            "BUILD_PROMPT.md",
            b"Build {spec_path} in {cwd}.",
            "placeholder {build_path} is missing; BUILD_PROMPT takes {build_path}, {cwd}, {spec_path}",
        ),
        ("BLOCKED_NOTE.md", b"Ask {}.", "placeholder {} at line 1, column 5 is a positional field; a placeholder is a name"),
        ("CONTINUE_PROMPT.md", b"Write {path!z}.", "placeholder {path!z} at line 1, column 7 has a conversion; a placeholder takes none"),
        ("CONTINUE_PROMPT.md", b"Write {path}}.", "} at line 1, column 13 closes no placeholder; write a literal brace as }}"),
        ("FIX_PROMPT.md", b"Fix \xe9", "not UTF-8 text"),
    ];
    for (name, text, message) in cases {
        let (dir, config) = prompt_dir(&[(name, text)]);
        let e = load_prompts(&config).unwrap_err();
        let path = dir.join(&format!("prompts/{name}"));
        assert!(e.0.starts_with(&format!("{path}: {message}")), "{name}: {e}");
    }
}

#[test]
fn an_unreadable_file_names_it() {
    let (dir, config) = prompt_dir(&[]);
    let path = dir.join("prompts/FIX_PROMPT.md");
    std::fs::create_dir(&path).unwrap();
    let e = load_prompts(&config).unwrap_err();
    assert_eq!(e.0, format!("cannot read prompt file {path}: Is a directory"));
}

#[test]
fn an_unlistable_directory_names_it() {
    let (dir, config) = prompt_dir(&[]);
    let prompts = dir.join("prompts");
    std::fs::set_permissions(&prompts, std::fs::Permissions::from_mode(0o000)).unwrap();
    let listed = load_prompts(&config);
    std::fs::set_permissions(&prompts, std::fs::Permissions::from_mode(0o700)).unwrap();
    // root lists any directory; anyone else is refused.
    if let Err(e) = listed {
        assert_eq!(e.0, format!("cannot list {prompts}: Permission denied"));
    }
}

#[test]
fn without_prompt_files_nothing_changes() {
    let (dir, config) = prompt_dir(&[("notes.txt", b"not a prompt file")]);
    assert_eq!(load_prompts(&config).unwrap(), Prompts::default());
    assert_eq!(load_prompts(&ConfigDir::at(&dir.join("missing"))).unwrap(), Prompts::default());
}

#[test]
fn an_override_ends_with_one_newline_less_and_its_line_ends_read_as_newlines() {
    let (_dir, config) = prompt_dir(&[("BLOCKED_NOTE.md", b"Ask.\n"), ("CONTINUE_PROMPT.md", b"Go on\r\nto\r{path}.\r")]);
    let prompts = load_prompts(&config).unwrap();
    assert_eq!(prompts.text(Prompt::BlockedNote), "Ask.\n");
    // "\r\n" with the editor's newline is the one trailing newline left out.
    assert_eq!(prompts.text(Prompt::ContinuePrompt), "Go on\nto\n{path}.");
    let overrides = prompts.overrides();
    assert_eq!(overrides.keys().collect::<Vec<_>>(), ["CONTINUE_PROMPT", "BLOCKED_NOTE"]);
    assert_eq!(Prompts::from_overrides(&overrides).unwrap(), prompts);
}

#[test]
fn a_state_that_replaces_an_unknown_prompt() {
    let overrides: Map<String> = [("LATER_NOTE", "x".to_string()), ("ANOTHER", "y".to_string())].into_iter().collect();
    assert_eq!(
        saved_prompts(&overrides, "20260929-120000-a1b2c3").unwrap_err().0,
        "run state 20260929-120000-a1b2c3 replaces prompt ANOTHER, which this orchestrator lacks"
    );
    assert_eq!(saved_prompts(&Map::new(), "r").unwrap(), Prompts::default());
}

#[test]
fn a_state_whose_prompt_an_earlier_release_could_fill_in_is_refused() {
    let overrides: Map<String> = [("CONTINUE_PROMPT", "Go on to {path!s}.".to_string())].into_iter().collect();
    assert_eq!(
        saved_prompts(&overrides, "r").unwrap_err().0,
        "run state r replaces prompt CONTINUE_PROMPT with a text this orchestrator cannot fill in: placeholder {path!s} \
         at line 1, column 10 has a conversion; a placeholder takes none; finish the run with the release that started it"
    );
}

#[test]
fn a_state_keeps_the_fix_prompt_an_earlier_release_saved() {
    // Saved before 0.58.0, whose FIX_PROMPT named {prev_review_path}: refused as a prompt file (see
    // the_builders_fix_prompt_names_the_findings_that_sent_the_round_back), read as saved.
    let earlier = "Fix {prev_review_path}; report to {build_path}.";
    let overrides: Map<String> = [("FIX_PROMPT", earlier.to_string())].into_iter().collect();
    assert_eq!(saved_prompts(&overrides, "r").unwrap().text(Prompt::FixPrompt), earlier);
    // Only one text's placeholders, not a mix of the earlier and today's.
    for (name, text) in [
        ("FIX_PROMPT", "Fix {prev_review_path} and {findings}; report to {build_path}."),
        ("RECHECK_PROMPT", "Recheck {prev_review_path}, {build_path}, {change}, {spec_path}; write {review_path}."),
    ] {
        let overrides: Map<String> = [(name, text.to_string())].into_iter().collect();
        assert!(saved_prompts(&overrides, "r").is_err(), "{name}: {text}");
    }
}

#[test]
fn a_state_keeps_an_override_that_leaves_out_a_placeholder_its_built_in_text_has() {
    // A release that adds a placeholder to a built-in text must not refuse the runs that saved an override without it.
    let text = "Recheck {change}.";
    let overrides: Map<String> = [("RECHECK_PROMPT", text.to_string())].into_iter().collect();
    assert_eq!(saved_prompts(&overrides, "r").unwrap().text(Prompt::RecheckPrompt), text);
    assert!(prompt_problem("RECHECK_PROMPT", text).unwrap().contains("is missing"));
    // An earlier text's placeholders may be left out too.
    let overrides: Map<String> = [("FIX_PROMPT", "Fix {prev_review_path}.".to_string())].into_iter().collect();
    assert!(saved_prompts(&overrides, "r").is_ok());
}

#[test]
fn a_state_and_a_file_both_refuse_an_unknown_placeholder_and_a_template_that_does_not_parse() {
    for text in ["Recheck {change} {nothing}.", "Recheck {change"] {
        let overrides: Map<String> = [("RECHECK_PROMPT", text.to_string())].into_iter().collect();
        assert!(saved_prompts(&overrides, "r").is_err(), "{text}");
        assert!(prompt_problem("RECHECK_PROMPT", text).is_some(), "{text}");
    }
}

#[test]
fn a_problem_names_no_prompt_that_does_not_exist() {
    assert_eq!(prompt_problem("NO_SUCH_PROMPT", "x"), None);
}
